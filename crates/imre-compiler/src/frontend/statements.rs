//! Ordinary source statements shared by typed code and registered Python scopes.

use super::*;

pub(super) type Block = (Vec<Statement>, bool);

pub(super) trait StatementLowering {
    type Error;

    fn assignment(&mut self, assign: &ast::StmtAssign) -> StdResult<Vec<Statement>, Self::Error>;
    fn definition(
        &mut self,
        def: &ast::StmtFunctionDef,
    ) -> StdResult<Option<Statement>, Self::Error>;
    fn effect(&mut self, value: &ast::Expr) -> StdResult<(Option<Statement>, bool), Self::Error>;
    fn returned(&mut self, ret: &ast::StmtReturn) -> StdResult<Expr, Self::Error>;
    fn other_statement(&mut self, stmt: &ast::Stmt) -> StdResult<Block, Self::Error>;

    fn source_statement(
        &mut self,
        stmt: &ast::Stmt,
        module: bool,
    ) -> StdResult<Block, Self::Error> {
        let (statement, continues) = match stmt {
            ast::Stmt::Assign(assign) => return Ok((self.assignment(assign)?, true)),
            ast::Stmt::FunctionDef(def) => (self.definition(def)?, true),
            ast::Stmt::Expr(expr) => self.effect(&expr.value)?,
            ast::Stmt::Return(ret) if !module => {
                (Some(Statement::Return(self.returned(ret)?)), false)
            }
            ast::Stmt::Pass(_) => (None, true),
            _ => return self.other_statement(stmt),
        };
        Ok((statement.into_iter().collect(), continues))
    }
}

impl StatementLowering for Checker<'_> {
    type Error = Failure<String>;

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
        let value = self.expr(expr)?;
        // Only a direct call selects one implementation; a dispatched override may return.
        let continues = !matches!(&value.kind, ExprKind::Call(id, _) if self.classes.raising_methods.contains(id));
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
            self.coerce_return(value, self.returns, ret)
        }
    }

    fn other_statement(&mut self, stmt: &ast::Stmt) -> Result<Block, String> {
        self.specialized_statement(stmt)
    }
}
