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

pub(super) trait StatementLowering {
    type Error;

    fn assignment(
        &mut self,
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
                (None, self.assignment(targets, rhs, body)?)
            }
            ast::Stmt::If(branch) => (None, self.branch(branch, body)?),
            ast::Stmt::FunctionDef(def) => (self.definition(def)?, true),
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
        targets: &[ast::Expr],
        rhs: &ast::Expr,
        body: &mut Vec<Statement>,
    ) -> Result<bool, String> {
        let continues = self.assignment_statement(targets, rhs, body)?;
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

    fn definition(&mut self, def: &ast::StmtFunctionDef) -> Result<Option<Statement>, String> {
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
                let checked = if self.registered_result_contract.is_some()
                    && value.ty == Type::PythonObject
                {
                    // The Python frame returns the original handle, even when its source
                    // operand carries a checked type from a live call.
                    let actual = value.checked_operand_type();
                    if actual == Type::PythonObject {
                        Ok(value)
                    } else {
                        self.expect(expected, actual, &origin).map(|()| value)
                    }
                } else {
                    self.coerce(value, expected, &origin)
                };
                value = checked.map_err(|failure| {
                    if matches!(&failure, Failure::Diagnostic(d) if d.kind == Kind::ExactType) {
                        failure
                            .expected_from(self.return_declaration, origin)
                            .classify(Kind::ReturnType)
                    } else {
                        failure
                    }
                })?;
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
        self.iteration(loop_, true)
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
