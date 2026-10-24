//! Cold native calls capture source-ordered arguments. Only await, task-group
//! scheduling, or an event-loop runner can enter an asynchronous function body.

use super::*;
use crate::ir::ProcessOperation as Op;

fn operation(op: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Process(op), args),
    }
}

impl Checker<'_> {
    pub(super) fn indirect_coroutine(
        &self,
        callable: Expr,
        args: Vec<Expr>,
        returns: Type,
    ) -> Expr {
        let signature = callable.ty;
        let mut values = Vec::with_capacity(args.len() + 1);
        values.push(callable);
        values.extend(args);
        let frame = Expr {
            ty: self.types.tuple(values.iter().map(|arg| arg.ty).collect()),
            kind: ExprKind::Tuple(values),
        };
        operation(
            Op::IndirectCoroutine {
                signature,
                result: self.types.awaited(returns).unwrap(),
            },
            returns,
            vec![frame],
        )
    }

    pub(super) fn async_gather(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let tasks = match call.arguments.args.as_ref() {
            [ast::Expr::Starred(args)] => self.expr(&args.value)?,
            args => {
                let values = args
                    .iter()
                    .map(|arg| self.expr(arg))
                    .collect::<Result<Vec<_>, _>>()?;
                let first = values.first().ok_or_else(|| {
                    error(
                        self.source,
                        call,
                        "empty gather needs a typed list of coroutines",
                    )
                })?;
                if values.iter().any(|value| value.ty != first.ty) {
                    return Err(error(
                        self.source,
                        call,
                        "gather arguments need a common result type",
                    ));
                }
                Expr {
                    ty: self.types.list(first.ty),
                    kind: ExprKind::List(values),
                }
            }
        };
        let result = self
            .types
            .element(tasks.ty)
            .and_then(|item| self.types.awaited(item))
            .ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "gather requires coroutines or tasks sharing one result type",
                )
            })?;
        let report = match call.arguments.keywords.as_ref() {
            [] => Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(false),
            },
            [keyword] if keyword.arg.as_deref() == Some("return_exceptions") => {
                self.expr_expected(&keyword.value, Type::Bool)?
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "gather accepts only return_exceptions",
                ));
            }
        };
        let element = if matches!(report.kind, ExprKind::Bool(false)) {
            result
        } else {
            self.types
                .union(&[result, Type::Exception])
                .ok_or_else(|| {
                    error(
                        self.source,
                        call,
                        "gather with return_exceptions needs a fixed record result",
                    )
                })?
        };
        let payload = self.types.payload(result).unwrap_or(result);
        let tagged = element != result && matches!(payload, Type::Class(_));
        Ok(operation(
            Op::Gather { element, tagged },
            self.types.list(element),
            vec![tasks, report],
        ))
    }

    pub(super) fn call_value(
        &mut self,
        signature: &Signature,
        result: Type,
        args: ir::CallArgs,
    ) -> Expr {
        let context = self.classes.contexts.contains(&signature.id);
        if !signature.asynchronous && !context {
            return Expr {
                ty: result,
                kind: ExprKind::Call(signature.id, args),
            };
        }
        let slots: Vec<_> = args.values.iter().map(|arg| self.hidden(arg.ty)).collect();
        let fields: Vec<_> = args
            .order
            .iter()
            .map(|&index| self.local(slots[index]))
            .collect();
        let frame = Expr {
            ty: self
                .types
                .tuple(fields.iter().map(|field| field.ty).collect()),
            kind: ExprKind::Tuple(fields),
        };
        let (ty, op) = if context {
            (
                Type::GeneratorContext,
                Op::ContextNew {
                    function: signature.id,
                },
            )
        } else {
            (
                self.types.coroutine(result),
                Op::Coroutine {
                    function: signature.id,
                    result,
                },
            )
        };
        let body = slots
            .iter()
            .copied()
            .zip(args.values)
            .map(|(slot, arg)| Statement::Assign(slot, arg))
            .collect();
        Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(operation(op, ty, vec![frame])),
                clear: slots,
            },
        }
    }

    pub(super) fn task_await(
        &self,
        value: Expr,
        run: bool,
        site: &impl Ranged,
    ) -> Result<Expr, String> {
        let result = self.types.awaited(value.ty).ok_or_else(|| {
            error(
                self.source,
                site,
                "await requires a typed coroutine or task",
            )
        })?;
        Ok(operation(
            if run { Op::RunLoop } else { Op::Await },
            result,
            vec![value],
        ))
    }

    pub(super) fn async_builtin(
        &mut self,
        kind: Intrinsic,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        match (kind, call.arguments.args.as_ref()) {
            (Intrinsic::NewLoop, []) => Ok(operation(Op::NewLoop, Type::EventLoop, vec![])),
            (Intrinsic::SetLoop, [event_loop]) => Ok(operation(
                Op::SetLoop,
                Type::None,
                vec![self.expr_expected(event_loop, Type::EventLoop)?],
            )),
            (Intrinsic::EnsureTask, [coroutine]) => {
                let coroutine = self.expr(coroutine)?;
                let (Type::Coroutine(id) | Type::Task(id)) = coroutine.ty else {
                    return Err(error(
                        self.source,
                        call,
                        "ensure_future requires a coroutine or task",
                    ));
                };
                Ok(operation(Op::EnsureTask, Type::Task(id), vec![coroutine]))
            }
            (Intrinsic::RunLoop, [value]) => {
                let value = self.expr(value)?;
                self.task_await(value, true, call)
            }
            (Intrinsic::TaskGroup, []) => Ok(operation(Op::TaskGroup, Type::TaskGroup, vec![])),
            (Intrinsic::Semaphore, [count]) => Ok(operation(
                Op::Semaphore,
                Type::Semaphore,
                vec![self.expr_expected(count, Type::Int)?],
            )),
            _ => Err(error(self.source, call, "unsupported asyncio arguments")),
        }
    }

    pub(super) fn task_method(
        &mut self,
        value: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        match (value.ty, attr.attr.as_str(), call.arguments.args.as_ref()) {
            (Type::EventLoop, "run_until_complete", [future]) => {
                let future = self.expr(future)?;
                let result = self.types.awaited(future.ty).ok_or_else(|| {
                    error(
                        self.source,
                        call,
                        "loop.run_until_complete requires an awaitable",
                    )
                })?;
                Ok(operation(Op::RunUntilComplete, result, vec![value, future]))
            }
            (Type::EventLoop, "close", []) => Ok(operation(Op::CloseLoop, Type::None, vec![value])),
            (Type::EventLoop, "add_signal_handler", [number, ast::Expr::Attribute(cancel)])
                if cancel.attr.as_str() == "cancel" =>
            {
                let number = self.expr_expected(number, Type::Int)?;
                let task = self.expr(&cancel.value)?;
                if !matches!(task.ty, Type::Task(_)) {
                    return Err(error(
                        self.source,
                        call,
                        "signal handler requires a bound task.cancel",
                    ));
                }
                Ok(operation(
                    Op::LoopSignal,
                    Type::None,
                    vec![value, number, task],
                ))
            }
            (Type::Task(_), "cancel", []) => Ok(operation(Op::CancelTask, Type::Bool, vec![value])),
            (Type::TaskGroup, "create_task", [coroutine]) => {
                let coroutine = self.expr(coroutine)?;
                let Type::Coroutine(id) = coroutine.ty else {
                    return Err(error(
                        self.source,
                        call,
                        "TaskGroup.create_task requires a coroutine",
                    ));
                };
                Ok(operation(
                    Op::CreateTask,
                    Type::Task(id),
                    vec![value, coroutine],
                ))
            }
            (Type::Task(_), "result", []) => Ok(operation(
                Op::TaskResult,
                self.types.awaited(value.ty).unwrap(),
                vec![value],
            )),
            _ => Err(error(self.source, call, "unsupported asyncio task method")),
        }
    }

    pub(super) fn async_with(
        &mut self,
        items: &[ast::WithItem],
        suite: &[ast::Stmt],
    ) -> Result<(Vec<Statement>, bool), String> {
        let Some((item, remaining)) = items.split_first() else {
            return self.block(suite);
        };
        let value = self.expr(&item.context_expr)?;
        let ty = value.ty;
        if !matches!(ty, Type::TaskGroup | Type::Semaphore) {
            return Err(error(
                self.source,
                item,
                "async with requires a TaskGroup or Semaphore",
            ));
        }
        let slot = self.hidden(ty);
        let mut body = vec![Statement::Assign(slot, value)];
        body.push(Statement::Expr(operation(
            if ty == Type::Semaphore {
                Op::Acquire
            } else {
                Op::EnterGroup
            },
            Type::None,
            vec![self.local(slot)],
        )));
        let mut entered = Vec::new();
        if let Some(target) = &item.optional_vars {
            let value = if ty == Type::Semaphore {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }
            } else {
                self.local(slot)
            };
            entered.push(self.assign(target, value)?);
        }
        let (rest, falls) = self.async_with(remaining, suite)?;
        entered.extend(rest);
        let handlers = if ty == Type::TaskGroup {
            let error = self.hidden(Type::Exception);
            vec![ir::ExceptHandler {
                kinds: vec![0],
                name: Some(error),
                body: vec![
                    Statement::Expr(operation(
                        Op::AbortGroup,
                        Type::None,
                        vec![self.local(slot), self.local(error)],
                    )),
                    Statement::Raise(None),
                ],
            }]
        } else {
            vec![]
        };
        body.push(Statement::Try {
            body: entered,
            handlers,
            orelse: vec![],
            finalbody: vec![Statement::Expr(operation(
                if ty == Type::Semaphore {
                    Op::Release
                } else {
                    Op::JoinGroup
                },
                Type::None,
                vec![self.local(slot)],
            ))],
        });
        Ok((
            vec![Statement::Scope {
                body,
                clear: vec![slot],
            }],
            falls,
        ))
    }
}
