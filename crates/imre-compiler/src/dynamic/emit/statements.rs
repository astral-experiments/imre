use crate::frontend::{PhysicalOperand, SourceEffect};
mod declarations;
use super::sites::{CheckedSites, CompiledDefinition};

use super::{AnnotationScope, FrameEmitter, Output, RegisteredFrame, SourceMap};
use crate::llvm::registered::context::{ContextEmitter, ContextRegion};
use imre_ast::code_flags;
use ruff_text_size::{TextRange, TextSize};
use serde_json::json;

use super::{Body, Cleanup, DefinitionName, Entry, Function};

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

impl<'a, 'p> ContextEmitter<'a, 'p> for Function<'a, 'p> {
    fn push_context(&mut self, exit: ContextRegion, outer: String) {
        self.exits.push((Cleanup::With(exit), outer));
    }

    fn pop_context(&mut self) -> (ContextRegion, String) {
        let Some((Cleanup::With(context), outer)) = self.exits.pop() else {
            unreachable!("active context region")
        };
        (context, outer)
    }
}

impl SourceMap<'_> {
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
            Body::AnnotationProvider(body, scope),
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
        let entries: Vec<_> = sites
            .iter()
            .enumerate()
            .map(|(index, site)| crate::ir::AnnotationOccurrence {
                site: *site,
                index: Some(index),
            })
            .collect();
        let body = crate::frontend::DeferredAnnotation::prepare_body(
            &self.annotations,
            &entries,
            self.module_annotation_bindings.clone(),
        );
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

    fn start_statement(&mut self) -> bool {
        if self.body.terminated {
            if self.module_annotation_sites.is_none() {
                return false;
            }
            // Unreachable statements still contribute to a module provider's site order.
            let dead = self.name("b");
            self.label(&dead);
        }
        true
    }

    fn checked_block(
        &mut self,
        statements: &[crate::ir::Statement],
        checked: &crate::frontend::RegisteredFunction,
    ) {
        self.prepare_checked_sites(statements);
        self.checked_region(|code| code.frame.checked_block(statements, checked, &code.sites));
    }

    /// Route checked returns through the exits surrounding this mixed region.
    fn checked_region(&mut self, emit: impl FnOnce(&mut Self)) {
        if self.exits.is_empty() {
            emit(self);
            return;
        }
        self.cleanup_returns = Some(Vec::new());
        emit(self);
        let returns = self.cleanup_returns.take().expect("active cleanup region");
        if returns.is_empty() {
            return;
        }
        let resume = (!self.body.terminated).then(|| self.name("b"));
        if let Some(label) = &resume {
            self.jump(label);
        }
        let range = self.current_range;
        for (label, value, site) in returns {
            self.label(&label);
            self.current_range = site;
            self.return_from_scope(value);
        }
        self.current_range = range;
        if let Some(label) = resume {
            self.label(&label);
        }
    }

    pub(super) fn registered_value(&mut self, value: &crate::ir::Expr) -> String {
        self.prepare_checked_value(value);
        self.frame.registered_value(value, &self.sites)
    }

    pub(super) fn physical_suite(&mut self, prepared: &'p crate::frontend::RegisteredSuite) {
        let Some(enclosing) = self.frame.enter_checked_slots(prepared.checked.region()) else {
            return;
        };
        let body = &prepared.checked.function.body;
        let mut offset = 0;
        for run in &prepared.residual {
            if offset < run.at && self.start_statement() {
                self.checked_block(&body[offset..run.at], &prepared.checked);
            }
            self.emit_effect(&run.effect);
            offset = run.at;
        }
        if offset < body.len() && self.start_statement() {
            self.checked_block(&body[offset..], &prepared.checked);
        }
        self.frame.restore_checked_slots(enclosing);
    }

    fn emit_effect(&mut self, effect: &'p SourceEffect) {
        if !self.start_statement() {
            return;
        }
        match effect {
            SourceEffect::LoopTransfer(range, transfer) => {
                self.checkpoint(range);
                self.simple_call = None;
                self.unwind_loop(transfer, |code, depth| code.run_cleanups(depth, None));
            }
            SourceEffect::Assertion(site) => self.emit_assertion_inputs(site),
            SourceEffect::Class(site) => self.mixed_class_definition(*site),
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
                    Self::physical_suite,
                    true,
                );
            }
            SourceEffect::While(region) => self.prepared_while(region),
            SourceEffect::Refused(range, reason) => {
                self.with_expression_range(*range, |code| code.build_diagnostic(reason));
            }
            SourceEffect::Value(output, inputs) => match output {
                crate::frontend::ValueStatement::Store(slot) => {
                    let value = self.physical_operand(&inputs[0]);
                    self.frame.store_private(*slot, &value);
                }
                crate::frontend::ValueStatement::Return => {
                    let value = inputs.first().map_or_else(
                        || super::permanent::NONE.to_string(),
                        |input| self.physical_operand(input),
                    );
                    self.return_from_scope(value);
                }
                crate::frontend::ValueStatement::Expression => {
                    let value = self.physical_operand(&inputs[0]);
                    self.discard_handle(&value);
                }
                crate::frontend::ValueStatement::Raise => {
                    self.raise_operands(inputs.first(), inputs.get(1), Self::physical_operand);
                }
                crate::frontend::ValueStatement::AugmentName(name, op) => {
                    let left = self.physical_operand(&inputs[0]);
                    let right = self.physical_operand(&inputs[1]);
                    let value = self.inplace(op, &left, &right);
                    self.bind(name, &value);
                }
                crate::frontend::ValueStatement::AugmentItem(op) => {
                    self.augment_item(
                        op,
                        &inputs[0],
                        &inputs[1],
                        &inputs[2],
                        Self::physical_operand,
                    );
                }
            },
            SourceEffect::Discard(inputs) => {
                for input in inputs {
                    self.discard_operand(|code| code.physical_operand(input));
                }
            }
            SourceEffect::ObjectOperation(op, inputs) => {
                let (receiver, args) = inputs.split_first().expect("object operation receiver");
                self.object_operation(op, receiver, args.iter(), Self::physical_operand);
            }
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
            PhysicalOperand::Checked(operand) => self.branch_operand(operand, yes, no),
            PhysicalOperand::Source(expr) => self.branch_expr(expr, yes, no),
        }
    }

    fn branch_operand(
        &mut self,
        operand: &crate::frontend::RegisteredOperand,
        yes: &str,
        no: &str,
    ) {
        self.prepare_checked_value(&operand.value);
        let bit = self
            .test_with_temporaries(|code| code.frame.prepared_operand(operand, &code.sites, true));
        self.branch(&bit, yes, no);
    }

    pub(super) fn physical_operand(&mut self, input: &PhysicalOperand) -> String {
        match input {
            PhysicalOperand::Checked(value) => {
                self.prepare_checked_value(&value.value);
                self.frame.prepared_operand(value, &self.sites, false)
            }
            PhysicalOperand::Source(expr) => self.expr(expr),
        }
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
                    PhysicalOperand::Source(expr) => self.branch_expr(expr, matched, failed),
                },
                MatchRegion::Body(case) => self.physical_suite(&site.cases[case].body),
            }
        }
    }

    fn mixed_class_definition(&mut self, site: TextSize) {
        let class = &self.source.class_bodies[&site];
        let header = &class.header;
        let decorators = header
            .decorators
            .iter()
            .map(|operand| self.physical_operand(operand))
            .collect::<Vec<_>>();
        let arguments = header.parameters.is_none().then(|| {
            self.class_arguments(&header.layout, |code, index| {
                code.physical_operand(&header.operands[index])
            })
        });
        self.callable_sites().place_definition(site);
        let value = self
            .sites
            .definition(&mut self.frame, site, &decorators, arguments);
        self.bind(class.name.as_str(), &value);
    }

    fn check_suspended_parameters(
        &mut self,
        params: &[crate::ir::RegisteredTypeParameter],
        range: TextRange,
    ) {
        if self.native_suspension && params.iter().any(|param| param.has_bound_or_default) {
            self.with_expression_range(range, |code| {
                code.unsupported("class type parameter bounds and defaults")
            });
        }
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
        let type_alias = matches!(name, DefinitionName::TypeAlias(_));
        let class = matches!(name, DefinitionName::ClassHeader(_))
            .then(|| &self.source.class_bodies[&start]);
        let (name, global, generator) = match name {
            DefinitionName::Binding(identifier) => (
                self.source.private_names.definition(identifier),
                self.declared_globals.contains(identifier.as_str()),
                None,
            ),
            DefinitionName::ClassHeader(name) | DefinitionName::TypeAlias(name) => {
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
            let nested = self.in_function
                || type_alias && self.source.type_aliases[&start].parameters.is_some();
            let method = self.method_scope && class_header.is_none() && !type_alias;
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
            _ if type_alias => Some(&scopes.aliases[&start]),
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
        // A type alias thunk has an implicit default for its format parameter.
        if type_alias {
            signature["posonly"] = json!([".format"]);
            if self.class_locals.is_some() {
                signature["annotation_class"] = json!(true);
            }
        }
        let in_function = self.in_function || !matches!(body, Body::Namespace);
        let code = self
            .source
            .definition(self.output, prefix, body, start, in_function, entry);
        CompiledDefinition {
            class_header,
            type_alias,
            ..CompiledDefinition::new(
                code,
                signature,
                name,
                qualname,
                class_header.is_some() || type_alias,
            )
        }
    }
}

impl<'p> Function<'_, 'p> {
    pub(in crate::dynamic::emit) fn finish_class_annotations(
        &mut self,
        plan: &crate::frontend::ClassAnnotations,
        globals: &std::collections::BTreeSet<String>,
    ) {
        let Some(body) = &plan.body else {
            return;
        };
        if self.body.terminated {
            return;
        }
        let offsets = if plan.sites.iter().any(|site| site.index.is_some()) {
            let offsets = self.op("list", &[]);
            for site in plan.sites.iter().filter(|site| site.index.is_some()) {
                let offset = self.integer(site.site.to_u32());
                self.op("append", &[&offsets, &offset]);
            }
            offsets
        } else {
            "0".into()
        };
        let signature = json!({
            "posonly": ["format"], "scope_complete": !plan.incomplete, "scope_reads": plan.reads,
            "code_flags": self.source.function_flags(self.in_function, false),
        });
        let scope = AnnotationScope::Class {
            globals: globals.clone(),
            conditional: plan.sites.iter().any(|entry| entry.index.is_some()),
        };
        let compiled = self.source.annotation_provider(
            self.frame.output,
            body,
            scope,
            plan.sites[0].site,
            signature,
            format!("{}__annotate__", self.prefix),
        );
        let environment = self.frame.op(
            "class_annotation_environment",
            &[&self.scope.clone(), &offsets],
        );
        let function = compiled.bind_defaults(&mut self.frame, &environment, &[]);
        self.bind("__annotate_func__", &function);
    }

    fn physical_binding(&mut self, target: &'p crate::frontend::PhysicalBinding, value: &str) {
        use crate::frontend::PhysicalBinding;
        match target {
            PhysicalBinding::Checked {
                entered, binding, ..
            } => {
                self.initialize_binding(binding, |frame| frame.store_private(*entered, value));
            }
            PhysicalBinding::Refused(range, reason) => {
                self.with_expression_range(*range, |code| code.build_diagnostic(reason));
            }
        }
    }

    fn initialize_binding(
        &mut self,
        binding: &'p crate::frontend::RegisteredFunction,
        initialize: impl FnOnce(&mut super::RegisteredFrame<'_, 'p>),
    ) {
        self.prepare_checked_sites(&binding.function.body);
        self.frame.checked_binding(binding, &self.sites, initialize);
    }

    fn prepared_context(&mut self, context: &'p crate::frontend::PreparedContext) {
        self.checkpoint(&context.range);
        self.simple_call = None;
        let inputs = &context.inputs;
        self.with_contexts(
            inputs.iter().map(|input| input.site),
            |code, i| {
                code.with_expression_range(inputs[i].site.range, |code| {
                    code.physical_operand(&inputs[i].manager)
                })
            },
            |code, i, value| {
                if let Some(target) = &inputs[i].target {
                    code.physical_binding(target, value);
                }
            },
            |code| code.physical_suite(&context.body),
        );
    }

    fn prepared_while(&mut self, region: &'p crate::frontend::PreparedWhile) {
        use crate::frontend::WhileGuard;
        if matches!(region.condition, WhileGuard::Literal(false)) {
            if let Some(otherwise) = &region.otherwise {
                self.physical_suite(otherwise);
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
                let depth = code.exits.len();
                code.push_loop(edges.clone(), depth, depth);
                code.physical_suite(&region.body);
                code.loops.pop();
            },
            region
                .otherwise
                .as_ref()
                .map(|otherwise| |code: &mut Self| code.physical_suite(otherwise)),
        );
    }

    fn prepared_for(&mut self, region: &'p crate::frontend::PreparedFor) {
        self.checkpoint(&region.range);
        self.simple_call = None;
        let cursor = self.iteration_result(|code| {
            let value = code.physical_operand(&region.iterable);
            if code.native_suspension {
                code.call_position(region.iterable_range);
            }
            code.op(region.acquire.runtime_op(), &[&value])
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
        let enclosing = self.exits.len();
        self.live.push(cursor.clone());
        self.exits.push((
            Cleanup::RetainedValue {
                value: cursor.clone(),
                enclosing_exception: None,
            },
            self.body.error_target.clone(),
        ));
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
                code.push_loop(edges.clone(), code.exits.len(), enclosing);
                code.physical_suite(&region.body);
                code.loops.pop();
            },
            // Even a loop without an else must release the cursor on exhaustion.
            Some(|code: &mut Self| {
                code.exits.pop();
                code.live.pop();
                code.finish_unwind(unwind, |frame| frame.release_target(None));
                code.release_target(None);
                if let Some(otherwise) = &region.otherwise {
                    code.physical_suite(otherwise);
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
            |code, body, cleanup| {
                if let Some((name, outer)) = cleanup {
                    code.exits
                        .push((Cleanup::ExceptionName(name.into()), outer.into()));
                }
                code.physical_suite(body);
                if cleanup.is_some() {
                    code.exits.pop();
                }
            },
        );
    }

    fn prepared_try(&mut self, region: &'p crate::frontend::PreparedTry) {
        let Some(suite) = &region.finalbody else {
            self.protected_suite(region);
            return;
        };
        crate::llvm::registered::handlers::finalized(
            self,
            |code, enclosing| {
                code.exits.push((
                    Cleanup::Finally {
                        body: suite,
                        loop_depth: code.loops.len(),
                        caught_depth: code.caught.len(),
                    },
                    enclosing.into(),
                ));
                code.protected_suite(region);
                code.exits.pop();
            },
            |code, pending| {
                if let Some((value, enclosing)) = pending {
                    code.exits.push((
                        Cleanup::RetainedValue {
                            value: value.into(),
                            enclosing_exception: Some(enclosing.into()),
                        },
                        code.body.error_target.clone(),
                    ));
                }
                code.physical_suite(suite);
                if pending.is_some() {
                    code.exits.pop();
                }
            },
        );
    }

    /// Collect discarded owners while retaining the operands needed after the transfer.
    pub(super) fn return_from_scope(&mut self, value: String) {
        self.live.push(value.clone());
        self.run_cleanups(0, Some(&value));
        self.live.pop();
        self.return_value(&value);
    }

    /// A return, break or continue inside a finally may replace the pending exit.
    fn run_cleanups(&mut self, depth: usize, pending: Option<&str>) {
        let mut left = Vec::new();
        let mut discarded = Vec::new();
        while self.exits.len() > depth && !self.body.terminated {
            let region = self.exits.pop().unwrap();
            let (cleanup, outer) = &region;
            let previous = std::mem::replace(&mut self.body.error_target, outer.clone());
            match cleanup {
                Cleanup::With(context) => self.leave_context(context, &mut discarded),
                Cleanup::ExceptionName(name) => self.clear_exception_name(name),
                Cleanup::RetainedValue {
                    value,
                    enclosing_exception,
                } => {
                    if let Some(caught) = enclosing_exception {
                        self.op("restore_caught", &[caught]);
                    }
                    let slot = self.live.iter().rposition(|live| live == value).unwrap();
                    discarded.push((slot, self.live.remove(slot)));
                    self.release_target(None);
                }
                Cleanup::Finally {
                    body,
                    loop_depth,
                    caught_depth,
                } => {
                    // Loops and handlers entered inside the try do not enclose its finally.
                    let protected_loops = self.loops.split_off(*loop_depth);
                    let protected_handlers = self.caught.split_off(*caught_depth);
                    let pending = pending.filter(|value| *value != "0");
                    if let Some(value) = pending {
                        self.exits.push((
                            Cleanup::RetainedValue {
                                value: value.into(),
                                enclosing_exception: None,
                            },
                            outer.clone(),
                        ));
                    }
                    self.physical_suite(body);
                    if pending.is_some() {
                        self.exits.pop();
                    }
                    self.caught.extend(protected_handlers);
                    self.loops.extend(protected_loops);
                }
            }
            self.body.error_target = previous;
            left.push(region);
        }
        for (slot, value) in discarded.into_iter().rev() {
            self.live.insert(slot, value);
        }
        self.exits.extend(left.into_iter().rev());
    }
}
