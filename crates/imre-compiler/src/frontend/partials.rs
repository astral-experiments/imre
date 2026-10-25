//! A partial retains its bound operands once, then fills the open positions at each call.

use super::*;

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

impl Checker<'_> {
    pub(super) fn partial(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        if call.arguments.keywords.iter().any(|kw| kw.arg.is_none()) {
            return Err(error(
                self.source,
                call,
                "partial requires named keyword operands",
            ));
        }
        let Some((function, bound)) = call.arguments.args.split_first() else {
            return self.partial_error(call, None, "type 'partial' takes at least one argument");
        };
        let callable = self.expr(function)?;
        let Some((parameters, returns)) = self.types.signature(callable.ty) else {
            if matches!(
                callable.ty,
                Type::None
                    | Type::Bool
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
        let placeholder = Some(Binding::Intrinsic(Intrinsic::Placeholder));
        if bound
            .last()
            .is_some_and(|arg| self.resolve(arg) == placeholder)
        {
            return self.partial_error(
                call,
                Some(callable),
                "trailing Placeholders are not allowed",
            );
        }
        if call
            .arguments
            .keywords
            .iter()
            .any(|kw| self.resolve(&kw.value) == placeholder)
        {
            return self.partial_error(
                call,
                Some(callable),
                "Placeholder cannot be passed as a keyword argument",
            );
        }
        self.reject_builtin_keywords(call)?;
        if matches!(returns, Type::Coroutine(_)) || bound.len() > parameters.len() {
            return Err(error(
                self.source,
                call,
                "partial requires a synchronous target with enough positional parameters",
            ));
        }
        if bound.is_empty() {
            // The call interface is unchanged. An indirect wrapper with this same
            // signature would appear to call itself in the closed target set.
            return Ok(callable);
        }

        // A known definition uses its direct ABI. For a bound method, capture
        // the receiver before any bound arguments; a dynamic target owns the
        // callable itself in position zero of the private, immutable frame.
        let mut captured = Vec::new();
        let (target, receiver) = match callable.kind {
            ExprKind::Callable { function, receiver } => {
                let has_receiver = receiver.is_some();
                if let Some(receiver) = receiver {
                    captured.push(*receiver);
                }
                (Target::Direct(function), has_receiver)
            }
            _ => {
                captured.push(callable);
                (Target::Indirect, false)
            }
        };
        let mut slots = Vec::with_capacity(parameters.len());
        let mut remaining = Vec::new();
        for (index, &ty) in parameters.iter().enumerate() {
            if let Some(arg) = bound.get(index)
                && self.resolve(arg) != placeholder
            {
                slots.push((true, captured.len(), ty));
                captured.push(self.expr_expected(arg, ty)?);
            } else {
                remaining.push(ty);
                slots.push((false, remaining.len(), ty));
            }
        }
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
        let ty = self.types.callable(remaining, returns);
        let context = Expr {
            ty: frame,
            kind: ExprKind::Tuple(captured),
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Callable {
                function: id,
                receiver: Some(Box::new(context)),
            },
        })
    }

    /// Evaluate every supplied operand before reporting a proven constructor error.
    fn partial_error(
        &mut self,
        call: &ast::ExprCall,
        callable: Option<Expr>,
        message: &str,
    ) -> Result<Expr, String> {
        let skip = usize::from(callable.is_some());
        let mut args = Vec::from_iter(callable);
        for value in call.arguments.args[skip..]
            .iter()
            .chain(call.arguments.keywords.iter().map(|kw| &kw.value))
        {
            if self.resolve(value) != Some(Binding::Intrinsic(Intrinsic::Placeholder)) {
                args.push(self.expr(value)?);
            }
        }
        args.push(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(message))),
            },
        });
        Ok(Expr {
            ty: self.types.callable(vec![], Type::None),
            kind: ExprKind::Builtin(Builtin::RaiseException, args),
        })
    }
}
