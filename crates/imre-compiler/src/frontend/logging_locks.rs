//! Reentrant handler locks keep their acquired identity until control leaves the scope.
use super::*;
use ir::LogOperation as Op;

impl Checker<'_> {
    /// Bind arguments once and keep recursive ownership observable at Python level.
    pub(super) fn handler_lock_method(
        &mut self,
        receiver: Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if name == "__exit__" {
            if !call.arguments.keywords.is_empty() {
                return Err(error(
                    self.source,
                    call,
                    "the reentrant lock exit takes positional arguments",
                ));
            }
            let mut values = vec![receiver];
            for arg in &call.arguments.args {
                values.push(self.expr(arg)?);
            }
            if call.arguments.args.len() < 3 {
                self.graph
                    .checked_missing_argument(self.module, call.range());
            }
            // RLock releases only the receiver but evaluates all supplied exception arguments.
            return Ok(self.ordered_builtin(
                Builtin::Logging(Op::LockRelease),
                Type::None,
                ir::CallArgs {
                    values,
                    order: vec![0],
                    exhaustive_enum_match: false,
                },
            ));
        }
        let param = |name: &str, value: Expr| Parameter {
            name: name.into(),
            ty: value.ty,
            kind: ParameterKind::PositionalOrKeyword,
            default: Some(value),
            sign_only: false,
        };
        let (mut op, ty, params) = match name {
            "acquire" | "__enter__" => (
                Op::LockAcquire,
                Type::Bool,
                vec![
                    param(
                        "blocking",
                        Expr {
                            ty: Type::Bool,
                            kind: ExprKind::Bool(true),
                        },
                    ),
                    param(
                        "timeout",
                        Expr {
                            ty: Type::Float,
                            kind: ExprKind::Float(-1.0),
                        },
                    ),
                ],
            ),
            "release" => (Op::LockRelease, Type::None, vec![]),
            "_at_fork_reinit" => (Op::LockReset, Type::None, vec![]),
            "_recursion_count" => (Op::LockCount, Type::Int, vec![]),
            "_is_owned" => (Op::LockOwned, Type::Bool, vec![]),
            "locked" => (Op::LockLocked, Type::Bool, vec![]),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "unsupported reentrant lock operation",
                ));
            }
        };
        let args = self.arguments_with(call, &params, Some(receiver), |checker, index, arg| {
            let value = checker.expr(arg)?;
            if index == 0 {
                return Ok(checker.truth_value(value));
            }
            if index == 1 && matches!(value.ty, Type::Int | Type::Bool) {
                op = Op::LockAcquireInt;
                if value.ty == Type::Bool {
                    return Ok(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                    });
                }
                return Ok(value);
            }
            checker.coerce(value, params[index].ty, arg)
        })?;
        Ok(self.ordered_builtin(Builtin::Logging(op), ty, args))
    }

    /// Capture the optional receiver once. Entering None fails before the try;
    /// every exit from the protected body releases exactly the captured lock.
    pub(super) fn handler_lock_scope(
        &mut self,
        item: &ast::WithItem,
        remaining: &[ast::WithItem],
        body: &[ast::Stmt],
        manager: Expr,
    ) -> Result<(Vec<Statement>, bool), String> {
        let ty = self.types.optional(Type::HandlerLock);
        if manager.ty == ty {
            self.graph
                .checked_optional_lock_context(self.module, item.context_expr.range());
        }
        let manager = self.coerce(manager, ty, &item.context_expr)?;
        let captured = self.hidden(ty);
        let enter = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::Logging(Op::LockEnter), vec![self.local(captured)]),
        };
        let mut statements = vec![Statement::Assign(captured, manager)];
        let mut clear = vec![captured];
        let mut protected = Vec::new();
        if let Some(target) = &item.optional_vars {
            if !matches!(&**target, ast::Expr::Name(_)) {
                return Err(error(
                    self.source,
                    target.as_ref(),
                    "with targets must be local names",
                ));
            }
            let result = self.hidden(Type::Bool);
            statements.push(Statement::Assign(result, enter));
            clear.push(result);
            protected.push(self.assign(target, self.local(result))?);
        } else {
            statements.push(Statement::Expr(enter));
        }
        let (nested, falls) = self.with_items(remaining, body)?;
        protected.extend(nested);
        statements.push(Statement::Try {
            body: protected,
            handlers: Vec::new(),
            orelse: Vec::new(),
            finalbody: vec![Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::Logging(Op::LockExit), vec![self.local(captured)]),
            })],
        });
        Ok((
            vec![Statement::Scope {
                body: statements,
                clear,
            }],
            falls,
        ))
    }
}
