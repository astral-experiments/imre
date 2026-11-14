//! Ordinary source statements shared by typed code and registered Python scopes.

use super::*;

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
    body: Vec<Statement>,
    handlers: Vec<ir::ExceptHandler>,
    orelse: Vec<Statement>,
    normal_exit: bool,
    (finalbody, final_falls): Block,
) -> (Statement, bool) {
    (
        Statement::Try {
            body,
            handlers,
            orelse,
            finalbody,
        },
        normal_exit && final_falls,
    )
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

pub(super) fn if_arms(
    arms: Vec<(Expr, Vec<Statement>)>,
    mut otherwise: Vec<Statement>,
) -> Vec<Statement> {
    for (test, body) in arms.into_iter().rev() {
        otherwise = vec![Statement::If(test, body, otherwise)];
    }
    otherwise
}

pub(super) trait StatementLowering {
    type Error;

    fn assignment(
        &mut self,
        assign: &ast::StmtAssign,
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

    fn source_statement(
        &mut self,
        stmt: &ast::Stmt,
        module: bool,
        body: &mut Vec<Statement>,
    ) -> StdResult<bool, Self::Error> {
        let (statement, continues) = match stmt {
            ast::Stmt::Assign(assign) => return self.assignment(assign, body),
            ast::Stmt::If(branch) => return self.branch(branch, body),
            ast::Stmt::FunctionDef(def) => (self.definition(def)?, true),
            ast::Stmt::Expr(expr) => self.effect(&expr.value)?,
            ast::Stmt::Return(ret) if !module => (Some(self.returned(ret)?), false),
            ast::Stmt::For(loop_) => {
                let (statements, continues) = self.for_loop(loop_)?;
                append_block(body, statements);
                return Ok(continues);
            }
            ast::Stmt::Raise(raise) => (
                Some(raised(raise, |value| self.raised_value(raise, value))?),
                false,
            ),
            ast::Stmt::Pass(_) => (self.position_only(stmt.range()), true),
            _ => match loop_transfer(stmt) {
                Some(transfer) if self.record_loop_transfer(&transfer) => (Some(transfer), false),
                _ => return self.other_statement(stmt, body),
            },
        };
        body.extend(statement);
        Ok(continues)
    }
}

impl StatementLowering for Checker<'_> {
    type Error = Failure<String>;

    fn branch(&mut self, branch: &ast::StmtIf, body: &mut Vec<Statement>) -> Result<bool, String> {
        let (arms, falls) = self.if_chain(branch)?;
        append_block(body, arms);
        Ok(falls)
    }

    fn assignment(
        &mut self,
        assign: &ast::StmtAssign,
        body: &mut Vec<Statement>,
    ) -> Result<bool, String> {
        self.assignment_statement(assign, body)
    }

    fn definition(&mut self, def: &ast::StmtFunctionDef) -> Result<Option<Statement>, String> {
        if self.registered_local_start.is_some() {
            let header = imre_ast::function_definition_expressions(def)
                .map(|expr| self.expr(expr))
                .collect::<Result<Vec<_>, _>>()?;
            let site = ir::DefinitionSite::Function(def.start());
            let value = registered::definition_value(site, header);
            let slot = self.declare(def.name.as_str(), Type::PythonObject, &def.name)?;
            self.assigned_value(slot, &value);
            return Ok(Some(Statement::Assign(slot, value)));
        }
        self.define_local_function(def)
    }

    fn position_only(&mut self, range: TextRange) -> Option<Statement> {
        if self.fault_enabled() {
            return None;
        }
        self.fixed_sites
            .as_mut()?
            .record(range, FixedNode::Position(range));
        Some(Statement::Position(range, None))
    }

    fn effect(&mut self, expr: &ast::Expr) -> Result<(Option<Statement>, bool), String> {
        if expr.is_string_literal_expr() || expr.is_ellipsis_literal_expr() {
            return Ok((self.position_only(expr.range()), true));
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
        let mut value = self.expr(expr)?;
        // A discarded run forwarding stdout to stderr needs only its exit status.
        let result = match &mut value.kind {
            ExprKind::Scope { value, .. } => value.as_mut(),
            _ => &mut value,
        };
        if let ExprKind::Builtin(
            Builtin::Process(op @ ir::ProcessOperation::Run { streams: 4, .. }),
            args,
        ) = &mut result.kind
            && args[0].ty == self.types.list(Type::Str)
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
        // Only a direct call selects one implementation; a dispatched override may return.
        let continues = !self.terminating_call(&value);
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
                self.expr(value)?
            } else {
                origin = value.range();
                self.expr_in_context(
                    value,
                    self.returns,
                    |_, _| false,
                    |_, value, _, _| Ok(value),
                    None,
                )?
            }
        } else {
            Expr::none()
        };
        let terminates = self.terminating_call(&value);
        self.explicit_return |= !terminates;
        let operand = if let Some(types) = &mut self.inferred_returns {
            if !terminates {
                types.push(value.ty);
            }
            None
        } else {
            let source_result = !terminates
                && self.checked_call_contracts
                && self.returns.builtin_scalar_name().is_some()
                && value.ty == Type::PythonObject
                && ret.value.as_deref().is_some_and(|expression| {
                    self.graph.session.scalar_type(self.module, expression) == Some(self.returns)
                });
            if !terminates && !source_result {
                value = self
                    .coerce(value, self.returns, &origin)
                    .map_err(|failure| {
                        if matches!(&failure, Failure::Diagnostic(d) if d.kind == Kind::ExactType) {
                            failure.classify(Kind::ReturnType)
                        } else {
                            failure
                        }
                    })?;
            }
            ret.value.as_deref()
        };
        // A live Python replacement may return even when the selected fixed definition cannot.
        // Keep the written return whenever its value already has the function's return type.
        let statement = if terminates && value.ty != self.returns {
            Statement::Expr(value)
        } else {
            Statement::Return(value)
        };
        Ok(match operand {
            Some(operand) => self.statement_at(operand, statement),
            None => statement,
        })
    }

    fn for_loop(&mut self, loop_: &ast::StmtFor) -> Result<Block, String> {
        self.iteration(loop_)
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
                let outer = self.finally_scope;
                if outer == FinallyScope::Outside && !try_.finalbody.is_empty() {
                    self.finally_scope = FinallyScope::Protected;
                }
                let result = self.try_statement(try_);
                self.finally_scope = outer;
                let (statement, continues) = result?;
                output.push(statement);
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
