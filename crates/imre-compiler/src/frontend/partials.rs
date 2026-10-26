//! A partial retains its bound operands once, then fills the open positions at each call.

use super::*;
use ir::PlaceholderKind;

/// Each load owns a reference to the same native singleton or its class object.
pub(super) fn singleton(kind: PlaceholderKind) -> Expr {
    Expr {
        ty: Type::Placeholder(kind),
        kind: ExprKind::Builtin(Builtin::Placeholder(kind), vec![]),
    }
}

pub(super) fn after(value: Expr, result: Expr) -> Expr {
    Expr {
        ty: result.ty,
        kind: ExprKind::Scope {
            body: vec![Statement::Expr(value)],
            value: Box::new(result),
            clear: vec![],
        },
    }
}

enum Target {
    Direct(usize),
    Indirect,
}

fn local(slot: usize, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Local(slot),
    }
}

fn raised(ty: Type, mut arguments: Vec<Expr>, message: &str) -> Expr {
    arguments.push(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("TypeError").unwrap(),
            message: Some(Box::new(unpacking::string(message))),
        },
    });
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::RaiseException, arguments),
    }
}

impl Checker<'_> {
    pub(super) fn is_partial_args(&self, expr: &Expr) -> bool {
        let ExprKind::Field(frame, slot) = &expr.kind else {
            return false;
        };
        let ExprKind::Builtin(Builtin::CallableContext, values) = &frame.kind else {
            return false;
        };
        matches!(values.as_slice(), [wrapper] if self.types.partial_function(wrapper.ty).is_some_and(|p| p.func_slot + 1 == *slot))
    }

    /// Read a retained native field, including when an empty wrapper borrows its parent's frame.
    pub(super) fn partial_attribute(&self, receiver: &Expr, name: &str) -> Option<Expr> {
        let layout = self.types.partial_function(receiver.ty)?;
        let slot = match name {
            "func" => layout.func_slot,
            "args" => layout.func_slot + 1,
            _ => return None,
        };
        let ty = *self.types.fields(layout.frame)?.get(slot)?;
        let frame = Expr {
            ty: layout.frame,
            kind: ExprKind::Builtin(Builtin::CallableContext, vec![receiver.clone()]),
        };
        Some(Expr {
            ty,
            kind: ExprKind::Field(Box::new(frame), slot),
        })
    }

    /// Fill an inner partial's placeholders before appending outer operands, as the constructor does.
    fn partial_args_tuple(&self, inner: Option<Expr>, values: &[Expr]) -> Option<Expr> {
        let marker = Type::Placeholder(PlaceholderKind::Value);
        let mut supplied = values.iter().cloned();
        let mut args = Vec::new();
        if let Some(inner) = inner {
            for (index, ty) in self.types.fields(inner.ty)?.into_iter().enumerate() {
                let field = || Expr {
                    ty,
                    kind: ExprKind::Field(Box::new(inner.clone()), index),
                };
                args.push(if ty == marker {
                    supplied.next().unwrap_or_else(field)
                } else {
                    field()
                });
            }
        }
        args.extend(supplied);
        // A partial can capture callbacks in its private frame, even when exposing their
        // tuple would exceed the native public tuple storage contract.
        if args
            .iter()
            .any(|arg| callables::contains_callable(arg.ty, self.types))
        {
            return None;
        }
        Some(Expr {
            ty: self.types.tuple(args.iter().map(|arg| arg.ty).collect()),
            kind: ExprKind::Tuple(args),
        })
    }

    /// Retain each element of a tuple display without storing its callable in a public tuple.
    fn partial_spread(
        &mut self,
        scope: &mut calls::ArgumentScope,
        star: &ast::ExprStarred,
    ) -> Result<Vec<Expr>, String> {
        let ast::Expr::Tuple(tuple) = &*star.value else {
            return scope.spread(self, star);
        };
        let mut fields = Vec::with_capacity(tuple.elts.len());
        for item in &tuple.elts {
            if let ast::Expr::Starred(nested) = item {
                fields.extend(self.partial_spread(scope, nested)?);
            } else {
                let value = self.expr(item)?;
                fields.push(scope.hold(self, value));
            }
        }
        Ok(fields)
    }

    /// The constructor accepts no arguments; ordinary operands are still evaluated before an error.
    pub(super) fn placeholder_call(
        &mut self,
        call: &ast::ExprCall,
        target: Expr,
        kind: PlaceholderKind,
    ) -> Result<Expr, String> {
        if kind == PlaceholderKind::Class && call.arguments.is_empty() {
            return Ok(after(target, singleton(PlaceholderKind::Value)));
        }
        if call.arguments.args.iter().any(ast::Expr::is_starred_expr)
            || call.arguments.keywords.iter().any(|kw| kw.arg.is_none())
        {
            return Err(error(
                self.source,
                call,
                "expanding arguments into a Placeholder call requires a dynamic call frame",
            ));
        }
        match kind {
            PlaceholderKind::Class => {
                for arg in &call.arguments.args {
                    self.graph.checked_excess_argument(self.module, arg.range());
                }
                self.graph.checked_keywords(
                    self.module,
                    call.arguments.keywords.iter().map(Ranged::range),
                );
            }
            PlaceholderKind::Value => self.graph.checked_noncallable(self.module, call.range()),
        }
        let mut values = vec![target];
        for argument in call
            .arguments
            .args
            .iter()
            .chain(call.arguments.keywords.iter().map(|kw| &kw.value))
        {
            values.push(self.expr(argument)?);
        }
        let message = match kind {
            PlaceholderKind::Value => "'functools._PlaceholderType' object is not callable",
            PlaceholderKind::Class => "PlaceholderType takes no arguments",
        };
        Ok(raised(
            Type::Placeholder(PlaceholderKind::Value),
            values,
            message,
        ))
    }

    pub(super) fn partial(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        if call.arguments.keywords.iter().any(|kw| kw.arg.is_none()) {
            return Err(error(
                self.source,
                call,
                "partial requires named keyword operands",
            ));
        }
        let mut scope = calls::ArgumentScope::default();
        let mut operands = call.arguments.args.iter();
        // The target can itself be the first item of an unpacked tuple.
        let (function, callable, leading) = loop {
            let Some(operand) = operands.next() else {
                self.graph
                    .checked_missing_argument(self.module, call.range());
                return self.partial_error(
                    call,
                    scope,
                    &[],
                    None,
                    "type 'partial' takes at least one argument",
                );
            };
            if let ast::Expr::Starred(star) = operand {
                let mut items = self.partial_spread(&mut scope, star)?.into_iter();
                if let Some(first) = items.next() {
                    break (operand, first, items.collect::<Vec<_>>());
                }
            } else {
                break (operand, self.expr(operand)?, Vec::new());
            }
        };
        let bound = operands.as_slice();
        let Some((parameters, returns)) = self.types.signature(callable.ty) else {
            if matches!(
                callable.ty,
                Type::None
                    | Type::Bool
                    | Type::Placeholder(PlaceholderKind::Value)
                    | Type::Int
                    | Type::Float
                    | Type::Str
                    | Type::Bytes
                    | Type::ListInt
                    | Type::List(_)
                    | Type::Dict(_)
                    | Type::Tuple(_)
                    | Type::Set(_)
            ) {
                return self.partial_error(
                    call,
                    scope,
                    bound,
                    Some(callable),
                    "the first argument must be callable",
                );
            }
            return Err(error(
                self.source,
                function,
                "partial requires a callable with a known signature",
            ));
        };
        // A known definition uses its direct ABI. For a bound method, capture
        // the receiver before any bound arguments; a dynamic target owns the
        // callable itself in position zero of the private, immutable frame.
        let mut captured = Vec::new();
        let inner_layout = self.types.partial_function(callable.ty);
        let (target, receiver, original) = match callable.kind {
            ExprKind::Builtin(Builtin::StaticFunction(function), _) => {
                // Use the actual module function object for partial.func.
                (
                    Target::Direct(function),
                    false,
                    Some(scope.hold(self, callable)),
                )
            }
            ExprKind::Callable { function, receiver } => {
                let has_receiver = receiver.is_some();
                if let Some(receiver) = receiver {
                    captured.push(scope.hold(self, *receiver));
                }
                let retained = scope.hold(
                    self,
                    Expr {
                        ty: callable.ty,
                        kind: ExprKind::Callable {
                            function,
                            receiver: captured.first().cloned().map(Box::new),
                        },
                    },
                );
                (Target::Direct(function), has_receiver, Some(retained))
            }
            _ => {
                captured.push(scope.hold(self, callable));
                (Target::Indirect, false, None)
            }
        };
        let marker = Type::Placeholder(PlaceholderKind::Value);
        let mut evaluated: Vec<_> = leading.into_iter().map(|value| (value, function)).collect();
        for arg in bound {
            if let ast::Expr::Starred(star) = arg {
                evaluated.extend(
                    self.partial_spread(&mut scope, star)?
                        .into_iter()
                        .map(|value| (value, arg)),
                );
                continue;
            }
            let value = match parameters.get(evaluated.len()) {
                Some(&ty) => self.expr_expected_or(arg, ty, Some(marker))?,
                None => self.expr(arg)?,
            };
            evaluated.push((scope.hold(self, value), arg));
        }
        let mut keyword_marker = false;
        for kw in &call.arguments.keywords {
            let value = self.expr(&kw.value)?;
            keyword_marker |= value.ty == marker;
            scope.hold(self, value);
        }
        let message = if evaluated
            .last()
            .is_some_and(|(value, _)| value.ty == marker)
        {
            Some("trailing Placeholders are not allowed")
        } else if keyword_marker {
            Some("Placeholder cannot be passed as a keyword argument")
        } else {
            None
        };
        if let Some(message) = message {
            let ty = self.types.callable(vec![], Type::None);
            return Ok(scope.finish(raised(ty, vec![], message)));
        }
        self.reject_builtin_keywords(call)?;
        if matches!(returns, Type::Coroutine(_)) || evaluated.len() > parameters.len() {
            return Err(error(
                self.source,
                call,
                "partial requires a synchronous target with enough positional parameters",
            ));
        }
        if evaluated.is_empty() && matches!(target, Target::Indirect) {
            // No arguments change the target's ABI. Keep its identity as the owner
            // of a new partial rather than emitting a recursive forwarding helper.
            let ty = if inner_layout.is_some() {
                captured[0].ty
            } else {
                self.types.callable(parameters, returns)
            };
            let original = self.coerce(captured[0].clone(), ty, function)?;
            return Ok(scope.finish(Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::EmptyPartial, vec![original]),
            }));
        }
        let mut slots = Vec::with_capacity(parameters.len());
        let mut remaining = Vec::new();
        for (index, &ty) in parameters.iter().enumerate() {
            if let Some((value, site)) = evaluated.get(index)
                && value.ty != marker
            {
                slots.push((true, captured.len(), ty));
                captured.push(self.coerce_argument_value(value.clone(), ty, site)?);
            } else {
                remaining.push(ty);
                slots.push((false, remaining.len(), ty));
            }
        }
        let inherited = inner_layout.map(|_| original.as_ref().unwrap_or(&captured[0]).clone());
        let func = match &inherited {
            Some(inner) => self.partial_attribute(inner, "func"),
            None => original,
        };
        let func_slot = func.map(|func| {
            let slot = captured.len();
            captured.push(func);
            // Erased Callable parameters can conceal a partial with a different underlying
            // signature. Only publish metadata when the root is known at construction.
            let previous = inherited
                .as_ref()
                .and_then(|inner| self.partial_attribute(inner, "args"));
            if inherited.is_none() || previous.is_some() {
                let values: Vec<_> = evaluated.iter().map(|(value, _)| value.clone()).collect();
                if let Some(args) = self.partial_args_tuple(previous, &values) {
                    captured.push(args);
                }
            }
            slot
        });
        let frame = self
            .types
            .partial_frame(captured.iter().map(|v| v.ty).collect());
        let field = |index, ty| Expr {
            ty,
            kind: ExprKind::Field(Box::new(local(0, frame)), index),
        };
        let arguments = slots.into_iter().map(|(bound, index, ty)| {
            if bound {
                field(index, ty)
            } else {
                local(index, ty)
            }
        });
        let (call, calls) = match target {
            Target::Direct(id) => {
                let receiver = receiver.then(|| field(0, captured[0].ty));
                (
                    ExprKind::Call(
                        id,
                        ir::CallArgs::positional(receiver.into_iter().chain(arguments).collect()),
                    ),
                    HashSet::from([id]),
                )
            }
            Target::Indirect => (
                ExprKind::Invoke {
                    callable: Box::new(field(0, captured[0].ty)),
                    args: ir::CallArgs::positional(arguments.collect()),
                },
                HashSet::new(),
            ),
        };
        let id = self.helpers.reserve();
        let locals: Vec<_> = [frame]
            .into_iter()
            .chain(remaining.iter().copied())
            .collect();
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                function: ir::Function {
                    params: (0..locals.len()).collect(),
                    locals,
                    returns,
                    body: vec![Statement::Return(Expr {
                        ty: returns,
                        kind: call,
                    })],
                },
                calls,
                class_reads: HashSet::new(),
                source_frame: false,
                recursion_error: self.graph.modules[self.module]
                    .error(function, "recursive calls through partial are unsupported"),
            },
        );
        let callable = self.types.callable(remaining, returns);
        let ty = func_slot.map_or(callable, |func_slot| {
            self.types.partial_callable(crate::types::PartialFunction {
                callable,
                frame,
                func_slot,
            })
        });
        let context = Expr {
            ty: frame,
            kind: ExprKind::Tuple(captured),
        };
        Ok(scope.finish(Expr {
            ty,
            kind: ExprKind::Callable {
                function: id,
                receiver: Some(Box::new(context)),
            },
        }))
    }

    /// Evaluate every supplied operand before reporting a proven constructor error.
    fn partial_error(
        &mut self,
        call: &ast::ExprCall,
        mut scope: calls::ArgumentScope,
        bound: &[ast::Expr],
        callable: Option<Expr>,
        message: &str,
    ) -> Result<Expr, String> {
        if let Some(callable) = callable {
            scope.hold(self, callable);
        }
        for arg in bound {
            if let ast::Expr::Starred(star) = arg {
                self.partial_spread(&mut scope, star)?;
            } else {
                let value = self.expr(arg)?;
                scope.hold(self, value);
            }
        }
        for kw in &call.arguments.keywords {
            let value = self.expr(&kw.value)?;
            scope.hold(self, value);
        }
        Ok(scope.finish(raised(
            self.types.callable(vec![], Type::None),
            vec![],
            message,
        )))
    }
}
