use crate::frontend::{ExpressionInput, PhysicalOperand, SourceEffect, SourceOperand};
use crate::llvm::registered::{PreparedSuiteEmitter, emit_operand, emit_physical_operand, emit_prepared_suite};
mod declarations;
use super::sites::{CheckedSites, CompiledDefinition};

use super::{AnnotationScope, FrameEmitter, Output, RegisteredFrame, SourceMap};
use crate::llvm::registered::context::{Finally, Region, RegionEmitter};
use imre_ast::code_flags;
use ruff_text_size::TextSize;
use serde_json::json;

use super::{Body, DefinitionName, Entry, Function};

/// The enclosing lexical scope and the existing address table used to place checked callables.
pub(in crate::dynamic::emit) struct CallableSites<'a, 'p> {
    source: &'a SourceMap<'p>,
    output: &'a mut Output,
    sites: &'a mut CheckedSites,
    prefix: &'a str,
    declared_globals: &'a std::collections::BTreeSet<String>,
    in_function: bool,
    method_scope: bool,
    class_locals: Option<&'a crate::frontend::function_scope::ClassBindings>,
}

impl<'a, 'p> crate::llvm::registered::PreparedSuiteEmitter<'a, 'p> for Function<'a, 'p> {
    fn frame_and_sites(
        &mut self,
        operand: Option<&crate::ir::Expr>,
    ) -> (&mut RegisteredFrame<'a, 'p>, &CheckedSites) {
        if let Some(operand) = operand {
            self.prepare_checked_value(operand);
        }
        (&mut self.frame, &self.sites)
    }

    fn place_statements(&mut self, body: &[crate::ir::Statement]) {
        self.callable_sites().place_statements(body);
    }

    fn emit_effect(
        &mut self,
        effect: &'p SourceEffect,
    ) -> Option<(&'p crate::ir::Statement, &'p [PhysicalOperand])> {
        match effect {
            SourceEffect::Assertion(site) => self.emit_assertion_inputs(site),
            SourceEffect::Class {
                site,
                layout,
                inputs,
            } => {
                self.mixed_class_definition(*site, layout, inputs);
            }
            SourceEffect::Match(range, site) => {
                self.checkpoint(range);
                self.simple_call = None;
                self.prepared_match(site);
            }
            SourceEffect::Context(context) => self.prepared_context(context),
            SourceEffect::Try(region) => {
                self.checkpoint(&region.range);
                self.simple_call = None;
                self.prepared_try(region);
            }
            SourceEffect::For(region) => self.prepared_for(region),
            SourceEffect::If { range, arms } => {
                self.checkpoint(range);
                self.simple_call = None;
                crate::llvm::operations::choose(
                    self,
                    |code| {
                        code.flush_check();
                        &mut code.frame.body
                    },
                    arms.iter().map(|(test, suite)| (test.as_ref(), suite)),
                    Self::branch_physical_operand,
                    |code, suite| emit_prepared_suite(code, suite),
                    true,
                );
            }
            SourceEffect::While(region) => self.prepared_while(region),
            SourceEffect::Refused(range, reason) => {
                self.refuse_at(*range, reason);
            }
            SourceEffect::Value(statement, inputs) => {
                return Some((statement, inputs));
            }
            SourceEffect::ObjectOperation(op, inputs) => {
                let (receiver, args) = inputs.split_first().expect("object operation receiver");
                self.object_operation(op, receiver, args.iter(), Self::physical_operand);
            }
        }
        None
    }

    fn refuse_at(&mut self, range: ruff_text_size::TextRange, reason: &str) {
        self.frame
            .output
            .diagnostic
            .get_or_insert_with(|| self.source.diagnostic_at(range, reason));
    }
}

impl<'a, 'p> RegionEmitter<'a, 'p> for Function<'a, 'p> {
    fn emit_finally(&mut self, suite: &Finally<'p>) {
        // Checked final suites are nested inside the checked run that owns their bindings.
        let Finally::Prepared(suite) = suite else {
            unreachable!("checked finally outside its checked run")
        };
        emit_prepared_suite(self, suite);
    }
}

impl SourceMap<'_> {
    pub(in crate::dynamic::emit) fn finish_class_annotations(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        prefix: &str,
        in_function: bool,
        plan: &crate::frontend::ClassAnnotations,
        globals: &std::collections::BTreeSet<String>,
    ) {
        let Some(body) = &plan.body else {
            return;
        };
        if frame.body.terminated {
            return;
        }
        let offsets = if plan.sites.iter().any(|site| site.index.is_some()) {
            let offsets = frame.op("list", &[]);
            for site in plan.sites.iter().filter(|site| site.index.is_some()) {
                let offset = frame.integer(site.site.to_u32());
                frame.op("append", &[&offsets, &offset]);
            }
            offsets
        } else {
            "0".into()
        };
        let signature = json!({
            "posonly": ["format"], "scope_complete": !plan.incomplete, "scope_reads": plan.reads,
            "code_flags": self.function_flags(in_function, false),
        });
        let scope = AnnotationScope::Class {
            globals: globals.clone(),
            conditional: plan.sites.iter().any(|entry| entry.index.is_some()),
        };
        let compiled = self.annotation_provider(
            frame.output,
            body,
            scope,
            plan.sites[0].site,
            signature,
            format!("{prefix}__annotate__"),
        );
        let environment = frame.op(
            "class_annotation_environment",
            &[&frame.scope.clone(), &offsets],
        );
        let function = compiled.bind_defaults(frame, &environment, &[]);
        frame.bind("__annotate_func__", &function);
    }

    fn annotation_provider(
        &self,
        output: &mut Output,
        body: &crate::frontend::AnnotationProvider,
        scope: AnnotationScope,
        start: TextSize,
        signature: serde_json::Value,
        qualname: String,
    ) -> CompiledDefinition {
        // Class annotations already have an explicit class environment. A method's provider
        // instead captures the class namespace of the function it annotates.
        let annotation_scope = !matches!(&scope, AnnotationScope::Class { .. });
        let code = self.definition(
            output,
            String::new(),
            Body::CheckedProvider(body, scope),
            start,
            false,
            Entry::Compiled,
        );
        CompiledDefinition::new(code, signature, "__annotate__", qualname, annotation_scope)
    }

    /// Publish a module's provider before its first statement. Every simple
    /// annotation, including an unconditional one, becomes visible only after
    /// execution reaches its assignment.
    pub(super) fn define_module_annotations(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        sites: &[TextSize],
    ) {
        let body = self.module_annotations.module_body(sites);
        let compiled = self.annotation_provider(
            frame.output,
            &body,
            AnnotationScope::Plain,
            self.module_start,
            json!({
                "posonly": ["format"], "scope_complete": true, "scope_locals_complete": true,
                "code_flags": self.function_flags(false, false),
            }),
            "__annotate__".into(),
        );
        let scope = frame.scope.clone();
        let provider = compiled.bind_defaults(frame, &scope, &[]);
        frame.bind("__annotate__", &provider);
    }
}

impl<'p> Function<'_, 'p> {
    pub(super) fn declare_globals(&mut self, globals: &std::collections::BTreeSet<String>) {
        for name in globals {
            self.declared_globals.insert(name.to_owned());
            self.frame.declare_global(name);
        }
    }

    fn emit_assertion_inputs(&mut self, site: &'p crate::frontend::AssertionSite) {
        self.checkpoint(&site.test_range);
        self.simple_call = None;
        crate::llvm::operations::choose(
            self,
            |code| {
                code.flush_check();
                &mut code.frame.body
            },
            [(Some(&site.test), false), (None, true)],
            Self::branch_physical_operand,
            |code, failed| {
                if failed {
                    let message = site
                        .message
                        .as_ref()
                        .map(|input| code.physical_operand(input));
                    code.frame.assertion_failure(
                        site.statement,
                        site.test_range.start(),
                        message.as_deref(),
                    );
                }
            },
            false,
        );
    }

    fn branch_physical_operand(&mut self, test: &PhysicalOperand, yes: &str, no: &str) {
        match test {
            PhysicalOperand::Checked(operand) => {
                self.prepare_checked_value(&operand.value);
                let bit = self.frame.prepared_test(operand, &self.sites);
                self.branch(&bit, yes, no);
            }
            PhysicalOperand::Residual(input) => self.branch_input(input.as_input(), yes, no),
        }
    }

    fn branch_input(&mut self, input: ExpressionInput<'_>, yes: &str, no: &str) {
        let bit = self
            .test_with_temporaries(|code| emit_operand(code, SourceOperand::Residual(input), true));
        self.branch(&bit, yes, no);
    }

    pub(super) fn physical_operand(&mut self, input: &PhysicalOperand) -> String {
        emit_physical_operand(self, input)
    }

    fn prepared_match(&mut self, site: &'p crate::frontend::SourceMatch) {
        use crate::llvm::registered::patterns::MatchRegion;

        let mark = self.temporary_mark();
        let subject = self.physical_operand(&site.subject);
        let mut regions = self
            .frame
            .registered_match(&subject, &mark, site, &self.sites);
        while let Some(region) = regions.next(&mut self.frame) {
            match region {
                MatchRegion::Guard {
                    case,
                    matched,
                    failed,
                } => match site.cases[case].guard.as_ref().expect("match guard") {
                    PhysicalOperand::Checked(operand) => {
                        self.prepare_checked_value(&operand.value);
                        self.frame
                            .prepared_branch(operand, &self.sites, matched, failed);
                    }
                    PhysicalOperand::Residual(input) => {
                        self.branch_input(input.as_input(), matched, failed)
                    }
                },
                MatchRegion::Body(case) => emit_prepared_suite(self, &site.cases[case].body),
            }
        }
    }

    fn mixed_class_definition(
        &mut self,
        site: TextSize,
        layout: &crate::ir::PackedArguments,
        inputs: &[PhysicalOperand],
    ) {
        let class = &self.source.class_bodies[&site];
        let (decorators, operands) = inputs.split_at(class.decorator_offsets.len());
        let decorators =
            self.decorator_operands(decorators, |code, operand| code.physical_operand(operand));
        let arguments = self.class_arguments(layout, |code, index| {
            code.physical_operand(&operands[index])
        });
        self.callable_sites().place_definition(site);
        let value = self
            .sites
            .definition(&mut self.frame, site, &decorators, &[], Some(arguments));
        self.bind(class.name.as_str(), &value);
    }
}

impl CallableSites<'_, '_> {
    pub(in crate::dynamic::emit) fn compile_class_namespace(
        &mut self,
        site: TextSize,
    ) -> CompiledDefinition {
        self.compile_definition(
            site,
            DefinitionName::Binding(&self.source.class_bodies[&site].name),
            Body::Namespace,
        )
    }

    fn compile_definition(
        &mut self,
        start: TextSize,
        name: DefinitionName<'_>,
        body: Body<'_>,
    ) -> CompiledDefinition {
        let (evaluator_scope, type_parameter_scope) = match name {
            DefinitionName::Evaluator {
                scope,
                type_parameter_scope,
                ..
            } => (Some(scope), type_parameter_scope),
            _ => (None, false),
        };
        let class = matches!(name, DefinitionName::ClassHeader(_))
            .then(|| &self.source.class_bodies[&start]);
        let (name, global, generator) = match name {
            DefinitionName::Binding(identifier) => (
                self.source.private_names.definition(identifier),
                self.declared_globals.contains(identifier.as_str()),
                None,
            ),
            DefinitionName::ClassHeader(name) | DefinitionName::Evaluator { name, .. } => {
                (name, false, None)
            }
            DefinitionName::Generator(site) => ("<genexpr>", false, Some(site)),
        };
        let qualname = if global {
            name.to_owned()
        } else {
            format!("{}{name}", self.prefix)
        };
        let class_header = class.map(|class| class.range);
        let prefix = if let Some(class) = class {
            if self.declared_globals.contains(class.name.as_str()) {
                String::new()
            } else {
                self.prefix.to_owned()
            }
        } else if matches!(body, Body::Namespace) || generator.is_some() {
            // Nested code in a comprehension uses its name without a <locals> segment.
            format!("{qualname}.")
        } else {
            format!("{qualname}.<locals>.")
        };
        let generator_flags = generator.map_or(0, |site| {
            if self.source.generators[&site].is_async {
                code_flags::ASYNC_GENERATOR
            } else {
                code_flags::GENERATOR
            }
        });
        let mut signature = json!({});
        if !matches!(body, Body::Namespace) {
            let nested = self.in_function || type_parameter_scope;
            let method = self.method_scope && class_header.is_none() && evaluator_scope.is_none();
            let flags = self.source.function_flags(nested, method);
            signature["code_flags"] = json!(flags | generator_flags);
        }
        if generator.is_some() {
            // The creating frame supplies the already-acquired iterator.
            signature["positional"] = json!([".0"]);
        }
        let entry = match generator_flags {
            0 => Entry::Compiled,
            flags => Entry::Suspended(flags),
        };
        let scopes = &self.source.scopes;
        let scope_facts = match &body {
            Body::Namespace => {
                let class = &scopes.classes[&start];
                signature["postponed_annotations"] = json!(class.postponed_annotations);
                signature["needs_class_cell"] = json!(class.needs_class_cell);
                signature["needs_dict_cell"] = json!(class.needs_dict_cell);
                signature["static_attributes"] = json!(class.static_attributes);
                signature["class_frame_reads"] = json!(class.frame_reads);
                if !class.bindings.globals.is_empty() {
                    signature["class_globals"] = json!(class.bindings.globals);
                }
                Some(&class.namespace)
            }
            _ if evaluator_scope.is_some() => evaluator_scope,
            _ => generator.map(|site| &scopes.generators[&site]),
        };
        if let Some(facts) = scope_facts {
            facts.write_signature(&mut signature);
            if generator.is_some() && self.method_scope {
                signature["method_class_cell"] = json!(facts.captures_class);
            }
        } else {
            signature["scope_cellvars"] = serde_json::Value::Null;
            signature["scope_complete"] = json!(false);
            signature["scope_locals_complete"] = json!(true);
        }
        if evaluator_scope.is_some() {
            signature["posonly"] = json!([".format"]);
            if matches!(body, Body::Operand(_, _, Some(_))) {
                signature["annotation_class"] = json!(true);
            }
        }
        let in_function = self.in_function || !matches!(body, Body::Namespace);
        let code = self
            .source
            .definition(self.output, prefix, body, start, in_function, entry);
        CompiledDefinition {
            class_header,
            format_evaluator: evaluator_scope.is_some(),
            ..CompiledDefinition::new(
                code,
                signature,
                name,
                qualname,
                class_header.is_some() || evaluator_scope.is_some(),
            )
        }
    }
}

impl<'p> Function<'_, 'p> {
    fn physical_binding(&mut self, target: &'p crate::frontend::PhysicalBinding, value: &str) {
        use crate::frontend::PhysicalBinding;
        match target {
            PhysicalBinding::Checked {
                entered, binding, ..
            } => {
                self.initialize_binding(binding, |frame| frame.store_private(*entered, value));
            }
            PhysicalBinding::Refused(range, reason) => {
                self.refuse_at(*range, reason);
            }
        }
    }

    fn initialize_binding(
        &mut self,
        binding: &'p crate::frontend::RegisteredFunction,
        initialize: impl FnOnce(&mut super::RegisteredFrame<'_, 'p>),
    ) {
        self.callable_sites()
            .place_statements(&binding.function.body);
        self.frame.checked_binding(binding, &self.sites, initialize);
    }

    fn prepared_context(&mut self, context: &'p crate::frontend::PreparedContext) {
        self.checkpoint(&context.range);
        self.simple_call = None;
        let inputs = &context.inputs;
        self.with_contexts(
            inputs.iter().map(|input| input.site),
            |code, i| {
                let previous = std::mem::replace(&mut code.current_range, inputs[i].site.range);
                let manager = code.physical_operand(&inputs[i].manager);
                code.current_range = previous;
                manager
            },
            |code, i, value| {
                if let Some(target) = &inputs[i].target {
                    code.physical_binding(target, value);
                }
            },
            |code| emit_prepared_suite(code, &context.body),
        );
    }

    fn prepared_while(&mut self, region: &'p crate::frontend::PreparedWhile) {
        use crate::frontend::WhileGuard;
        if matches!(region.condition, WhileGuard::Literal(false)) {
            if let Some(otherwise) = &region.otherwise {
                emit_prepared_suite(self, otherwise);
            }
            return;
        }
        crate::llvm::operations::loop_region(
            self,
            |code| {
                code.flush_check();
                &mut code.frame.body
            },
            |code| code.name("b"),
            |code, edges| code.jump(&edges.step),
            |code, yes, no| match &region.condition {
                WhileGuard::Literal(taken) => {
                    code.branch(if *taken { "true" } else { "false" }, yes, no);
                }
                WhileGuard::Operand(test) => code.branch_physical_operand(test, yes, no),
            },
            |code, edges, ()| {
                let depth = code.regions.len();
                code.push_loop(edges.clone(), depth, depth);
                emit_prepared_suite(code, &region.body);
                code.loops.pop();
            },
            region
                .otherwise
                .as_ref()
                .map(|otherwise| |code: &mut Self| emit_prepared_suite(code, otherwise)),
        );
    }

    fn prepared_for(&mut self, region: &'p crate::frontend::PreparedFor) {
        self.checkpoint(&region.range);
        self.simple_call = None;
        let cursor = self.acquire_iterator(region.acquire, |code| {
            code.physical_operand(&region.iterable)
        });

        let guarded = match &region.target {
            crate::frontend::PhysicalBinding::Checked {
                entered,
                binding,
                pair: Some(pair),
            } => {
                let plain = self.value(&format!(
                    "call i1 @imre_dyn_plain_items_cursor(ptr %ctx, i64 {cursor})"
                ));
                Some((binding, *entered, pair, plain))
            }
            _ => None,
        };
        let enclosing = self.regions.len();
        self.live.push(cursor.clone());
        let outer = self.body.error_target.clone();
        self.regions.push(Region::Pending {
            value: cursor.clone(),
            enclosing: None,
            outer,
        });
        let unwind = self.begin_operand_scope();

        crate::llvm::operations::loop_region(
            self,
            |code| {
                code.flush_check();
                &mut code.frame.body
            },
            |code| code.name("b"),
            |code, edges| code.jump(&edges.step),
            |code, item, exhausted| {
                if let Some((_, _, _, plain)) = &guarded {
                    code.frame.sync_iteration_edge(
                        &cursor,
                        region.range.start(),
                        region.otherwise.is_some(),
                        Some(plain),
                        item,
                        exhausted,
                    );
                    None
                } else {
                    code.frame.iteration_edge(
                        &cursor,
                        region.pull,
                        region.range.start(),
                        region.otherwise.is_some(),
                        item,
                        exhausted,
                    )
                }
            },
            |code, edges, pulled| {
                if let Some((binding, entered, pair, plain)) = &guarded {
                    code.initialize_binding(binding, |frame| {
                        frame.bind_item_pair(&cursor, plain, *entered, pair)
                    });
                } else {
                    let item = code.frame.iteration_item(&cursor, pulled);
                    code.physical_binding(&region.target, &item);
                }
                code.push_loop(edges.clone(), code.regions.len(), enclosing);
                emit_prepared_suite(code, &region.body);
                code.loops.pop();
            },
            // Even a loop without an else must release the cursor on exhaustion.
            Some(|code: &mut Self| {
                code.regions.pop();
                code.live.pop();
                code.finish_unwind(unwind, |frame| frame.release_target(None));
                code.release_target(None);
                if let Some(otherwise) = &region.otherwise {
                    emit_prepared_suite(code, otherwise);
                }
            }),
        );
    }

    fn protected_suite(&mut self, region: &'p crate::frontend::PreparedTry) {
        crate::llvm::registered::handlers::protected(
            self,
            &region.body,
            &region.handlers,
            &region.otherwise,
            region.is_star,
            |code, handler| {
                let class = handler.matcher.as_ref().map(|(range, input)| {
                    code.checkpoint(range);
                    code.physical_operand(input)
                });
                (class, handler.name.as_deref(), &handler.body)
            },
            |code, body| emit_prepared_suite(code, body),
        );
    }

    fn prepared_try(&mut self, region: &'p crate::frontend::PreparedTry) {
        let Some(suite) = &region.finalbody else {
            self.protected_suite(region);
            return;
        };
        crate::llvm::registered::handlers::finalized(self, Finally::Prepared(suite), |code| {
            code.protected_suite(region)
        });
    }
}
