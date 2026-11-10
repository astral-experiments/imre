use crate::frontend::{PhysicalOperand, SourceEffect, SourceGuard};
mod declarations;
use super::sites::{CompiledDefinition, CompiledProvider};

use super::{AnnotationScope, FrameEmitter, Output, RegisteredFrame, SourceMap};
use crate::ir::Statement;
use crate::llvm::registered::Place;
use crate::llvm::registered::context::{ContextEmitter, ContextRegion};
use imre_ast::code_flags;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};
use serde_json::json;

use super::{Body, CheckedHeader, Cleanup, DefinitionName, Entry, Function};

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
        let code = self.annotation_body(frame.output, &entries, None, self.module_start);
        let name = frame.text("__annotate__");
        let provider = frame.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {signature}, i64 0, i64 {name}, i64 0, i64 0, i1 true)", frame.scope));
        frame.bind("__annotate__", &provider);
    }

    fn annotation_body(
        &self,
        output: &mut Output,
        entries: &[crate::ir::AnnotationOccurrence],
        globals: Option<&std::collections::BTreeSet<String>>,
        start: TextSize,
    ) -> String {
        let plan = crate::frontend::DeferredAnnotation::prepare_body(&self.annotations, entries);
        let scope = globals.map_or(AnnotationScope::Plain, |globals| AnnotationScope::Class {
            globals: globals.clone(),
            conditional: entries.iter().any(|entry| entry.index.is_some()),
        });
        self.definition(
            output,
            String::new(),
            Body::AnnotationProvider(&plan, entries, scope),
            start,
            false,
            Entry::Compiled,
        )
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

    /// A physically complete frame takes the canonical function entry. Partial
    /// frames interleave its checked blocks with only their unprepared source.
    pub(super) fn prepared_function(
        &mut self,
        prepared: &'p crate::frontend::RegisteredSuite,
        emit_source: impl FnMut(&mut Self, &'p crate::frontend::SourceRun),
    ) {
        if prepared.fully_checked() {
            self.checked_function(&prepared.checked);
        } else {
            self.checked_runs(prepared, emit_source);
        }
    }

    fn checked_statement(&mut self, statement: &Statement) {
        self.prepare_checked_sites(std::slice::from_ref(statement));
        self.checked_region(|code| code.frame.checked_statement(statement, &code.sites));
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

    fn emit_suite(
        &mut self,
        plan: Option<&'p crate::frontend::RegisteredSuite>,
        source: &'p [ast::Stmt],
    ) {
        if let Some(plan) = plan {
            self.checked_runs(plan, |code, run| code.source_run(source, run));
        } else {
            self.source_statements(source);
        }
    }

    pub(super) fn statements(&mut self, source: &'p [ast::Stmt]) {
        let plan = source
            .first()
            .and_then(|stmt| self.source.nested_bodies.get(&stmt.start()));
        self.emit_suite(plan, source);
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
                if self.loops.is_empty() {
                    self.unsupported("loop control outside a loop");
                } else {
                    self.unwind_loop(transfer, |code, depth| code.run_cleanups(depth, None));
                }
            }
            SourceEffect::Assertion(site) => {
                let source = suite[run.statements.start]
                    .as_assert_stmt()
                    .expect("external assertion input");
                self.emit_assertion_inputs(site, source);
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
                    |code, test, yes, no| match test {
                        PhysicalOperand::Checked(operand) => code.branch_operand(operand, yes, no),
                        PhysicalOperand::Source(expr) => code.branch_expr(expr, yes, no),
                    },
                    Self::physical_suite,
                    true,
                );
            }
            SourceEffect::While(condition) => {
                let source = suite[run.statements.start]
                    .as_while_stmt()
                    .expect("prepared loop");
                if matches!(condition, SourceGuard::Literal(false)) {
                    self.statements(&source.orelse);
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
                    |code, yes, no| match condition {
                        SourceGuard::Literal(taken) => {
                            code.branch(if *taken { "true" } else { "false" }, yes, no);
                        }
                        SourceGuard::Checked(operand) => code.branch_operand(operand, yes, no),
                        SourceGuard::Residual => code.branch_expr(&source.test, yes, no),
                    },
                    |code, edges, ()| {
                        let depth = code.exits.len();
                        code.push_loop(edges.clone(), depth, depth);
                        code.statements(&source.body);
                        code.loops.pop();
                    },
                    (!source.orelse.is_empty())
                        .then_some(|code: &mut Self| code.statements(&source.orelse)),
                );
            }
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

    fn emit_assertion_inputs(
        &mut self,
        site: &crate::frontend::AssertionSite,
        source: &'p ast::StmtAssert,
    ) {
        let [checkpoint, Statement::If(test, yes, no)] = site.body.as_slice() else {
            unreachable!("prepared assertion")
        };
        self.checked_statement(checkpoint);
        crate::llvm::operations::choose(
            self,
            |code| {
                code.flush_check();
                &mut code.frame.body
            },
            [(Some(test), yes), (None, no)],
            |code, test, yes, no| {
                if site.external_test {
                    code.branch_expr(&source.test, yes, no);
                } else {
                    code.prepare_checked_value(test);
                    code.frame.registered_branch(test, &code.sites, yes, no);
                }
            },
            |code, statements| {
                for statement in statements {
                    if site.external_message {
                        let message = code.expr(source.msg.as_deref().expect("external message"));
                        code.frame.checked_statement_with_inputs(
                            statement,
                            &code.sites,
                            &[message],
                        );
                    } else {
                        code.checked_statement(statement);
                    }
                }
            },
            false,
        );
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

    fn physical_operand(&mut self, input: &PhysicalOperand) -> String {
        match input {
            PhysicalOperand::Checked(value) => {
                self.prepare_checked_value(&value.value);
                self.frame.prepared_operand(value, &self.sites, false)
            }
            PhysicalOperand::Source(expr) => self.expr(expr),
        }
    }

    /// Supply only the source operands requested by the registered matcher.
    fn emit_match_inputs(
        &mut self,
        site: &'p crate::frontend::SourceMatch,
        source: &'p ast::StmtMatch,
    ) {
        use crate::llvm::registered::patterns::MatchRegion;

        let mark = self.temporary_mark();
        let subject = match &site.subject {
            Some(operand) => {
                self.prepare_checked_value(&operand.value);
                self.frame.prepared_operand(operand, &self.sites, false)
            }
            None => self.expr(&source.subject),
        };
        let mut regions = self
            .frame
            .registered_match(&subject, &mark, site, &self.sites);
        loop {
            if let Some(test) = regions.pending_checked_guard() {
                self.prepare_checked_value(test);
            }
            let Some(region) = self.frame.next_match_region(&mut regions, &self.sites) else {
                break;
            };
            match region {
                MatchRegion::ResidualGuard {
                    case,
                    matched,
                    failed,
                } => self.branch_expr(
                    source.cases[case].guard.as_deref().unwrap(),
                    matched,
                    failed,
                ),
                MatchRegion::Body(case) => match &site.cases[case].body {
                    Some(suite) => self.checked_runs(suite, |code, run| {
                        code.source_run(&source.cases[case].body, run);
                    }),
                    None => self.source_statements(&source.cases[case].body),
                },
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
            if let Some(site) = self.source.statement_sites.matches.get(&stmt.range()) {
                if let Some(site) = site {
                    self.emit_match_inputs(site, stmt.as_match_stmt().unwrap());
                } else {
                    self.unsupported("unsupported value in match pattern");
                }
                continue;
            }
            if let Some(super::DefinitionSite::Class(class)) = self
                .source
                .definitions
                .as_ref()
                .and_then(|sites| sites.get(&stmt.start()))
            {
                self.mixed_class_definition(class);
                continue;
            }
            if let Some(operation) = self.source.namespace_operations.get(&stmt.range()) {
                let value = self.registered_value(operation);
                if let crate::ir::ExprKind::Builtin(
                    crate::ir::Builtin::RegisteredTypeAlias(site),
                    _,
                ) = &operation.kind
                {
                    self.frame
                        .bind(&self.source.type_aliases[site].name, &value);
                }
                continue;
            }
            if !stmt.is_assign_stmt()
                && let Some(effect) = crate::frontend::attribute_write(stmt)
            {
                self.attribute_operation(
                    effect.action,
                    effect.field.attr.as_str(),
                    effect.field.range(),
                    effect.operands(),
                    |code, node| code.expr(node),
                );
                continue;
            }
            match stmt {
                ast::Stmt::Pass(_) | ast::Stmt::Global(_) | ast::Stmt::Nonlocal(_) => {}
                ast::Stmt::AnnAssign(v) => {
                    if let Some(value) = &v.value {
                        let value = self.expr(value);
                        if let ast::Expr::Name(target) = &*v.target {
                            let name = self.code_name(target.id.as_str());
                            let scope = self.scope.clone();
                            self.assign_owned_name(&scope, &name, &value, None);
                        } else {
                            self.assign_target(&v.target, &value);
                        }
                    }
                    if (self.module || self.class_locals.is_some())
                        && let Some(marker) =
                            self.source.namespace_operations.get(&v.target.range())
                    {
                        self.registered_value(marker);
                    }
                }
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

    fn class_range(&self, class: &ast::StmtClassDef) -> TextRange {
        let index = self
            .source
            .class_keywords
            .partition_point(|&p| p < class.name.start());
        TextRange::new(self.source.class_keywords[index - 1], class.end())
    }

    fn mixed_class_definition(&mut self, class: &ast::StmtClassDef) {
        let header = &self.source.class(class).header;
        let decorators = header
            .decorators
            .iter()
            .enumerate()
            .map(|(index, operand)| {
                self.class_operand(operand.as_ref(), || &class.decorator_list[index].expression)
            })
            .collect::<Vec<_>>();
        let arguments = header
            .parameters
            .is_none()
            .then(|| self.class_arguments(class));
        self.prepare_definition(class.start());
        let value = self
            .sites
            .definition(&mut self.frame, class.start(), &decorators, arguments);
        self.bind(class.name.as_str(), &value);
    }

    fn class_operand<'a>(
        &mut self,
        prepared: Option<&crate::frontend::RegisteredOperand>,
        residual: impl FnOnce() -> &'a ast::Expr,
    ) -> String {
        if let Some(operand) = prepared {
            self.prepare_checked_value(&operand.value);
            self.frame.prepared_operand(operand, &self.sites, false)
        } else {
            self.expr(residual())
        }
    }

    fn class_arguments(&mut self, class: &ast::StmtClassDef) -> (String, String) {
        let header = &self.source.class(class).header;
        let mut pack = crate::llvm::registered::ArgumentPack::new(self, &header.layout, None);
        for (index, operand) in header.operands.iter().enumerate() {
            let slot = pack.prepare(self, index);
            let value = self.class_operand(operand.as_ref(), || {
                crate::frontend::registered_argument_at(
                    class.arguments.as_deref().expect("class argument"),
                    index,
                )
            });
            pack.insert(self, slot, &value);
        }
        pack.finish(self)
    }

    fn compile_class_namespace(&mut self, class: &ast::StmtClassDef) -> CompiledDefinition {
        self.compile_definition(
            class
                .decorator_list
                .first()
                .map_or(class.start(), Ranged::start),
            DefinitionName::Binding(&class.name),
            None,
            None,
            None,
            Body::Namespace(&class.body),
        )
    }

    pub(super) fn parameterized_class(&mut self, class: &ast::StmtClassDef) {
        let header = &self.source.class(class).header;
        let parameters = header
            .parameters
            .as_deref()
            .expect("generic class declaration");
        let range = self.class_range(class);
        self.checkpoint(&class.name);
        self.frame.set_source_range(Some(range));
        let frame = self.scope.clone();
        let outer = self.op("annotation_scope", &[&frame]);
        let scope = self.frame.checked_parameter_scope(&frame, parameters, true);
        self.scope = self.op("class_base_scope", &[&scope, &outer]);
        self.frame.set_source_range(None);
        let args = self.class_arguments(class);
        self.scope = scope;
        let definition = self.compile_class_namespace(class);
        let result = definition.instantiate_class(&mut self.frame, args, Some(range));
        self.end(&format!("ret i64 {result}"));
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
        if self.prepare_ordinary_function(v.start()) {
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
        let compiled = self.compile_definition(start, name, params, function, type_params, body);
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
        let (name, global, generator) = match name {
            DefinitionName::Binding(identifier) => (
                self.source.private_names.definition(identifier),
                self.declared_globals.contains(identifier.as_str()),
                None,
            ),
            DefinitionName::Plain(name) | DefinitionName::TypeAlias(name) => (name, false, None),
            DefinitionName::Generator(site) => ("<genexpr>", false, Some(site)),
        };
        let qualname = if global {
            name.to_owned()
        } else {
            format!("{}{name}", self.prefix)
        };
        // A generic function's immediate lexical parent is its type-parameter scope,
        // even when lowering does not materialize a separate parameter frame.
        let generic = function.is_some_and(|f| f.type_params.is_some());
        let nested = self.in_function || generic;
        let class_header = match &body {
            Body::ClassParameters(class) => Some(self.class_range(class)),
            _ => None,
        };
        let prefix = if let Body::ClassParameters(class) = &body {
            if self.declared_globals.contains(class.name.as_str()) {
                String::new()
            } else {
                self.prefix.clone()
            }
        } else if matches!(body, Body::Namespace(..)) || generator.is_some() {
            // Nested code in a comprehension uses its name without a <locals> segment.
            format!("{qualname}.")
        } else {
            format!("{qualname}.<locals>.")
        };
        let named_scope = function.map(|def| self.source.scopes.function(def));
        let statements = match &body {
            Body::Function(stmts, _) => Some(*stmts),
            _ => None,
        };
        let declaration = function
            .and_then(|def| self.source.checked_function_header(def.start()))
            .map(|header| match header {
                CheckedHeader::Declaration {
                    docstring,
                    integer_sign_parameters,
                    ..
                } => (docstring, integer_sign_parameters),
                CheckedHeader::Ordinary => {
                    unreachable!("ordinary header uses its callable metadata")
                }
            });
        let doc = if !self.output.keep_docstrings {
            None
        } else if let Some((docstring, _)) = declaration {
            docstring
                .clone()
                .or_else(|| self.source.bundled_docstring(&qualname))
        } else {
            statements.and_then(|stmts| self.source.docstring(&qualname, stmts))
        };
        let generator_flags = match (&body, generator) {
            (Body::Function(_, flags), _) => *flags,
            (_, Some(site)) if self.source.generators[&site].is_async => {
                code_flags::ASYNC_GENERATOR
            }
            (_, Some(_)) => code_flags::GENERATOR,
            _ => named_scope.map_or(0, |scope| scope.suspension_flags),
        };
        let flags = match &body {
            Body::Namespace(..) => None,
            _ => {
                let mut flags = self.source.function_flags(
                    params,
                    nested,
                    self.class_locals.is_some()
                        && self.annotation_values.is_none()
                        && class_header.is_none()
                        && !generic,
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
                .as_ref()
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
                self.frame.output,
                String::new(),
                Body::AnnotationProvider(&annotations.body, &annotations.entries, scope),
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
            Body::Namespace(_) => {
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
            if (callable || generator.is_some())
                && self.class_locals.is_some()
                && self.annotation_values.is_none()
                && type_params.is_none()
            {
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
        }
        let mut default_names = Vec::new();
        let in_function = self.in_function || !matches!(body, Body::Namespace(..));
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
        let code =
            self.source
                .definition(self.frame.output, prefix, body, start, in_function, entry);
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
                        && declaration.map_or_else(
                            || {
                                statements.is_some_and(|body| {
                                    crate::frontend::only_compares_zero(body, name)
                                })
                            },
                            |(_, names)| names.iter().any(|p| p == name),
                        )
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

    pub(in crate::dynamic::emit) fn finish_class_annotations(
        &mut self,
        plan: &crate::frontend::ClassAnnotations,
        globals: &std::collections::BTreeSet<String>,
    ) {
        let Some(first) = plan.sites.first() else {
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
        let code =
            self.source
                .annotation_body(self.frame.output, &plan.sites, Some(globals), first.site);
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

    pub(super) fn unprepared_annotation(&mut self, occurrence: crate::ir::AnnotationOccurrence) {
        let decl = &self.source.annotations[&occurrence.site];
        let crate::frontend::AnnotationValue::Source(expression) = &decl.value else {
            unreachable!("checked annotation outside its prepared run")
        };
        let target = self
            .frame
            .annotation_target(occurrence.index, &decl.name, decl.range);
        let value = if let ast::Expr::Starred(starred) = expression {
            let value = self.expr(&starred.value);
            let count = self.integer(1);
            let items = self.op("unpack", &[&value, &count, "0"]);
            let index = self.integer(0);
            self.op("get", &[&items, &index])
        } else {
            self.expr(expression)
        };
        self.frame.store_annotation(target, &value);
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

    fn place(&mut self, target: &ast::Expr) -> Option<Place> {
        Some(match target {
            ast::Expr::Name(n) => Place::Name(self.code_name(n.id.as_str())),
            ast::Expr::Subscript(s) => {
                let obj = self.expr(&s.value);
                Place::Item(obj, self.expr(&s.slice))
            }
            _ => {
                self.unsupported("assignment target");
                return None;
            }
        })
    }

    fn assign_target(&mut self, target: &ast::Expr, value: &str) {
        let elements = match target {
            ast::Expr::Tuple(t) => &t.elts,
            ast::Expr::List(l) => &l.elts,
            ast::Expr::Attribute(field) => {
                self.store_attribute(field.attr.as_str(), field.range(), value, |code| {
                    code.expr(&field.value)
                });
                return;
            }
            _ => {
                if let Some(p) = self.place(target) {
                    self.write(&p, value);
                }
                return;
            }
        };
        if let Some([left, right]) = Self::pair_target(target) {
            self.max_live = self.max_live.max(2);
            self.checked(&format!(
                "call i64 @imre_dyn_unpack_pair(ptr %ctx, i64 {value}, ptr %live)"
            ));
            self.assign_pair_slots(left, right);
            return;
        }
        let count = self.integer(elements.len());
        let star = elements
            .iter()
            .position(|v| matches!(v, ast::Expr::Starred(_)))
            .map_or_else(|| "0".into(), |i| self.integer(i));
        let sequence = self.op("unpack_read", &[value, &count, &star]);
        for (index, target) in elements.iter().enumerate() {
            let i = self.integer(index);
            let item = self.op("get", &[&sequence, &i]);
            let target = if let ast::Expr::Starred(s) = target {
                &s.value
            } else {
                target
            };
            self.assign_target(target, &item);
        }
    }

    fn pair_target(target: &ast::Expr) -> Option<[&ast::Expr; 2]> {
        let elements = match target {
            ast::Expr::Tuple(t) => &t.elts,
            ast::Expr::List(l) => &l.elts,
            _ => return None,
        };
        let [left, right] = elements.as_slice() else {
            return None;
        };
        (!left.is_starred_expr() && !right.is_starred_expr()).then_some([left, right])
    }

    fn assign_pair_slots(&mut self, left: &ast::Expr, right: &ast::Expr) {
        // Target expressions may make calls that reuse the scratch slots.
        let first = self.value("load i64, ptr %live");
        let second_slot = self.value("getelementptr i64, ptr %live, i64 1");
        let second = self.value(&format!("load i64, ptr {second_slot}"));
        self.assign_target(left, &first);
        self.assign_target(right, &second);
    }

    fn physical_binding(&mut self, target: &'p crate::frontend::PhysicalBinding, value: &str) {
        use crate::frontend::PhysicalBinding;
        match target {
            PhysicalBinding::Checked { entered, binding } => {
                self.prepare_checked_sites(&binding.function.body);
                self.frame
                    .checked_binding(binding, *entered, value, &self.sites);
            }
            PhysicalBinding::Refused(range, reason) => {
                self.with_expression_range(*range, |code| code.build_diagnostic(reason));
            }
        }
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
                code.frame.iteration_edge(
                    &cursor,
                    region.pull,
                    region.range.start(),
                    region.otherwise.is_some(),
                    item,
                    exhausted,
                )
            },
            |code, edges, pulled| {
                let item = code.frame.iteration_item(&cursor, pulled);
                code.physical_binding(&region.target, &item);
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

    fn physical_suite(&mut self, suite: &'p crate::frontend::PhysicalSuite) {
        self.emit_suite(Some(&suite.plan), &suite.sources);
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
