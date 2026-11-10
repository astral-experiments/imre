//! Ordinary source statements shared by typed code and registered Python scopes.

use super::*;

pub(super) type Block = (Vec<Statement>, bool);

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

    fn assignment(&mut self, assign: &ast::StmtAssign) -> StdResult<Vec<Statement>, Self::Error>;
    fn branch(&mut self, branch: &ast::StmtIf) -> StdResult<Block, Self::Error>;
    fn definition(
        &mut self,
        def: &ast::StmtFunctionDef,
    ) -> StdResult<Option<Statement>, Self::Error>;
    fn effect(&mut self, value: &ast::Expr) -> StdResult<(Option<Statement>, bool), Self::Error>;
    fn returned(&mut self, ret: &ast::StmtReturn) -> StdResult<Expr, Self::Error>;
    fn for_loop(&mut self, loop_: &ast::StmtFor) -> StdResult<Block, Self::Error>;
    fn async_iteration(&self) -> bool {
        false
    }
    fn other_statement(&mut self, stmt: &ast::Stmt) -> StdResult<Block, Self::Error>;

    fn attribute_statements(
        &mut self,
        stmt: &ast::Stmt,
    ) -> StdResult<Option<Vec<Statement>>, Self::Error>;

    fn source_statement(
        &mut self,
        stmt: &ast::Stmt,
        module: bool,
    ) -> StdResult<Block, Self::Error> {
        if let Some(effects) = self.attribute_statements(stmt)? {
            return Ok((effects, true));
        }
        let (statement, continues) = match stmt {
            ast::Stmt::Assign(assign) => return Ok((self.assignment(assign)?, true)),
            ast::Stmt::If(branch) => return self.branch(branch),
            ast::Stmt::FunctionDef(def) => (self.definition(def)?, true),
            ast::Stmt::Expr(expr) => self.effect(&expr.value)?,
            ast::Stmt::Return(ret) if !module => {
                (Some(Statement::Return(self.returned(ret)?)), false)
            }
            ast::Stmt::For(loop_) if !loop_.is_async || self.async_iteration() => {
                return self.for_loop(loop_);
            }
            ast::Stmt::Pass(_) => (None, true),
            _ => return self.other_statement(stmt),
        };
        Ok((statement.into_iter().collect(), continues))
    }
}

impl StatementLowering for Checker<'_> {
    type Error = Failure<String>;

    fn attribute_statements(&mut self, stmt: &ast::Stmt) -> Result<Option<Vec<Statement>>, String> {
        // Known native fields retain their declared layout and assignment contracts.
        if self.registered.is_none() {
            return Ok(None);
        }
        let Some(effects) = registered::attribute_effects(stmt) else {
            return Ok(None);
        };
        if !effects.iter().all(|effect| {
            self.graph
                .session
                .is_dynamic_receiver(self.module, &effect.field.value)
        }) {
            return Ok(None);
        }
        let mut statements = Vec::with_capacity(effects.len());
        for effect in effects {
            let values = effect
                .operands()
                .map(|node| self.expr(node))
                .collect::<Result<Vec<_>, _>>()?;
            let receiver = usize::from(effect.action == ir::AttributeAction::Store);
            if values[receiver].ty != Type::PythonObject {
                return Err(error(
                    self.source,
                    effect.field,
                    "attribute target requires a Python object",
                ));
            }
            statements.push(registered::attribute_statement(
                effect.action,
                effect.field,
                values,
            ));
        }
        Ok(Some(statements))
    }

    fn branch(&mut self, branch: &ast::StmtIf) -> Result<Block, String> {
        self.if_chain(branch)
    }

    fn assignment(&mut self, assign: &ast::StmtAssign) -> Result<Vec<Statement>, String> {
        self.assignment_statement(assign)
    }

    fn definition(&mut self, def: &ast::StmtFunctionDef) -> Result<Option<Statement>, String> {
        self.define_local_function(def)
    }

    fn effect(&mut self, expr: &ast::Expr) -> Result<(Option<Statement>, bool), String> {
        if expr.is_ellipsis_literal_expr() {
            return Ok((None, true));
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
                Some(Binding::Intrinsic(Intrinsic::SubprocessRun))
                    if call.arguments.find_keyword("stdout").is_some()
                        && !processes::boxed_launch_keywords(call) =>
                {
                    return Ok((Some(Statement::Expr(self.process_run(call, true)?)), true));
                }
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
            && let Some(value) = self.registered_callback_effect(expr)?
        {
            return Ok((Some(Statement::Expr(value)), true));
        }
        let value = self.expr(expr)?;
        // Only a direct call selects one implementation; a dispatched override may return.
        let continues = !matches!(&value.kind, ExprKind::Call(id, _) if self.classes.raising_methods.contains(id));
        if let Some(sites) = &mut self.fixed_sites {
            sites.statement(expr.range(), FixedNode::Effect);
        }
        Ok((Some(Statement::Expr(value)), continues))
    }

    fn returned(&mut self, ret: &ast::StmtReturn) -> Result<Expr, String> {
        let value = if let Some(value) = &ret.value {
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
            } else if self.inferred_returns.is_some() {
                self.expr(value)?
            } else {
                self.expr_in_context(value, self.returns, |_, _| false, Self::coerce_return)?
            }
        } else {
            Expr::none()
        };
        if let Some(types) = &mut self.inferred_returns {
            types.push(value.ty);
            Ok(value)
        } else {
            let value = self.coerce_return(value, self.returns, ret)?;
            if let (Some(sites), Some(operand)) = (&mut self.fixed_sites, &ret.value) {
                sites.statement(operand.range(), FixedNode::Return);
            }
            Ok(value)
        }
    }

    fn for_loop(&mut self, loop_: &ast::StmtFor) -> Result<Block, String> {
        self.iteration(loop_)
    }

    fn other_statement(&mut self, stmt: &ast::Stmt) -> Result<Block, String> {
        self.specialized_statement(stmt)
    }
}
