use crate::frontend::{PhysicalOperand, SourceEffect};
mod declarations;
use super::sites::{CheckedSites, CompiledDefinition, CompiledProvider};

use super::{AnnotationScope, FrameEmitter, Output, RegisteredFrame, SourceMap};
use crate::llvm::registered::context::{ContextEmitter, ContextRegion};
use imre_ast::code_flags;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};
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
        let signature = frame.text(
            &json!({
                "posonly": ["format"], "scope_complete": true, "scope_locals_complete": true,
                "code_flags": self.function_flags(None, false, false),
            })
            .to_string(),
        );
        let body = crate::frontend::DeferredAnnotation::prepare_body(
            &self.annotations,
            &entries,
            self.module_annotation_bindings.clone(),
        );
        let code = self.definition(
            frame.output,
            String::new(),
            Body::AnnotationProvider(&body, AnnotationScope::Plain),
            self.module_start,
            false,
            Entry::Compiled,
        );
        let name = frame.text("__annotate__");
        let provider = frame.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {signature}, i64 0, i64 {name}, i64 0, i64 0, i1 true)", frame.scope));
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

    pub(super) fn checked_runs(
        &mut self,
        prepared: &'p crate::frontend::RegisteredSuite,
        mut emit_source: impl FnMut(&mut Self, &'p crate::frontend::SourceRun),
    ) {
        let Some(enclosing) = self.frame.enter_checked_slots(prepared.checked.region()) else {
            return;
        };
        let body = &prepared.checked.function.body;
        let mut offset = 0;
        for run in &prepared.residual {
            if offset < run.at && self.start_statement() {
                self.checked_block(&body[offset..run.at], &prepared.checked);
            }
            emit_source(self, run);
            offset = run.at;
        }
        if offset < body.len() && self.start_statement() {
            self.checked_block(&body[offset..], &prepared.checked);
        }
        self.frame.restore_checked_slots(enclosing);
    }

    pub(super) fn source_run(
        &mut self,
        suite: &'p [ast::Stmt],
        run: &'p crate::frontend::SourceRun,
    ) {
        let Some(effect) = &run.effect else {
            self.source_statements(&suite[run.statements.clone()]);
            return;
        };
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
                if let Some(site) = site {
                    self.prepared_match(site);
                } else {
                    self.unsupported("unsupported value in match pattern");
                }
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
            SourceEffect::RefusedTargetOperand(range, reason) => {
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

    pub(super) fn source_statements(&mut self, suite: &'p [ast::Stmt]) {
        for stmt in suite {
            if !self.start_statement() {
                break;
            }
            self.checkpoint(&stmt.range());
            self.simple_call = imre_ast::unhighlighted_call(stmt);
            match stmt {
                ast::Stmt::FunctionDef(v) => {
                    let function = self.function_definition(v);
                    self.bind(v.name.as_str(), &function);
                }
                _ => {
                    self.unsupported(&format!("{:?}", ast::AnyNodeRef::from(stmt).kind()));
                }
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
        self.prepare_definition(site);
        let value = self
            .sites
            .definition(&mut self.frame, site, &decorators, arguments);
        self.bind(class.name.as_str(), &value);
    }

    fn function_parameter_scope(&mut self, outer: &str, params: &ast::TypeParams) -> String {
        let params =
            crate::frontend::registered_type_parameters(params, &self.source.private_names);
        self.check_suspended_parameters(&params, self.current_range);
        self.frame.checked_parameter_scope(outer, &params, false)
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

    fn namespace_type_parameters<'a>(
        &self,
        function: &'a ast::StmtFunctionDef,
    ) -> Option<&'a ast::TypeParams> {
        function.type_params.as_deref().filter(|_| {
            self.source
                .namespace_operations
                .contains_key(&function.name.range())
        })
    }

    fn function_definition(&mut self, v: &ast::StmtFunctionDef) -> String {
        let mut header: Vec<_> = v
            .decorator_list
            .iter()
            .map(|d| self.expr(&d.expression))
            .collect();
        if self.callable_sites().prepare_ordinary_function(v.start()) {
            // Residual decorators and defaults run in the enclosing scope, in source order.
            for default in v
                .parameters
                .iter_non_variadic_params()
                .filter_map(|p| p.default.as_deref())
            {
                header.push(self.with_source_range(default.range(), |code| code.expr(default)));
            }
            return self
                .sites
                .definition(&mut self.frame, v.start(), &header, None);
        }
        // A method's lazy annotations also need its defining class namespace.
        let type_params =
            if self.module || (self.class_locals.is_some() && !self.source.string_annotations) {
                self.namespace_type_parameters(v)
            } else {
                crate::frontend::function_scope::boxed_function_type_params(
                    v,
                    self.source.string_annotations,
                    !self.prefix.is_empty(),
                    Some(&self.source.private_names),
                )
            };
        let function = self.define(
            v.decorator_list.first().map_or(v.start(), Ranged::start),
            DefinitionName::Binding(&v.name),
            Some(&v.parameters),
            Some(v),
            type_params,
            self.source.function_body(v),
        );
        self.apply_decorators(&header, &v.decorator_list, function)
    }

    pub(super) fn define(
        &mut self,
        start: TextSize,
        name: DefinitionName<'_>,
        params: Option<&ast::Parameters>,
        function: Option<&ast::StmtFunctionDef>,
        type_params: Option<&ast::TypeParams>,
        body: Body<'_>,
    ) -> String {
        let compiled = self.callable_sites().compile_definition(
            start,
            name,
            params,
            function,
            type_params,
            body,
        );
        let doc = compiled.doc(&mut self.frame);
        let outer = self.scope.clone();
        let scope = type_params
            .map(|params| self.function_parameter_scope(&outer, params))
            .unwrap_or(outer);
        let operands = compiled.operands(&mut self.frame, &scope, doc);
        if let Some(params) = params {
            for arg in params.iter_non_variadic_params() {
                if let Some(default) = &arg.default {
                    let value = self.with_source_range(default.range(), |code| code.expr(default));
                    operands.default(&mut self.frame, arg.parameter.name.as_str(), &value);
                }
            }
        }
        let function = compiled.instantiate(&mut self.frame, &scope, operands);
        if type_params.is_some() {
            let key = self.code_name(".type_params");
            let params = self.op("lookup", &[&scope, &key]);
            let key = self.code_name("__type_params__");
            self.op("setattr", &[&function, &key, &params]);
        }
        function
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
            None,
            None,
            None,
            Body::Namespace,
        )
    }

    /// Compile the body and its annotation provider with the declaring lexical scope.
    fn compile_definition(
        &mut self,
        start: TextSize,
        name: DefinitionName<'_>,
        params: Option<&ast::Parameters>,
        function: Option<&ast::StmtFunctionDef>,
        type_params: Option<&ast::TypeParams>,
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
        // A generic declaration's immediate lexical parent is its type-parameter scope.
        let generic = function.is_some_and(|f| f.type_params.is_some())
            || type_alias && self.source.type_aliases[&start].parameters.is_some();
        let nested = self.in_function || generic;
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
        let named_scope = function.map(|def| self.source.scopes.function(def));
        let declaration =
            function.map(|def| &self.source.function_bodies[&def.start()].declaration);
        let doc = if self.output.keep_docstrings {
            declaration.and_then(|facts| {
                facts
                    .docstring
                    .clone()
                    .or_else(|| self.source.bundled_docstring(&qualname))
            })
        } else {
            None
        };
        let generator_flags = if let Some(site) = generator {
            if self.source.generators[&site].is_async {
                code_flags::ASYNC_GENERATOR
            } else {
                code_flags::GENERATOR
            }
        } else {
            named_scope.map_or(0, |scope| scope.suspension_flags)
        };
        let flags = match &body {
            Body::Namespace => None,
            _ => {
                let mut flags = self.source.function_flags(
                    params,
                    nested,
                    self.method_scope && class_header.is_none() && !generic && !type_alias,
                );
                if doc.is_some() {
                    flags |= code_flags::HAS_DOCSTRING;
                }
                Some(flags | generator_flags)
            }
        };
        let annotations = params.and_then(|p| self.source.parameter_annotations.get(&p.start()));
        let provider = annotations.map(|annotations| {
            let class = self
                .class_locals
                .filter(|_| !self.source.string_annotations);
            let start = params.expect("annotated function parameters").start();
            let reads = self.source.scopes.annotations.parameter_reads(start, class);
            let mut metadata = json!({
                "posonly": ["format"],
                "code_flags": self.source.function_flags(None, nested, false),
                "scope_reads": reads,
                "scope_complete": true,
                "scope_locals_complete": true,
                "annotation_class": class.is_some(),
            });
            if !self.source.python_annotations && type_params.is_none() {
                for (slot, reference) in &annotations.references {
                    let root = reference.split('.').next().unwrap();
                    if !class.is_some_and(|class| class.globals.contains(root)) {
                        metadata["annotation_references"][slot] = json!(reference);
                    }
                }
            }
            let scope = class
                .cloned()
                .map_or(AnnotationScope::Plain, AnnotationScope::Method);
            let code = self.source.definition(
                self.output,
                String::new(),
                Body::AnnotationProvider(&annotations.body, scope),
                start,
                false,
                Entry::Compiled,
            );
            CompiledProvider {
                code,
                signature: metadata.to_string(),
            }
        });
        let mut signature = json!({});
        if let Some(flags) = flags {
            signature["code_flags"] = json!(flags);
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
            _ => named_scope
                .map(|scope| &scope.lexical)
                .or_else(|| generator.map(|site| &scopes.generators[&site])),
        };
        if let Some(facts) = scope_facts {
            facts.write_signature(&mut signature);
            let callable = named_scope.is_some();
            if callable && facts.captures_class {
                signature["class_cell"] = json!(true);
            }
            if (callable || generator.is_some()) && self.method_scope && type_params.is_none() {
                signature["method_class_cell"] = json!(facts.captures_class);
            }
        } else {
            signature["scope_cellvars"] = serde_json::Value::Null;
            signature["scope_complete"] = json!(false);
            signature["scope_locals_complete"] = json!(true);
        }
        if let Some(entry) = function.and_then(|def| self.source.thread_entries.get(&def.start())) {
            match entry {
                imre_ast::ThreadEntry::Parameters { names, lock_only } => {
                    signature["thread_parameters"] = json!(names);
                    if *lock_only {
                        signature["thread_worker_locks"] = json!(names);
                    }
                }
                imre_ast::ThreadEntry::Captured(names) => {
                    signature["thread_captured_locks"] = json!(names);
                }
                imre_ast::ThreadEntry::Bound(field) => {
                    signature["thread_bound_lock_field"] = json!(field)
                }
            }
        }
        // A type alias thunk has an implicit default for its format parameter.
        if type_alias {
            signature["posonly"] = json!([".format"]);
            if self.class_locals.is_some() {
                signature["annotation_class"] = json!(true);
            }
        }
        let mut default_names = Vec::new();
        let in_function = self.in_function || !matches!(body, Body::Namespace);
        // A generator call returns a suspended frame, before it yields or returns a value.
        let scalar = |slot: &str| {
            if self.source.python_annotations {
                return None;
            }
            let name = annotations?.references.get(slot)?;
            (!type_params.is_some_and(|params| params.iter().any(|p| p.name().as_str() == name)))
                .then_some(name)
        };
        if matches!(entry, Entry::Compiled)
            && let Some(ty) = scalar("return")
        {
            signature["return_type"] = json!(ty);
        }
        let code = self
            .source
            .definition(self.output, prefix, body, start, in_function, entry);
        if let Some(p) = params {
            self.output
                .interned_identifiers
                .extend(p.iter().map(|p| p.name().to_string()));
            default_names = crate::frontend::function_scope::parameter_layout(p, &mut signature);
            for arg in p.iter_non_variadic_params() {
                let name = arg.parameter.name.as_str();
                if let Some(ty) = scalar(name) {
                    signature["types"][name] = json!(ty);
                    if ty == "float"
                        && declaration.is_some_and(|facts| {
                            facts.integer_sign_parameters.iter().any(|p| p == name)
                        })
                    {
                        signature["integer_sign_parameters"][name] = json!(true);
                    }
                }
            }
        }
        CompiledDefinition {
            code,
            signature: signature.to_string(),
            name: name.to_owned(),
            qualname,
            doc,
            provider,
            default_names,
            class_header,
            type_alias,
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
        let signature = self.frame.text(&json!({
            "posonly": ["format"], "scope_complete": !plan.incomplete, "scope_reads": plan.reads,
            "code_flags": self.source.function_flags(None, self.in_function, false),
        }).to_string());
        let scope = AnnotationScope::Class {
            globals: globals.clone(),
            conditional: plan.sites.iter().any(|entry| entry.index.is_some()),
        };
        let code = self.source.definition(
            self.frame.output,
            String::new(),
            Body::AnnotationProvider(body, scope),
            plan.sites[0].site,
            false,
            Entry::Compiled,
        );
        let environment = self.frame.op(
            "class_annotation_environment",
            &[&self.scope.clone(), &offsets],
        );
        let name = self.text("__annotate__");
        let qualname = self.frame.text(&format!("{}__annotate__", self.prefix));
        let defaults = self.op("dict", &[]);
        let function = self.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {environment}, i64 {name}, i64 {signature}, i64 {defaults}, i64 {qualname}, i64 0, i64 0, i1 false)"));
        self.bind("__annotate_func__", &function);
    }

    fn apply_decorators(
        &mut self,
        decorators: &[String],
        positions: &[ast::Decorator],
        value: String,
    ) -> String {
        let lines = positions
            .iter()
            .map(|d| self.source.positions.line(d.expression.start()));
        self.frame.decorate(decorators, lines, value)
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

    pub(super) fn physical_suite(&mut self, suite: &'p crate::frontend::PhysicalSuite) {
        self.checked_runs(&suite.plan, |code, run| {
            code.source_run(&suite.sources, run)
        });
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
