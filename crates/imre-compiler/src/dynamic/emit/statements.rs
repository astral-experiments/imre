use crate::frontend::function_scope::ClassBindings;
mod declarations;
use super::sites::{CompiledDefinition, CompiledProvider};

use super::FrameEmitter;
use crate::ir::{ItemUpdate, IteratorOp, Statement};
use crate::llvm::registered::Place;
use crate::llvm::registered::context::{ContextEmitter, ContextRegion};
use imre_ast::code_flags;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};
use serde_json::json;

use super::{Body, Cleanup, DefinitionName, Entry, FinallySuite, Function, LoopTarget};

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
        prepared: &crate::frontend::RegisteredSuite,
        emit_source: impl FnMut(&mut Self, &crate::frontend::SourceRun),
    ) {
        if prepared.fully_checked() {
            self.prepare_checked_sites(&prepared.checked.function.body);
            super::RegisteredFrame::checked_body(&mut self.frame, &prepared.checked, &self.sites);
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
        prepared: &crate::frontend::RegisteredSuite,
        mut emit_source: impl FnMut(&mut Self, &crate::frontend::SourceRun),
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

    pub(super) fn statements(&mut self, suite: &'p [ast::Stmt]) {
        if let Some(plan) = suite
            .first()
            .and_then(|s| self.source.nested_bodies.get(&s.start()))
        {
            self.checked_runs(plan, |code, run| code.source_run(suite, run));
        } else {
            self.source_statements(suite);
        }
    }

    pub(super) fn source_run(&mut self, suite: &'p [ast::Stmt], run: &crate::frontend::SourceRun) {
        let statements = &suite[run.statements.clone()];
        if let Some(slot) = run.assignment_input {
            if self.start_statement() {
                let assignment = statements[0].as_assign_stmt().expect("assignment RHS");
                let value = self.expr(&assignment.value);
                self.frame.store_private(slot, &value);
            }
        } else {
            self.source_statements(statements);
        }
    }

    /// Supply only the source operands requested by the registered matcher.
    fn emit_match_inputs(&mut self, site: &crate::frontend::SourceMatch, source: &'p ast::StmtMatch) {
        use crate::llvm::registered::patterns::{MatchRegion, MatchSubject};

        let mark = self.temporary_mark();
        let subject = match &site.subject {
            Some(expr) => {
                self.prepare_checked_value(expr);
                MatchSubject::Checked(expr)
            }
            None => MatchSubject::Evaluated(self.expr(&source.subject)),
        };
        let mut regions = self
            .frame
            .registered_match(subject, &mark, site, &self.sites);
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

    fn emit_value_statement(&mut self, output: crate::frontend::ValueStatement<'_>) {
        use crate::frontend::ValueStatement;
        match output {
            ValueStatement::Checked(stmt) => self.checked_statement(&stmt),
            ValueStatement::ReturnOperand(expr) | ValueStatement::DiscardOperand(expr) => {
                // Ordinary preparation already declined this operand. A suspended frame
                // can still lower the whole expression, including its yield or await.
                let handle = if self.native_suspension {
                    self.expr(expr)
                } else {
                    self.with_expression_range(expr.range(), |code| code.located_expr(expr))
                };
                if matches!(output, ValueStatement::ReturnOperand(_)) {
                    self.return_from_scope(handle);
                } else {
                    self.discard_handle(&handle);
                }
            }
            ValueStatement::RaiseOperands(exception, cause) => {
                self.raise_operands(exception, cause, Self::expr);
            }
            ValueStatement::Ignore => {}
        }
    }

    pub(super) fn source_statements(&mut self, suite: &'p [ast::Stmt]) {
        for stmt in suite {
            if !self.start_statement() {
                break;
            }
            let position = stmt
                .as_assert_stmt()
                .map_or_else(|| stmt.range(), |assertion| assertion.test.range());
            self.checkpoint(&position);
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
            if let Some(output) = crate::frontend::value_statement(
                stmt,
                self.source.registered_source(),
                &self.source.logicals,
            ) {
                self.emit_value_statement(output);
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
            if let Some(effect) = crate::frontend::attribute_write(stmt) {
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
                ast::Stmt::TypeAlias(v) if self.class_locals.is_none() => {
                    let alias = &self.source.type_aliases[&v.start()];
                    debug_assert!(alias.body.is_none());
                    let compiled =
                        self.compile_type_alias(v.start(), super::AliasValue::Residual(&v.value));
                    let value = compiled.instantiate(&mut self.frame);
                    self.bind(&alias.name, &value);
                }
                ast::Stmt::Assign(v) => {
                    let value = self.expr(&v.value);
                    let chained = v.targets.len() > 1;
                    if chained {
                        self.live.push(value.clone());
                    }
                    for (index, target) in v.targets.iter().enumerate() {
                        if chained && index + 1 == v.targets.len() {
                            self.live.pop();
                        }
                        let owned = chained
                            || matches!(
                                target,
                                ast::Expr::Name(_) | ast::Expr::List(_) | ast::Expr::Tuple(_)
                            );
                        self.assign_target(target, &value, owned);
                    }
                }
                ast::Stmt::Delete(v) => {
                    for target in crate::frontend::delete_targets(v) {
                        if let ast::Expr::Name(name) = target {
                            self.frame.delete_name(name.id.as_str(), name.range());
                            continue;
                        }
                        if let ast::Expr::Attribute(field) = target {
                            self.attribute_operation(
                                crate::ir::AttributeAction::Delete,
                                field.attr.as_str(),
                                field.range(),
                                std::iter::once(field.value.as_ref()),
                                |code, expr| code.expr(expr),
                            );
                            continue;
                        }
                        match self.place(target) {
                            Some(Place::Item(obj, key)) => {
                                self.delete_item(&obj, &key);
                            }
                            _ => {
                                self.unsupported("deletion target");
                            }
                        }
                    }
                }
                ast::Stmt::AnnAssign(v) => {
                    if let Some(value) = &v.value {
                        let value = self.expr(value);
                        self.assign_target(
                            &v.target,
                            &value,
                            matches!(&*v.target, ast::Expr::Name(_)),
                        );
                    } else {
                        match &*v.target {
                            ast::Expr::Attribute(attr) => {
                                self.discard_annotation_operand(&attr.value);
                            }
                            ast::Expr::Subscript(subscript) => {
                                self.discard_annotation_operand(&subscript.value);
                                self.discard_annotation_slice(&subscript.slice);
                            }
                            _ => {}
                        }
                    }
                    if (self.module || self.class_locals.is_some())
                        && let Some(marker) =
                            self.source.namespace_operations.get(&v.target.range())
                    {
                        self.registered_value(marker);
                    }
                }
                ast::Stmt::AugAssign(v) => {
                    if let Some((receiver, key)) = self.checked_index_target(&v.target) {
                        if let Some(rhs) = crate::frontend::registered_value(
                            &v.value,
                            self.source.registered_source(),
                        ) {
                            self.checked_statement(&Statement::AugmentItem(
                                receiver,
                                key,
                                ItemUpdate::Protocol(v.op.as_str()),
                                rhs,
                            ));
                            continue;
                        }
                        let receiver = self.registered_value(&receiver);
                        let key = self.registered_value(&key);
                        self.update_item(receiver, key, v.op.as_str(), |code| code.expr(&v.value));
                        continue;
                    }
                    if let Some(target) = self.place(&v.target) {
                        self.update_place(&target, v.op.as_str(), |code| code.expr(&v.value));
                    }
                }
                ast::Stmt::FunctionDef(v) => {
                    let decorators = v
                        .decorator_list
                        .iter()
                        .map(|d| self.expr(&d.expression))
                        .collect::<Vec<_>>();
                    let function = self.function_definition(v);
                    let function = self.apply_decorators(&decorators, &v.decorator_list, function);
                    self.bind(v.name.as_str(), &function);
                }
                ast::Stmt::If(v) => self.conditional_fragments(v),
                ast::Stmt::For(v) => {
                    let iter = self.iteration_result(|code| {
                        let value = code.expr(&v.iter);
                        if code.native_suspension {
                            code.call_position(v.iter.range());
                        }
                        let operation = if v.is_async {
                            IteratorOp::Async(v.iter.range())
                        } else {
                            IteratorOp::Loop
                        };
                        code.op(operation.runtime_op(), &[&value])
                    });
                    self.loop_body(&v.body, &v.orelse, Some(&iter), |f, yes, no| {
                        f.frame.retained_loop_header(v.start());
                        f.enter_iteration(
                            &iter,
                            &v.target,
                            v.is_async.then(|| v.iter.range()),
                            !v.orelse.is_empty(),
                            yes,
                            no,
                        );
                    });
                }
                ast::Stmt::While(v) => {
                    if self.native_suspension
                        && matches!(&*v.test, ast::Expr::BooleanLiteral(lit) if !lit.value)
                    {
                        self.statements(&v.orelse);
                        continue;
                    }
                    self.loop_body(&v.body, &v.orelse, None, |f, yes, no| {
                        f.branch_expr(&v.test, yes, no);
                        f.label(yes);
                    });
                }
                ast::Stmt::Break(_) | ast::Stmt::Continue(_) => {
                    if let Some(target) = self.loops.last() {
                        let (dest, cleanup_depth) = if matches!(stmt, ast::Stmt::Break(_)) {
                            target.done.clone()
                        } else {
                            target.step.clone()
                        };
                        self.run_cleanups(cleanup_depth, None);
                        self.jump(&dest);
                    } else {
                        self.unsupported("loop control outside a loop");
                    }
                }
                ast::Stmt::Try(v) => self.protected_statement(v),
                ast::Stmt::With(v) if !v.is_async || self.native_suspension => {
                    self.emit_with_inputs(v);
                }
                ast::Stmt::Assert(v) => {
                    let pass = self.name("b");
                    let fail = self.name("b");
                    self.branch_expr(&v.test, &pass, &fail);
                    self.label(&fail);
                    let message = v.msg.as_deref().map(|expr| self.expr(expr));
                    self.frame
                        .assertion_failure(v.start(), v.test.start(), message.as_deref());
                    self.label(&pass);
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

    fn method_type_parameters<'a>(
        &self,
        function: &'a ast::StmtFunctionDef,
    ) -> Option<&'a ast::TypeParams> {
        function.type_params.as_deref().filter(|_| {
            self.class_locals.is_some()
                && self
                    .source
                    .namespace_operations
                    .contains_key(&function.name.range())
        })
    }

    fn function_definition(&mut self, v: &ast::StmtFunctionDef) -> String {
        // A method's lazy annotations also need its defining class namespace.
        let type_params = if self.class_locals.is_some() && !self.source.string_annotations {
            self.method_type_parameters(v)
        } else {
            crate::frontend::function_scope::boxed_function_type_params(
                v,
                self.source.string_annotations,
                !self.prefix.is_empty(),
                Some(&self.source.private_names),
            )
        };
        self.define(
            v.decorator_list.first().map_or(v.start(), Ranged::start),
            DefinitionName::Binding(&v.name),
            Some(&v.parameters),
            Some(v),
            type_params,
            self.source.function_body(v),
        )
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
        let (name, global) = match name {
            DefinitionName::Binding(identifier) => (
                self.source.private_names.definition(identifier),
                self.declared_globals.contains(identifier.as_str()),
            ),
            DefinitionName::Plain(name) => (name, false),
        };
        let qualname = if global {
            name.to_owned()
        } else {
            format!("{}{name}", self.prefix)
        };
        let returns = function.and_then(|f| f.returns.as_deref());
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
        } else if matches!(body, Body::Namespace(..)) {
            format!("{qualname}.")
        } else {
            format!("{qualname}.<locals>.")
        };
        let statements = match &body {
            Body::Statements(stmts) | Body::GeneratorFunction(stmts, _) => Some(*stmts),
            _ => None,
        };
        let doc = statements
            .filter(|_| self.output.keep_docstrings)
            .and_then(|stmts| self.source.docstring(&qualname, stmts));
        let generator_flags = body.generator_flags();
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
        let entries = params.map_or_else(Vec::new, |p| {
            crate::frontend::parameter_annotations(p, returns)
        });
        let provider = if entries.is_empty() {
            None
        } else {
            let class = self
                .class_locals
                .as_ref()
                .filter(|_| !self.source.string_annotations)
                .cloned();
            let reads = self.source.scopes.annotations.parameter_reads(
                params.expect("annotated function parameters").start(),
                class.as_ref(),
            );
            let mut metadata = json!({
                "posonly": ["format"],
                "code_flags": self.source.function_flags(None, nested, false),
                "scope_reads": reads,
                "scope_complete": true,
                "scope_locals_complete": true,
                "annotation_class": class.is_some(),
            });
            if !self.source.python_annotations && type_params.is_none() {
                for (slot, expr) in &entries {
                    if let Some(reference) = scalar_reference(expr) {
                        let root = reference.split('.').next().unwrap();
                        if class
                            .as_ref()
                            .is_some_and(|class| class.globals.contains(root))
                        {
                            continue;
                        }
                        metadata["annotation_references"][slot] = json!(reference);
                    }
                }
            }
            let code = self.source.definition(
                self.frame.output,
                String::new(),
                Body::Annotations(entries, class),
                params.map_or(start, Ranged::start),
                false,
                Entry::Compiled,
            );
            Some(CompiledProvider {
                code,
                signature: metadata.to_string(),
            })
        };
        let mut signature = json!({});
        if let Some(flags) = flags {
            signature["code_flags"] = json!(flags);
        }
        if body.is_genexpr() {
            // generator() binds the implicit .0 argument without calling the step function.
            signature["positional"] = json!([".0"]);
        }
        let entry = match generator_flags {
            0 => Entry::Compiled,
            _ => Entry::Suspended,
        };
        let scopes = &self.source.scopes;
        let scope_facts = match &body {
            Body::Statements(_) | Body::GeneratorFunction(..) => {
                Some(&scopes.functions[&start].lexical)
            }
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
            Body::AliasEvaluator(_) => Some(&scopes.aliases[&start]),
            Body::CheckedGenerator(site, _) => Some(&scopes.generators[site]),
            _ => None,
        };
        if let Some(facts) = scope_facts {
            facts.write_signature(&mut signature);
            let callable = matches!(body, Body::Statements(_) | Body::GeneratorFunction(..));
            if callable && facts.captures_class {
                signature["class_cell"] = json!(true);
            }
            if (callable || body.is_genexpr())
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
        let type_alias = matches!(body, Body::AliasEvaluator(_));
        if type_alias {
            signature["posonly"] = json!([".format"]);
        }
        let mut default_names = Vec::new();
        let in_function = self.in_function || !matches!(body, Body::Namespace(..));
        // A generator call returns a suspended frame, before it yields or returns a value.
        let scalar = |expr: &ast::Expr| {
            if self.source.python_annotations {
                return None;
            }
            scalar_reference(expr).filter(|name| {
                !type_params.is_some_and(|params| params.iter().any(|p| p.name().as_str() == name))
            })
        };
        if matches!(entry, Entry::Compiled)
            && let Some(ty) = returns.and_then(scalar)
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
            for (group, args) in [
                ("posonly", &p.posonlyargs),
                ("positional", &p.args),
                ("kwonly", &p.kwonlyargs),
            ] {
                let mut names = Vec::with_capacity(args.len());
                for arg in args {
                    let name = arg.parameter.name.as_str();
                    if let Some(ty) = arg.parameter.annotation.as_deref().and_then(scalar) {
                        signature["types"][name] = json!(ty);
                        if ty == "float"
                            && statements
                                .is_some_and(|body| crate::frontend::only_compares_zero(body, name))
                        {
                            signature["integer_sign_parameters"][name] = json!(true);
                        }
                    }
                    names.push(name);
                    if arg.default.is_some() {
                        default_names.push(name.to_owned());
                    }
                }
                if !names.is_empty() {
                    signature[group] = json!(names);
                }
            }
            for (key, parameter) in [("varargs", &p.vararg), ("kwargs", &p.kwarg)] {
                if let Some(parameter) = parameter {
                    signature[key] = json!(parameter.name.as_str());
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

    /// Publish a module's provider before its first statement. Every simple
    /// annotation, including an unconditional one, becomes visible only after
    /// execution reaches its assignment.
    pub(super) fn setup_deferred_module_annotations(&mut self, sites: &[TextSize]) {
        if !sites.is_empty() {
            let entries: Vec<_> = sites
                .iter()
                .enumerate()
                .map(|(index, site)| crate::ir::AnnotationOccurrence {
                    site: *site,
                    index: Some(index),
                })
                .collect();
            let signature = self.frame.text(
                &json!({
                    "posonly": ["format"], "scope_complete": true, "scope_locals_complete": true,
                    "code_flags": self.source.function_flags(None, false, false),
                })
                .to_string(),
            );
            let code = self.annotation_body(&entries, None, self.source.module_start);
            let name = self.text("__annotate__");
            let provider = self.frame.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {signature}, i64 0, i64 {name}, i64 0, i64 0, i1 true)", self.scope));
            self.bind("__annotate__", &provider);
        }
        self.frame
            .op("setup_module_annotations", &[&self.scope.clone()]);
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
        let code = self.annotation_body(&plan.sites, Some(globals), first.site);
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

    fn annotation_body(
        &mut self,
        entries: &[crate::ir::AnnotationOccurrence],
        globals: Option<&std::collections::BTreeSet<String>>,
        start: TextSize,
    ) -> String {
        let plan =
            crate::frontend::DeferredAnnotation::prepare_body(&self.source.annotations, entries);
        self.source.definition(
            self.frame.output,
            String::new(),
            Body::NamespaceAnnotations(&plan, entries, globals),
            start,
            false,
            Entry::Compiled,
        )
    }

    pub(super) fn unprepared_annotation(&mut self, occurrence: crate::ir::AnnotationOccurrence) {
        let decl = &self.source.annotations[&occurrence.site];
        let crate::frontend::AnnotationValue::Source(expression) = &decl.value else {
            unreachable!("checked annotation outside its prepared run")
        };
        let target = self
            .frame
            .annotation_target(occurrence.index, &decl.name, decl.range);
        self.place_annotation_string(expression.start());
        let value = self.annotation_value(expression);
        self.frame.store_annotation(target, &value);
    }

    pub(super) fn annotations(
        &mut self,
        entries: &[(&str, &ast::Expr)],
        class: Option<ClassBindings>,
    ) {
        self.begin_annotations();
        self.frame.class_locals = class;
        for &(name, expression) in entries {
            let target = self.frame.annotation_target(None, name, expression.range());
            let value = self.annotation_value(expression);
            self.frame.store_annotation(target, &value);
        }
        let values = self.take_annotation_result();
        self.return_value(&values);
    }

    fn annotation_value(&mut self, expression: &ast::Expr) -> String {
        match self.source.annotation_operands.get(&expression.start()) {
            Some(crate::frontend::PreparedAnnotation::String { .. }) => {
                self.annotation_string(expression.start())
            }
            Some(crate::frontend::PreparedAnnotation::Value(value)) => self.registered_value(value),
            None => {
                if let ast::Expr::Starred(starred) = expression {
                    let value = self.expr(&starred.value);
                    let count = self.integer(1);
                    let items = self.op("unpack", &[&value, &count, "0"]);
                    let index = self.integer(0);
                    self.op("get", &[&items, &index])
                } else {
                    self.expr(expression)
                }
            }
        }
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

    fn discard_annotation_operand(&mut self, expr: &ast::Expr) {
        let mark = self.temporary_mark();
        self.expr(expr);
        self.discard_truth(&mark);
    }

    // Without an assignment, CPython evaluates each subscript operand but
    // neither constructs an extended slice nor invokes __getitem__.
    fn discard_annotation_slice(&mut self, slice: &ast::Expr) {
        match slice {
            ast::Expr::Slice(s) => {
                for bound in [&s.lower, &s.upper, &s.step].into_iter().flatten() {
                    self.discard_annotation_operand(bound);
                }
            }
            ast::Expr::Tuple(t) => {
                for item in &t.elts {
                    self.discard_annotation_slice(item);
                }
            }
            expr => self.discard_annotation_operand(expr),
        }
    }

    pub(super) fn index_operand(&mut self, key: &ast::Expr) -> String {
        if let Some(key) =
            crate::frontend::registered_index_key(key, self.source.registered_source())
        {
            self.registered_value(&key)
        } else {
            self.expr(key)
        }
    }

    fn checked_index_target(
        &self,
        target: &ast::Expr,
    ) -> Option<(crate::ir::Expr, crate::ir::Expr)> {
        crate::frontend::registered_index_target(target, self.source.registered_source())
    }

    fn place(&mut self, target: &ast::Expr) -> Option<Place> {
        Some(match target {
            ast::Expr::Name(n) => Place::Name(self.code_name(n.id.as_str())),
            ast::Expr::Subscript(s) => {
                let obj = self.expr(&s.value);
                Place::Item(obj, self.index_operand(&s.slice))
            }
            _ => {
                self.unsupported("assignment target");
                return None;
            }
        })
    }

    pub(super) fn assign(&mut self, target: &ast::Expr, value: &str) {
        self.assign_target(target, value, false);
    }

    /// Transfer unpacked items to their targets while retaining the outstanding operands.
    fn assign_target(&mut self, target: &ast::Expr, value: &str, owned: bool) {
        let elements = match target {
            ast::Expr::Tuple(t) => &t.elts,
            ast::Expr::List(l) => &l.elts,
            ast::Expr::Attribute(field) => {
                self.store_attribute(field.attr.as_str(), field.range(), value, |code| {
                    code.expr(&field.value)
                });
                if owned {
                    self.release_target(None);
                }
                return;
            }
            _ => {
                if let Some(p) = self.place(target) {
                    if owned {
                        if let Place::Name(name) = &p {
                            let scope = self.scope.clone();
                            self.assign_owned_name(&scope, name, value, None);
                        } else {
                            self.write(&p, value);
                            // An item setter may replace a writer.
                            self.release_target(None);
                        }
                    } else {
                        self.write(&p, value);
                    }
                }
                return;
            }
        };
        if !owned && let Some([left, right]) = Self::pair_target(target) {
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
        let operation = if owned { "unpack" } else { "unpack_read" };
        let sequence = self.op(operation, &[value, &count, &star]);
        if owned {
            self.live.push(sequence.clone());
        }
        for (index, target) in elements.iter().enumerate() {
            let i = self.integer(index);
            let item = self.op(if owned { "unpack_take" } else { "get" }, &[&sequence, &i]);
            let target = if let ast::Expr::Starred(s) = target {
                &s.value
            } else {
                target
            };
            self.assign_target(target, &item, owned);
        }
        if owned {
            self.live.pop();
        }
    }

    /// Enter the loop body with its target bound, or branch to the exhaustion successor.
    pub(super) fn enter_iteration(
        &mut self,
        iter: &str,
        target: &ast::Expr,
        async_range: Option<ruff_text_size::TextRange>,
        has_else: bool,
        body: &str,
        exhausted: &str,
    ) {
        let item = if let Some(range) = async_range {
            let item = self.frame.async_pull(
                iter,
                crate::ir::AsyncPull {
                    range,
                    header: None,
                },
                body,
                exhausted,
            );
            self.label(body);
            item
        } else {
            let pair = Self::pair_target(target);
            self.frame
                .enter_sync_iteration(iter, has_else, pair.is_some(), body, exhausted);
            if let Some([left, right]) = pair {
                self.assign_pair_slots(left, right);
                return;
            }
            self.op("next", &[iter])
        };
        self.assign(target, &item);
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
        self.assign(left, &first);
        self.assign(right, &second);
    }

    /// Return whether control can reach a subsequent arm.
    fn conditional_fragments(&mut self, stmt: &'p ast::StmtIf) {
        use crate::frontend::SourceGuard;
        use crate::llvm::operations::Choice;
        let guards = &self.source.statement_sites.ifs[&stmt.range()];
        let fragments = std::iter::once((Some(stmt.test.as_ref()), stmt.body.as_slice())).chain(
            stmt.elif_else_clauses
                .iter()
                .map(|part| (part.test.as_ref(), part.body.as_slice())),
        );
        let mut choice = Choice::new(&mut self.frame.body);
        for (guard, (test, body)) in guards.iter().zip(fragments) {
            let next = match guard {
                SourceGuard::Otherwise => None,
                SourceGuard::Literal(taken) if self.native_suspension => {
                    if !taken {
                        continue;
                    }
                    None
                }
                guard => {
                    let (yes, no) = Choice::successors(&mut self.frame.body);
                    match guard {
                        SourceGuard::Literal(taken) => {
                            self.branch(if *taken { "true" } else { "false" }, &yes, &no)
                        }
                        SourceGuard::Checked(value) => {
                            self.prepare_checked_value(value);
                            self.frame.registered_branch(value, &self.sites, &yes, &no);
                        }
                        SourceGuard::Residual => {
                            self.branch_expr(test.expect("residual if guard"), &yes, &no)
                        }
                        SourceGuard::Otherwise => unreachable!(),
                    }
                    self.label(&yes);
                    Some(no)
                }
            };
            self.statements(body);
            self.flush_check();
            choice.finish_arm(&mut self.frame.body);
            let Some(next) = next else { break };
            self.label(&next);
        }
        // A residual suite can contain a suspension that resumes in a later block.
        self.flush_check();
        choice.finish_arm(&mut self.frame.body);
        choice.finish(&mut self.frame.body, true);
    }

    fn loop_body(
        &mut self,
        body: &'p [ast::Stmt],
        otherwise: &'p [ast::Stmt],
        iterator: Option<&str>,
        enter_body: impl FnOnce(&mut Self, &str, &str),
    ) {
        let step = self.name("b");
        let body_label = self.name("b");
        let exhausted = self.name("b");
        let done = self.name("b");
        let done_depth = self.exits.len();
        let cursor_unwind = iterator.map(|cursor| {
            // Continue retains the cursor. Every transfer out of the loop releases its owner.
            self.live.push(cursor.to_owned());
            self.exits.push((
                Cleanup::RetainedValue {
                    value: cursor.to_owned(),
                    enclosing_exception: None,
                },
                self.body.error_target.clone(),
            ));
            self.begin_operand_scope()
        });
        self.jump(&step);
        self.label(&step);
        enter_body(self, &body_label, &exhausted);
        self.loops.push(LoopTarget {
            step: (step.clone(), self.exits.len()),
            done: (done.clone(), done_depth),
        });
        self.statements(body);
        self.loops.pop();
        self.jump(&step);
        self.label(&exhausted);
        if let Some(edge) = cursor_unwind {
            self.exits.pop();
            self.live.pop();
            self.finish_unwind(edge, |frame| frame.release_target(None));
            self.release_target(None);
        }
        self.statements(otherwise);
        self.jump(&done);
        self.label(&done);
    }

    fn protected_suite(&mut self, stmt: &'p ast::StmtTry) {
        let (handler, enclosing) = self.begin_operand_scope();
        self.statements(&stmt.body);
        self.body.error_target = enclosing;
        self.statements(&stmt.orelse);
        let (exception, done) = self.enter_handlers(&handler);
        if stmt.is_star {
            crate::llvm::registered::handlers::groups(
                self,
                &stmt.handlers,
                &exception,
                &done,
                |code, ast::ExceptHandler::ExceptHandler(handler)| {
                    let matcher = handler.type_.as_deref().expect("except* requires a type");
                    code.checkpoint(handler);
                    (
                        code.expr(matcher),
                        handler.name.as_ref().map(|name| name.as_str()),
                        &handler.body,
                    )
                },
                Self::statements,
            );
        } else {
            for ast::ExceptHandler::ExceptHandler(handler) in &stmt.handlers {
                crate::llvm::registered::handlers::arm(
                    self,
                    &exception,
                    &done,
                    handler.name.as_ref().map(|name| name.as_str()),
                    |code| {
                        handler.type_.as_deref().map(|class| {
                            code.checkpoint(class);
                            code.expr(class)
                        })
                    },
                    |code, cleanup| {
                        if let Some((name, target)) = cleanup {
                            code.exits
                                .push((Cleanup::ExceptionName(name.into()), target.into()));
                        }
                        code.statements(&handler.body);
                        if cleanup.is_some() {
                            code.exits.pop();
                        }
                    },
                );
            }
            self.raise(&exception, None, true);
        }
        self.leave_handlers(&done);
    }

    fn emit_finally(&mut self, suite: FinallySuite<'p>) {
        match suite {
            FinallySuite::Checked(checked) => {
                let Some(enclosing) = self.frame.enter_checked_slots(checked.region()) else {
                    return;
                };
                if !checked.function.body.is_empty() && self.start_statement() {
                    self.checked_block(&checked.function.body, checked);
                }
                self.frame.restore_checked_slots(enclosing);
            }
            FinallySuite::Residual(source) => self.statements(source),
        }
    }

    fn protected_statement(&mut self, stmt: &'p ast::StmtTry) {
        if stmt.finalbody.is_empty() {
            self.protected_suite(stmt);
            return;
        }
        let suite = match self.source.nested_bodies.get(&stmt.finalbody[0].start()) {
            Some(plan) if plan.fully_checked() => FinallySuite::Checked(&plan.checked),
            _ => FinallySuite::Residual(&stmt.finalbody),
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
                if stmt.handlers.is_empty() {
                    code.statements(&stmt.body);
                } else {
                    code.protected_suite(stmt);
                }
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
                code.emit_finally(suite);
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
                Cleanup::With(context) => {
                    if context.is_async {
                        let slot = self
                            .live
                            .iter()
                            .rposition(|v| v == &context.callable)
                            .unwrap();
                        discarded.push((slot, self.live.remove(slot)));
                    }
                    self.exit_context(context, None);
                }
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
                    self.emit_finally(*body);
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

    fn emit_with_inputs(&mut self, source: &'p ast::StmtWith) {
        let inputs = &self.source.statement_sites.contexts[&source.range()];
        self.with_contexts(
            inputs.iter().map(|input| input.site),
            source.is_async,
            |code, i| match &inputs[i].manager {
                Some(manager) => code.with_expression_range(inputs[i].site.range, |code| {
                    code.registered_value(manager)
                }),
                None => code.expr(&source.items[i].context_expr),
            },
            |code, i, value| {
                if let Some(target) = &source.items[i].optional_vars {
                    code.assign(target, value);
                }
            },
            |code| code.statements(&source.body),
        );
    }
}

/// Names that can refer directly to a type without calling Python.
fn scalar_reference(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::NoneLiteral(_) => Some("None".to_owned()),
        ast::Expr::Name(name) if !name.id.starts_with("__") => Some(name.id.to_string()),
        ast::Expr::Attribute(attr) => {
            let module = attr.value.as_name_expr()?;
            (!module.id.starts_with("__") && !attr.attr.starts_with("__"))
                .then(|| format!("{}.{}", module.id, attr.attr))
        }
        _ => None,
    }
}
