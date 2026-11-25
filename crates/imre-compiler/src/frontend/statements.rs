//! Ordinary source statements shared by typed code and registered Python scopes.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr};

pub(super) type Block = (Vec<Statement>, bool);

/// A range loop has no pull instructions, so its consumer can supply the whole body.
pub(super) fn append_block(output: &mut Vec<Statement>, block: Vec<Statement>) {
    if output.capacity() == 0 {
        *output = block;
    } else {
        output.extend(block);
    }
}

pub(super) fn loop_transfer(stmt: &ast::Stmt) -> Option<Statement> {
    match stmt {
        ast::Stmt::Break(_) => Some(Statement::Break(Some(stmt.range()))),
        ast::Stmt::Continue(_) => Some(Statement::Continue(stmt.range())),
        _ => None,
    }
}

/// A bare raise reuses the active exception without evaluating an operand.
pub(super) fn raised<E>(
    raise: &ast::StmtRaise,
    prepare: impl FnOnce(&ast::Expr) -> StdResult<Statement, E>,
) -> StdResult<Statement, E> {
    match &raise.exc {
        Some(value) => prepare(value),
        None => Ok(Statement::Raise(None)),
    }
}

/// A normal exit from the protected region reaches its successor only if cleanup falls through.
pub(super) fn finish_try(
    output: Option<&mut Vec<Statement>>,
    body: Vec<Statement>,
    handlers: Vec<ir::ExceptHandler>,
    orelse: Vec<Statement>,
    normal_exit: bool,
    (finalbody, final_falls): Block,
) -> bool {
    if let Some(output) = output {
        output.push(Statement::Try {
            body,
            handlers,
            orelse,
            finalbody,
        });
    }
    normal_exit && final_falls
}

pub(super) fn if_clauses(
    branch: &ast::StmtIf,
) -> impl Iterator<Item = (Option<&ast::Expr>, &[ast::Stmt])> {
    std::iter::once((Some(branch.test.as_ref()), branch.body.as_slice())).chain(
        branch
            .elif_else_clauses
            .iter()
            .map(|clause| (clause.test.as_ref(), clause.body.as_slice())),
    )
}

pub(super) fn push_if(
    output: &mut Vec<Statement>,
    condition: Expr,
) -> (&mut Vec<Statement>, &mut Vec<Statement>) {
    let Statement::If(_, yes, no) =
        output.push_mut(Statement::If(condition, Vec::new(), Vec::new()))
    else {
        unreachable!()
    };
    (yes, no)
}

/// Find only a straight-line entry-suite pair whose generator has exactly one
/// adjacent consumer. Protected suites and functions are deliberately not traversed:
/// a caught failure could expose the generator binding before replacement.
fn adjacent_generator_consumer<'a>(
    suite: &'a [ast::Stmt],
    generator: TextRange,
    name: &str,
) -> Option<&'a ast::ExprListComp> {
    for pair in suite.windows(2) {
        let (ast::Stmt::Assign(first), ast::Stmt::Assign(second)) = (&pair[0], &pair[1]) else {
            continue;
        };
        if first.value.range() != generator
            || !matches!(first.targets.as_slice(), [ast::Expr::Name(n)] if n.id == name)
            || !matches!(second.targets.as_slice(), [ast::Expr::Name(n)] if n.id == name)
        {
            continue;
        }
        let ast::Expr::ListComp(display) = &*second.value else {
            continue;
        };
        let [clause] = display.generators.as_slice() else {
            continue;
        };
        if !clause.is_async && matches!(&clause.iter, ast::Expr::Name(n) if n.id == name) {
            return Some(display);
        }
    }
    for statement in suite {
        if let ast::Stmt::If(branch) = statement {
            for (_, arm) in if_clauses(branch) {
                if let Some(found) = adjacent_generator_consumer(arm, generator, name) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// The virtual binding cannot escape to callbacks, frames, or namespace lookup.
/// Limit calls to sealed scalar builtins and a literal string's immutable join
/// descriptor; lowered IR is independently checked before the consumer is accepted.
struct ClosedGeneratorExpressions<'a, 'g> {
    checker: &'a Checker<'g>,
    name: &'a str,
    reads: usize,
    valid: bool,
}

impl<'ast> Visitor<'ast> for ClosedGeneratorExpressions<'_, '_> {
    fn visit_expr(&mut self, expression: &'ast ast::Expr) {
        match expression {
            ast::Expr::Name(name) => {
                if name.id == self.name {
                    self.reads += 1;
                }
            }
            ast::Expr::Call(call) => {
                let sealed = match &*call.func {
                    ast::Expr::Name(name) => {
                        matches!(name.id.as_str(), "str" | "ord" | "len" | "range")
                            && self
                                .checker
                                .unshadowed_builtin(&call.func, name.id.as_str())
                            || self
                                .checker
                                .names
                                .get(name.id.as_str())
                                .is_some_and(|slot| {
                                    self.checker.assigned.contains(slot)
                                        && self.checker.assigned.is_range_alias(*slot)
                                        && !self.checker.live_module_name(name.id.as_str())
                                })
                    }
                    ast::Expr::Attribute(attribute) => {
                        attribute.attr.as_str() == "join"
                            && matches!(&*attribute.value, ast::Expr::StringLiteral(_))
                    }
                    _ => false,
                };
                self.valid &= sealed
                    && call.arguments.keywords.is_empty()
                    && call
                        .arguments
                        .args
                        .iter()
                        .all(|arg| !matches!(arg, ast::Expr::Starred(_)));
                self.visit_arguments(&call.arguments);
            }
            ast::Expr::BoolOp(_)
            | ast::Expr::BinOp(_)
            | ast::Expr::UnaryOp(_)
            | ast::Expr::If(_)
            | ast::Expr::ListComp(_)
            | ast::Expr::Generator(_)
            | ast::Expr::Compare(_)
            | ast::Expr::StringLiteral(_)
            | ast::Expr::BytesLiteral(_)
            | ast::Expr::NumberLiteral(_)
            | ast::Expr::BooleanLiteral(_)
            | ast::Expr::NoneLiteral(_)
            | ast::Expr::Subscript(_)
            | ast::Expr::Tuple(_)
            | ast::Expr::Slice(_) => walk_expr(self, expression),
            _ => self.valid = false,
        }
    }
}

/// A native operation may not observe the eliminated generator binding. Python
/// handles, source calls, overloaded protocols, and namespace operations remain unsupported.
fn closed_generator_ir(body: &[Statement]) -> bool {
    let mut closed = true;
    ir::walk_expressions(body, &mut |value| {
        closed &= !matches!(
            value.ty,
            Type::PythonObject
                | Type::ObjectValue
                | Type::SourceModule
                | Type::ModuleNamespace
                | Type::Class(_)
                | Type::Callable(_)
        );
        closed &= match &value.kind {
            ExprKind::Call(..)
            | ExprKind::Invoke { .. }
            | ExprKind::Construct { .. }
            | ExprKind::Callable { .. }
            | ExprKind::Field(..)
            | ExprKind::Bind(..) => false,
            ExprKind::Builtin(op, args) => match op {
                Builtin::GeneratorFrameEnter(_)
                | Builtin::GeneratorFrameLeave
                | Builtin::ActiveException
                | Builtin::Slice
                | Builtin::BytesGet
                | Builtin::StringGet
                | Builtin::Append
                | Builtin::StringJoin
                | Builtin::JoinBufferNew
                | Builtin::JoinBufferAppend
                | Builtin::JoinBufferFinish
                | Builtin::Bool
                | Builtin::Len
                | Builtin::Ord => true,
                // A statically invalid scalar operation may still be inside an
                // unselected conditional arm. Its exact builtin exception evaluates
                // prior operands in order and cannot inspect the virtual binding.
                Builtin::RaiseException => matches!(args.as_slice(), [operand, error]
                    if operand.ty == Type::Int && error.ty == Type::Exception
                        && matches!(&error.kind, ExprKind::Exception { kind, message: Some(message) }
                            if Some(*kind) == exception_kind("TypeError") && matches!(&message.kind, ExprKind::Str(_)))),
                Builtin::Str => args.first().is_some_and(|x| {
                    matches!(
                        x.ty,
                        Type::Int | Type::Float | Type::Bool | Type::Str | Type::Bytes
                    )
                }),
                _ => false,
            },
            _ => true,
        };
    });
    closed
}

pub(super) trait StatementLowering {
    type Error;

    fn assignment(
        &mut self,
        annotation: Option<&ast::StmtAnnAssign>,
        targets: &[ast::Expr],
        rhs: &ast::Expr,
        body: &mut Vec<Statement>,
    ) -> StdResult<bool, Self::Error>;
    fn branch(
        &mut self,
        branch: &ast::StmtIf,
        body: &mut Vec<Statement>,
    ) -> StdResult<bool, Self::Error>;
    fn definition(
        &mut self,
        def: &ast::StmtFunctionDef,
        body: &mut Vec<Statement>,
    ) -> StdResult<Option<Statement>, Self::Error>;
    /// The enclosing block can already own this statement's checkpoint.
    fn position_only(&mut self, _range: TextRange) -> Option<Statement> {
        None
    }
    fn effect(&mut self, value: &ast::Expr) -> StdResult<(Option<Statement>, bool), Self::Error>;
    fn returned(&mut self, ret: &ast::StmtReturn) -> StdResult<Statement, Self::Error>;
    fn raised_value(
        &mut self,
        raise: &ast::StmtRaise,
        value: &ast::Expr,
    ) -> StdResult<Statement, Self::Error>;
    fn for_loop(&mut self, loop_: &ast::StmtFor) -> StdResult<Block, Self::Error>;
    /// Record a reachable exit when this scope has an enclosing loop.
    fn record_loop_transfer(&mut self, transfer: &Statement) -> bool;
    fn other_statement(
        &mut self,
        stmt: &ast::Stmt,
        output: &mut Vec<Statement>,
    ) -> StdResult<bool, Self::Error>;

    /// Return the single completed operation. Compound lowerings write their block into `body`.
    fn source_statement(
        &mut self,
        stmt: &ast::Stmt,
        module: bool,
        body: &mut Vec<Statement>,
    ) -> StdResult<(Option<Statement>, bool), Self::Error> {
        Ok(match stmt {
            _ if !matches!(stmt, ast::Stmt::AnnAssign(a) if a.target.is_name_expr())
                && let Some((targets, rhs)) = imre_ast::assignment_parts(stmt) =>
            {
                (
                    None,
                    self.assignment(stmt.as_ann_assign_stmt(), targets, rhs, body)?,
                )
            }
            ast::Stmt::If(branch) => (None, self.branch(branch, body)?),
            ast::Stmt::FunctionDef(def) => (self.definition(def, body)?, true),
            ast::Stmt::Expr(expr) => self.effect(&expr.value)?,
            ast::Stmt::Return(ret) if !module => (Some(self.returned(ret)?), false),
            ast::Stmt::For(loop_) => {
                let (statements, continues) = self.for_loop(loop_)?;
                append_block(body, statements);
                (None, continues)
            }
            ast::Stmt::Raise(raise) => (
                Some(raised(raise, |value| self.raised_value(raise, value))?),
                false,
            ),
            ast::Stmt::Pass(_) => (self.position_only(stmt.range()), true),
            _ => match loop_transfer(stmt) {
                Some(transfer) if self.record_loop_transfer(&transfer) => (Some(transfer), false),
                _ => (None, self.other_statement(stmt, body)?),
            },
        })
    }
}

impl StatementLowering for Checker<'_> {
    type Error = Failure<String>;

    fn branch(&mut self, branch: &ast::StmtIf, body: &mut Vec<Statement>) -> Result<bool, String> {
        let start = body.len();
        let result = self.if_chain(branch, Some(body));
        if result.is_err() {
            body.truncate(start);
        } else if let Some(sites) = &mut self.fixed_sites
            && let [statement @ Statement::If(..)] = &body[start..]
        {
            sites.statement(branch.test.range(), statement);
        }
        result
    }

    fn assignment(
        &mut self,
        _annotation: Option<&ast::StmtAnnAssign>,
        targets: &[ast::Expr],
        rhs: &ast::Expr,
        body: &mut Vec<Statement>,
    ) -> Result<bool, String> {
        if _annotation.is_none()
            && self.module_scope
            && self.module == self.graph.entry
            && self.registered.is_none()
            && self.module_namespace.is_unexposed()
            && let ([ast::Expr::Name(name)], ast::Expr::Generator(generator)) = (targets, rhs)
            && self
                .names
                .get(name.id.as_str())
                .is_none_or(|slot| !self.assigned.contains(slot))
            && let Some(display) = adjacent_generator_consumer(
                &self.graph.modules[self.module].parsed.syntax().body,
                rhs.range(),
                name.id.as_str(),
            )
        {
            let mut proof = ClosedGeneratorExpressions {
                checker: self,
                name: name.id.as_str(),
                reads: 0,
                valid: true,
            };
            proof.visit_expr(rhs);
            proof.visit_expr(&ast::Expr::ListComp(display.clone()));
            if proof.valid && proof.reads == 1 {
                let consumer = display.generators[0].iter.range();
                let mut prepared = self.generator_input(generator, None, None, None)?;
                if closed_generator_ir(&prepared.evaluate) && closed_generator_ir(&prepared.next) {
                    body.append(&mut prepared.evaluate);
                    self.assigned.store_generator(consumer, prepared);
                    return Ok(true);
                }
            }
        }
        let fused = matches!(rhs, ast::Expr::ListComp(display) if display.generators.first().is_some_and(|clause| self.assigned.has_stored_generator(clause.iter.range())));
        let start = body.len();
        let builtin_range = _annotation.is_none() && self.unshadowed_builtin(rhs, "range");
        let continues = self.assignment_statement(targets, rhs, body)?;
        if fused && !closed_generator_ir(&body[start..]) {
            return Err(error(
                self.source,
                rhs,
                "a stored generator consumer can observe its binding",
            ));
        }
        if builtin_range
            && let [ast::Expr::Name(name)] = targets
            && let Some(&slot) = self.names.get(name.id.as_str())
        {
            self.assigned.mark_range_alias(slot);
        }
        if continues
            && !self.module_scope
            && self.reachable
            && let [ast::Expr::Name(name)] = targets
            && let Some(source) = self.frame_locals(rhs)
            && let Some(&slot) = self.names.get(name.id.as_str())
            && !self.declared_slots.contains_key(&slot)
        {
            self.assigned.set_value_source(slot, Some(source));
        }
        Ok(continues)
    }

    fn definition(
        &mut self,
        def: &ast::StmtFunctionDef,
        _body: &mut Vec<Statement>,
    ) -> Result<Option<Statement>, String> {
        if self.registered_local_start.is_some() {
            let header = imre_ast::function_definition_expressions(def)
                .map(|expr| self.expr(expr))
                .collect::<Result<Vec<_>, _>>()?;
            let value = registered::function_definition(def, header);
            return Ok(Some(self.bind_local_definition(&def.name, value)?));
        }
        self.define_local_function(def)
    }

    fn position_only(&mut self, range: TextRange) -> Option<Statement> {
        if self.fault_enabled() {
            return None;
        }
        let sites = self.fixed_sites.as_mut()?;
        let statement = Statement::Position(range, None);
        sites.record(range, ir::Node::Statement(&statement));
        Some(statement)
    }

    fn effect(&mut self, expr: &ast::Expr) -> Result<(Option<Statement>, bool), String> {
        if expr.is_string_literal_expr() || expr.is_ellipsis_literal_expr() {
            return Ok((self.position_only(expr.range()), true));
        }
        if self.module_scope
            && let ast::Expr::Name(name) = expr
            && !self.names.contains_key(name.id.as_str())
            && self.graph.closed_missing_module_probe(self.module, expr)
        {
            self.graph
                .checked_unbound_local_read(self.module, expr.range());
            let failure = unbound_locals::module_name_failure(name.id.as_str());
            return Ok((Some(self.statement_at(expr, failure)), false));
        }
        if let ast::Expr::Yield(yielded) = expr
            && let Some(element) = self.generator_item
            && let Some(value) = &yielded.value
        {
            let item = self.expr_expected(value, element)?;
            let resume = contexts::op(ir::ProcessOperation::GeneratorYield, Type::Bool, vec![item]);
            return Ok((
                Some(Statement::If(
                    resume,
                    Vec::new(),
                    vec![Statement::Return(Expr::none())],
                )),
                true,
            ));
        }
        if let ast::Expr::Yield(yielded) = expr
            && yielded.value.is_none()
            && self.context_method
        {
            return Ok((
                Some(Statement::Expr(contexts::op(
                    ir::ProcessOperation::ContextYield,
                    Type::None,
                    Vec::new(),
                ))),
                true,
            ));
        }
        if let ast::Expr::Call(call) = expr {
            match self.resolve(&call.func) {
                Some(Binding::Intrinsic(Intrinsic::AssertNever)) => {
                    return Ok((Some(self.assert_never(call, false)?), false));
                }
                Some(Binding::Intrinsic(Intrinsic::SysExit)) => {
                    return Ok((Some(Statement::Expr(self.expr(expr)?)), false));
                }
                _ => {}
            }
        }
        if self.checked_callback_effects
            && let Some(value) = self.registered_callback(expr)?
        {
            return Ok((Some(Statement::Expr(value)), true));
        }
        let mut value = self.expr_for(expr, ResultUse::Python)?;
        // Only a direct call selects one implementation; a dispatched override may return.
        let continues = !self.terminating_call(&value);
        if self.inference.is_some() && self.discarded_function_body() {
            return Ok((None, continues));
        }
        // A discarded run forwarding stdout to stderr needs only its exit status.
        let result = match &mut value.kind {
            ExprKind::Scope { value, .. } => value.as_mut(),
            _ => &mut value,
        };
        if let ExprKind::Builtin(
            Builtin::Process(op @ ir::ProcessOperation::Run { streams: 4, .. }, site),
            args,
        ) = &mut result.kind
            && site.formal(args, 0).ty == self.types.list(Type::Str)
            && let ast::Expr::Call(call) = expr
            && call
                .arguments
                .keywords
                .iter()
                .all(|kw| matches!(kw.arg.as_deref(), Some("stdout" | "check")))
        {
            *op = ir::ProcessOperation::Call {
                streams: 4,
                fixed_check: None,
            };
            result.ty = Type::Int;
            value.ty = Type::Int;
        }
        Ok((
            Some(self.statement_at(expr, Statement::Expr(value))),
            continues,
        ))
    }

    fn returned(&mut self, ret: &ast::StmtReturn) -> Result<Statement, String> {
        let mut origin = ret.range();
        let mut value = if let Some(value) = &ret.value {
            if let Some(repr) = self.exception_formatter {
                let value = self.expr(value)?;
                if value.ty == Type::ObjectValue {
                    Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(
                            Builtin::ExceptionFormatterResult(repr),
                            vec![value],
                        ),
                    }
                } else {
                    self.coerce(value, Type::Str, ret)?
                }
            } else if let Some(document) = self.discover_document_return {
                if documents::fresh_display(value, document) {
                    // This checked literal can take either layout during final lowering.
                    // Only an already existing mutable owner selects the document ABI.
                    self.explicit_return = true;
                    return self.expr_expected(value, document).map(Statement::Return);
                }
                self.document_annotation_value(value, self.returns, document)?
            } else if self.inferred_returns.is_some() {
                if let Some(context) = self.inferred_return_context {
                    self.expr_in_context(
                        value,
                        context,
                        |_, _| false,
                        |checker, operand, expected, site| {
                            if operand.ty != expected
                                && checker.types.element(operand.ty).is_some()
                                && checker.types.element(expected) == Some(Type::ObjectValue)
                            {
                                // An existing mutable owner cannot change representation.
                                // Its checked source needs the Python calling convention.
                                return Err(checker.classified_error(
                                    Kind::ScopedObject,
                                    site,
                                    "the inferred return retains its original mutable storage",
                                ));
                            }
                            checker.coerce(operand, expected, site)
                        },
                        None,
                    )?
                } else {
                    self.expr(value)?
                }
            } else {
                origin = value.range();
                let expected = self.registered_result_contract.unwrap_or(self.returns);
                self.with_declared_operand(|checker| {
                    checker.expr_in_context(
                        value,
                        expected,
                        |_, _| false,
                        |_, value, _, _| Ok(value),
                        checker.return_declaration,
                    )
                })?
            }
        } else {
            Expr::none()
        };
        let terminates = self.terminating_call(&value);
        self.explicit_return |= !terminates;
        if !terminates
            && let Some(tail) = self.python_result_tail
            && !matches!(self.value_source(&value), Some(ValueSource::PythonCall))
        {
            if ret.start() != tail {
                return Err(self.classified_error(
                    Kind::ScopedObject,
                    ret,
                    "the return requires its fixed source layout",
                ));
            }
            self.python_result_tail = None;
        }
        let operand = if let Some(types) = &mut self.inferred_returns {
            if !terminates {
                types.push(value.ty);
            }
            None
        } else {
            let source_result = !terminates
                && self.checked_call_contracts
                && self.registered_result_contract.is_none()
                && value.ty == Type::PythonObject
                && if self.returns.builtin_scalar_name().is_some() {
                    ret.value.as_deref().is_some_and(|expression| {
                        self.graph.session.scalar_type(self.module, expression)
                            == Some(self.returns)
                    })
                } else {
                    // The source check for this Python frame enforces its declared return.
                    self.registered.is_some() && self.return_declaration.is_some()
                };
            if !terminates && !source_result {
                let expected = self.registered_result_contract.unwrap_or(self.returns);
                let actual = value.ty;
                let checked = if self.registered_result_contract.is_some()
                    && value.ty == Type::PythonObject
                {
                    // The Python frame returns the original handle, even when its source
                    // operand carries a checked type from a live call.
                    let actual = value.checked_operand_type();
                    if actual == Type::PythonObject {
                        if self.types.is_display_container(expected)
                            && let (Some(site), Some(declaration)) =
                                (ret.value.as_deref(), self.return_declaration)
                            && !self
                                .graph
                                .session
                                .operand_types(self.module)
                                .accepts_return_operand(site, declaration.annotation)
                        {
                            return Err(self.classified_error(
                                Kind::ScopedObject,
                                site,
                                "the return requires its fixed source layout",
                            ));
                        }
                        Ok(value)
                    } else {
                        self.expect(expected, actual, &origin).map(|()| value)
                    }
                } else {
                    self.coerce(value, expected, &origin)
                };
                value = checked.map_err(|failure| {
                    if matches!(&failure, Failure::Diagnostic(d) if d.kind == Kind::ExactType) {
                        // Dormant numeric subtypes satisfy the source declaration,
                        // even if the fixed ABI cannot represent this return.
                        if matches!(
                            (expected, actual),
                            (Type::Int, Type::Bool) | (Type::Float, Type::Int | Type::Bool)
                        ) && self.return_declaration.is_some()
                            && self
                                .graph
                                .session
                                .unreachable_range(self.module, origin)
                                .is_some()
                        {
                            return failure.classify(Kind::ScopedObject);
                        }
                        failure
                            .expected_from(self.return_declaration, origin)
                            .classify(Kind::ReturnType)
                    } else {
                        failure
                    }
                })?;
                if self.return_declaration.is_some() && self.native_type_proof(&value, expected) {
                    self.graph.checked_native_type(
                        self.module,
                        crate::analysis::CheckedDiagnostic::NativeReturn(origin),
                    );
                }
            }
            ret.value.as_deref()
        };
        if terminates && value.ty != self.returns {
            // The fixed call never returns. Leave the return site unplaced so a Python
            // frame can return the actual value of a live replacement.
            return Ok(Statement::Expr(value));
        }
        if !terminates && self.reachable {
            self.record_caller_frame_return();
        }
        let statement = Statement::Return(value);
        Ok(match operand {
            Some(operand) => self.statement_at(operand, statement),
            None => statement,
        })
    }

    fn for_loop(&mut self, loop_: &ast::StmtFor) -> Result<Block, String> {
        if !self.fixed_stdin(&loop_.iter) {
            return self.iteration(loop_, true);
        }
        // A source-inexpressible local lets the ordinary file loop own the
        // stream across break, continue, return, and exceptions.
        let input = self.hidden(Type::TextIO);
        let name = format!("@stdin{input}");
        let mut lowered = loop_.clone();
        lowered.iter = Box::new(ast::Expr::Name(ast::ExprName {
            node_index: ast::AtomicNodeIndex::NONE,
            range: loop_.iter.range(),
            id: name.clone().into(),
            ctx: ast::ExprContext::Load,
        }));
        self.names.insert(name.clone(), input);
        self.assigned.insert(input);
        let result = self.iteration(&lowered, true);
        self.names.remove(&name);
        self.assigned.remove(&input);
        let (mut body, continues) = result?;
        body.insert(
            0,
            Statement::Assign(
                input,
                Expr {
                    ty: Type::TextIO,
                    kind: ExprKind::Builtin(Builtin::SysStdin, vec![]),
                },
            ),
        );
        Ok((
            vec![Statement::Scope {
                body,
                clear: vec![input],
            }],
            continues,
        ))
    }

    fn raised_value(
        &mut self,
        raise: &ast::StmtRaise,
        value: &ast::Expr,
    ) -> Result<Statement, String> {
        if self.registered.is_some() {
            return registered::raised_value(raise, value, |value| self.expr(value));
        }
        let raised = match self.raised_class(value)? {
            Some(exception) => exception,
            None => self.expr_expected(value, Type::Exception)?,
        };
        let (raised, cause) = match &raise.cause {
            Some(cause) => self.exception_with_cause(raise, raised, cause)?,
            None => (raised, None),
        };
        let origin = self.fixed_origin(raise.range(), None);
        Ok(self.statement_at(value, Statement::raise_at(raised, cause, Some(origin))))
    }

    fn record_loop_transfer(&mut self, transfer: &Statement) -> bool {
        if self.loop_breaks.is_empty() {
            return false;
        }
        if self.reachable && matches!(transfer, Statement::Break(_)) {
            let mut exits = self.take_breaks();
            self.merge_exit(&mut exits);
            *self.loop_breaks.last_mut().unwrap() = exits;
        }
        true
    }

    fn other_statement(
        &mut self,
        stmt: &ast::Stmt,
        output: &mut Vec<Statement>,
    ) -> Result<bool, String> {
        // A nested try keeps this frame live while its protected and cleanup suites are checked.
        match stmt {
            ast::Stmt::Try(_)
                if self.graph.modules[self.module]
                    .optional_imports
                    .contains_key(&stmt.range()) =>
            {
                let error = self.scoped_builtin_error("an optional standard-library import", stmt);
                self.defer_object_error(&error);
                Ok(true)
            }
            ast::Stmt::Try(try_) if !try_.is_star => {
                let start = output.len();
                let continues = self.try_statement(try_, output)?;
                if let Some(sites) = &mut self.fixed_sites
                    && let [statement] = &output[start..]
                    && let Some(first) = try_.body.first()
                {
                    sites.statement(first.range(), statement);
                }
                Ok(continues)
            }
            _ => {
                let (statements, continues) = self.specialized_statement(stmt)?;
                append_block(output, statements);
                Ok(continues)
            }
        }
    }
}

#[cfg(test)]
mod missing_module_probe_tests {
    #[test]
    fn closed_name_guard_checks_only_the_exact_missing_probe() {
        let positive = "import json\ntry:\n    absent_builtin\n    found = True\nexcept NameError:\n    found = False\n    absent_builtin = range\nprint(found)\n";
        crate::check(positive).unwrap();
        crate::compile(positive, &crate::CompileOptions::default()).unwrap();
        let unrelated = format!("{positive}print(unrelated_missing_name)\n");
        assert!(crate::check(&unrelated).is_err());
        let type_error = format!("{positive}bad: int = 'wrong'\n");
        assert!(crate::check(&type_error).is_err());
        let captured =
            "try:\n    missing_name\nexcept NameError as error:\n    print(error.name)\n";
        assert!(crate::check(captured).is_err());
    }

    #[test]
    fn closed_missing_name_is_catchable_but_builtin_mutation_is_not_assumed_away() {
        let positive = "import json\ntry:\n    missing_name\n    found = True\nexcept NameError:\n    found = False\nprint(found)\n";
        let graph = crate::modules::SourceGraph::single(positive).unwrap();
        assert!(crate::frontend::lower(&graph).is_ok());

        let changed = "import builtins\nbuiltins.missing_name = 42\ntry:\n    missing_name\nexcept NameError:\n    print(42)\n";
        let graph = crate::modules::SourceGraph::single(changed).unwrap();
        assert!(crate::frontend::lower(&graph).is_err());

        let captured =
            "try:\n    missing_name\nexcept NameError as error:\n    print(error.name)\n";
        let graph = crate::modules::SourceGraph::single(captured).unwrap();
        assert!(crate::frontend::lower(&graph).is_err());

        let builtin = "try:\n    range\nexcept NameError:\n    found = False\n";
        let graph = crate::modules::SourceGraph::single(builtin).unwrap();
        assert!(crate::frontend::lower(&graph).is_ok());
    }
}

#[cfg(test)]
mod native_stored_generator_tests {
    #[test]
    fn a_closed_conditional_retains_its_catchable_scalar_error() {
        let source = "if __name__ == '__main__':\n    legacy = False\n    raw = b'ab'\n    pieces = (raw[i:i + 1] for i in range(2))\n    pieces = [','.join(str(ord(c) if legacy else c) for c in part) for part in pieces]\n    print(pieces)\n";
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        assert!(crate::frontend::lower(&graph).is_ok());
    }

    #[test]
    fn adjacent_consumer_is_fused_but_escaping_or_protected_binding_is_refused() {
        let positive = "if __name__ == '__main__':\n    data = b'abcdef'\n    pieces = (data[i:i + 2] for i in range(0, len(data), 2))\n    pieces = [','.join(str(c) for c in piece) for piece in pieces]\n    print(';'.join(pieces))\n";
        let graph = crate::modules::SourceGraph::single(positive).unwrap();
        assert!(crate::frontend::lower(&graph).is_ok());

        for refused in [
            "if __name__ == '__main__':\n    values = (x for x in range(3))\n    other = values\n    values = [x for x in values]\n",
            "if __name__ == '__main__':\n    try:\n        values = (x for x in range(3))\n        values = [1 // x for x in values]\n    except ZeroDivisionError:\n        print(next(values))\n",
        ] {
            let graph = crate::modules::SourceGraph::single(refused).unwrap();
            assert!(crate::frontend::lower(&graph).is_err());
        }
    }
}
