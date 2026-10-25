//! A variable's exact value layout is independent of its identity and active context.
use super::*;
use crate::ir::ContextOperation as Op;

pub(super) fn op(operation: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::ContextVar(operation), args),
    }
}
fn none() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}

impl Checker<'_> {
    /// A binding can strongly retain its payload for the life of a task. A
    /// token, context, or opaque callable can point back into that same state.
    /// Until the native heap can collect such cycles, admit checked data layouts.
    fn supported_context_payload(&self, ty: Type, has_default: bool) -> bool {
        let mut pending = vec![ty];
        let mut seen = std::collections::HashSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            match ty {
                Type::None
                | Type::Bool
                | Type::Int
                | Type::Float
                | Type::Str
                | Type::StrLiteral(_)
                | Type::Bytes
                | Type::ObjectValue
                | Type::ListInt
                | Type::Path
                | Type::PurePath(_)
                | Type::Calendar(_)
                | Type::PlainObject => {}
                Type::Class(id) => {
                    // The runtime formatter cannot invoke user repr on constructor defaults.
                    let classes = &self.classes.declarations.definitions;
                    let overrides_repr = || {
                        std::iter::once(id)
                        .chain(classes[id].bases.iter().copied())
                        .any(|owner| classes[owner].node.body.iter().any(|statement| {
                            matches!(statement, ast::Stmt::FunctionDef(method) if method.name.as_str() == "__repr__")
                        }))
                    };
                    // A reachable-only build may have omitted the method from
                    // the lowered table. Its declaration still affects Python repr.
                    if has_default && overrides_repr() {
                        return false;
                    }
                    pending.extend_from_slice(&self.classes.fields[id]);
                    pending.extend_from_slice(&self.types.concrete_subclasses(id));
                }
                Type::List(_)
                | Type::Dict(_)
                | Type::Tuple(_)
                | Type::Set(_)
                | Type::Deque(_)
                | Type::Optional(_)
                | Type::Union(_) => {
                    pending.extend(self.types.children(ty));
                }
                _ => return false,
            }
        }
        true
    }

    pub(super) fn context_constructor(
        &mut self,
        call: &ast::ExprCall,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        let [name] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "ContextVar requires one positional name and an optional keyword default",
            ));
        };
        let default = match call.arguments.keywords.as_ref() {
            [] => None,
            [kw] if kw
                .arg
                .as_ref()
                .is_some_and(|name| name.as_str() == "default") =>
            {
                Some(&kw.value)
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "ContextVar accepts only the default keyword",
                ));
            }
        };
        let name = self.expr_expected(name, Type::Str)?;
        let mut args = vec![name];
        let ty = if let Some(node) = default {
            let v = match expected {
                Some(ty) => self.expr_expected(node, ty)?,
                None => Self::string_value(self.expr(node)?),
            };
            let ty = expected.unwrap_or(if v.ty == Type::None {
                Type::ObjectValue
            } else {
                v.ty
            });
            args.push(self.coerce(v, ty, node)?);
            ty
        } else {
            expected.unwrap_or(Type::ObjectValue)
        };
        if !self.supported_context_payload(ty, default.is_some()) {
            return Err(error(
                self.source,
                call,
                "ContextVar bindings require an acyclic data layout and a default with native repr",
            ));
        }
        Ok(op(Op::Var, self.types.context_var(ty), args))
    }

    pub(super) fn context_attribute(&self, receiver: &Expr, name: &str) -> Option<Expr> {
        let (kind, ty) = match (receiver.ty, name) {
            (Type::ContextVar(_), "name") => (Op::Name, Type::Str),
            (Type::ContextToken(id), "var") => (Op::TokenVar, Type::ContextVar(id)),
            (Type::ContextToken(_), "old_value")
                if self
                    .types
                    .context_value(receiver.ty)
                    .is_some_and(|ty| ty == Type::ObjectValue || self.boxable_value(ty)) =>
            {
                (Op::Old, Type::ObjectValue)
            }
            (Type::ContextToken(_), "MISSING") => (Op::Missing, Type::ContextMissing),
            _ => return None,
        };
        Some(op(kind, ty, vec![receiver.clone()]))
    }
    fn bind_context_operand(
        &mut self,
        value: Expr,
        body: &mut Vec<Statement>,
        slots: &mut Vec<usize>,
    ) -> Expr {
        let slot = self.hidden(value.ty);
        body.push(Statement::Assign(slot, value));
        slots.push(slot);
        self.local(slot)
    }

    /// A token for an arbitrary record can test for MISSING without erasing or
    /// formatting that record. Check the class binding, not the attribute's spelling.
    pub(super) fn context_missing_test(
        &mut self,
        left: &ast::Expr,
        right: &ast::Expr,
        negate: bool,
    ) -> Result<Option<Expr>, String> {
        for (old, sentinel, first) in [(left, right, true), (right, left, false)] {
            let (ast::Expr::Attribute(old), ast::Expr::Attribute(other)) = (old, sentinel) else {
                continue;
            };
            if old.attr.as_str() != "old_value" || other.attr.as_str() != "MISSING" {
                continue;
            }
            let known = self.resolve(&other.value)
                == Some(Binding::Intrinsic(Intrinsic::ContextToken))
                || (matches!(&*other.value,ast::Expr::Name(n) if self.names.get(n.id.as_str()).is_some_and(|slot| matches!(self.locals[*slot],Type::ContextToken(_)))));
            if !known {
                continue;
            }
            let receiver = self.expr(&old.value)?;
            if !matches!(receiver.ty, Type::ContextToken(_)) {
                continue;
            }
            let a = op(Op::WasMissing, Type::Bool, vec![receiver]);
            let sentinel = self.expr(sentinel)?;
            let mut body = Vec::new();
            let mut clear = Vec::new();
            let value = if first {
                let v = self.bind_context_operand(a, &mut body, &mut clear);
                body.push(Statement::Expr(sentinel));
                v
            } else {
                body.push(Statement::Expr(sentinel));
                a
            };
            let value = if negate {
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Not(Box::new(value)),
                }
            } else {
                value
            };
            return Ok(Some(Expr {
                ty: Type::Bool,
                kind: ExprKind::Scope {
                    body,
                    value: Box::new(value),
                    clear,
                },
            }));
        }
        Ok(None)
    }

    /// Format invalid keys after evaluating every operand, including the fallback.
    pub(super) fn context_key_error(
        &mut self,
        ctx: Expr,
        key: Expr,
        fallback: Option<Expr>,
        key_first: bool,
        ty: Type,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if !self.boxable_value(key.ty) {
            return Err(error(
                self.source,
                node,
                "Context keys require a ContextVar or a scalar with native repr",
            ));
        }
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let key = if key_first {
            let key = self.bind_context_operand(key, &mut body, &mut clear);
            self.bind_context_operand(ctx, &mut body, &mut clear);
            key
        } else {
            self.bind_context_operand(ctx, &mut body, &mut clear);
            self.bind_context_operand(key, &mut body, &mut clear)
        };
        if let Some(fallback) = fallback {
            self.bind_context_operand(fallback, &mut body, &mut clear);
        }
        let repr = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::Repr, vec![key]),
        };
        let message = Expr {
            ty: Type::Str,
            kind: ExprKind::Binary(
                Binary::Concat,
                Box::new(unpacking::string("a ContextVar key was expected, got ")),
                Box::new(repr),
            ),
        };
        let exc = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(message)),
            },
        };
        let value = Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::RaiseException, vec![exc]),
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                clear,
                value: Box::new(value),
            },
        })
    }

    /// Evaluate the eager fallback before looking up the binding: it can set the variable.
    fn context_get(
        &mut self,
        receiver: Option<Expr>,
        key: Expr,
        fallback: Option<Expr>,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let Type::ContextVar(_) = key.ty else {
            if let Some(receiver) = receiver {
                return self.context_key_error(
                    receiver,
                    key,
                    fallback,
                    false,
                    Type::ObjectValue,
                    node,
                );
            }
            return Err(error(
                self.source,
                node,
                "Context mapping reads require a ContextVar key",
            ));
        };
        let ty = self.types.context_value(key.ty).unwrap();
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let ctx = receiver.map(|v| self.bind_context_operand(v, &mut body, &mut clear));
        let key = self.bind_context_operand(key, &mut body, &mut clear);
        let args: Vec<_> = std::iter::once(key).chain(ctx.iter().cloned()).collect();
        let read = op(
            if ctx.is_some() {
                Op::ContextRead
            } else {
                Op::Read
            },
            ty,
            args.clone(),
        );
        let value = if let Some(f) = fallback {
            let f = self.bind_context_operand(f, &mut body, &mut clear);
            let joined = if ty == Type::ObjectValue
                && (self.boxable_value(f.ty) || f.ty == Type::ObjectValue)
            {
                Some(ty)
            } else {
                self.types.join(ty, f.ty).or_else(|| {
                    (self.boxable_value(ty) && self.boxable_value(f.ty))
                        .then_some(Type::ObjectValue)
                })
            };
            let joined = joined.ok_or_else(|| {
                error(
                    self.source,
                    node,
                    "context lookup value and fallback require a common representation",
                )
            })?;
            let yes = self.coerce(read, joined, node)?;
            let no = self.coerce(f, joined, node)?;
            let has = op(
                if ctx.is_some() {
                    Op::ContextContains
                } else {
                    Op::Contains
                },
                Type::Bool,
                args,
            );
            Expr {
                ty: joined,
                kind: ExprKind::Conditional(Box::new(has), Box::new(yes), Box::new(no)),
            }
        } else {
            read
        };
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(value),
                clear,
            },
        })
    }
    pub(super) fn context_subscript(&mut self, ctx: Expr, key: &ast::Expr) -> Result<Expr, String> {
        let var = self.expr(key)?;
        self.context_get(Some(ctx), var, None, key)
    }
    pub(super) fn context_method(
        &mut self,
        receiver: Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let args = call.arguments.args.as_ref();
        if name == "run" && receiver.ty == Type::Context {
            return self.context_run(receiver, call);
        }
        self.reject_builtin_keywords(call)?;
        match (receiver.ty, name, args) {
            (Type::ContextVar(_), "get", []) => self.context_get(None, receiver, None, call),
            (Type::ContextVar(_), "get", [default]) => {
                let fallback = Self::string_value(self.expr(default)?);
                self.context_get(None, receiver, Some(fallback), call)
            }
            (Type::ContextVar(id), "set", [value]) => {
                let ty = self.types.context_value(receiver.ty).unwrap();
                let value = self.expr_expected(value, ty)?;
                Ok(op(Op::Set, Type::ContextToken(id), vec![receiver, value]))
            }
            (Type::ContextVar(_), "reset", [t]) => {
                let token = self.expr(t)?;
                if !matches!(token.ty, Type::ContextToken(_)) {
                    return Err(error(self.source, t, "ContextVar.reset requires a Token"));
                }
                Ok(op(Op::Reset, Type::None, vec![receiver, token]))
            }
            (Type::Context, "copy", []) => Ok(op(Op::Copy, Type::Context, vec![receiver])),
            (Type::Context, "get", [key, rest @ ..]) if rest.len() <= 1 => {
                let key = self.expr(key)?;
                let invalid_key = !matches!(key.ty, Type::ContextVar(_));
                let fallback = if let [d] = rest {
                    Self::string_value(self.expr(d)?)
                } else {
                    none()
                };
                let checked = self.context_get(Some(receiver), key, Some(fallback), call)?;
                if invalid_key {
                    self.graph.checked_overload(self.module, call.range());
                }
                Ok(checked)
            }
            _ => Err(error(
                self.source,
                call,
                "unsupported contextvars method or signature",
            )),
        }
    }
    fn context_run(&mut self, ctx: Expr, call: &ast::ExprCall) -> Result<Expr, String> {
        let [fn_node, args @ ..] = call.arguments.args.as_ref() else {
            let failure = self.sys_type_error(
                call,
                Type::None,
                "run() missing 1 required positional argument".to_string(),
            )?;
            self.graph
                .checked_missing_argument(self.module, call.range());
            return Ok(Expr {
                ty: Type::None,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(ctx)],
                    value: Box::new(failure),
                    clear: vec![],
                },
            });
        };
        if !call.arguments.keywords.is_empty() {
            return Err(error(
                self.source,
                call,
                "Context.run currently requires the complete positional callback signature",
            ));
        }
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let ctx = self.bind_context_operand(ctx, &mut body, &mut clear);
        let bound = if let ast::Expr::Attribute(attr) = fn_node
            && matches!(attr.attr.as_str(), "get" | "set" | "reset")
            && let Ok(var) = self.expr(&attr.value)
            && let Type::ContextVar(id) = var.ty
        {
            let ty = self.types.context_value(var.ty).unwrap();
            let var = self.bind_context_operand(var, &mut body, &mut clear);
            Some(match (attr.attr.as_str(), args) {
                ("set", [arg]) => {
                    let v = self.expr_expected(arg, ty)?;
                    let v = self.bind_context_operand(v, &mut body, &mut clear);
                    op(Op::Set, Type::ContextToken(id), vec![var, v])
                }
                ("reset", [arg]) => {
                    let t = self.expr(arg)?;
                    if !matches!(t.ty, Type::ContextToken(_)) {
                        return Err(error(self.source, arg, "ContextVar.reset requires a Token"));
                    }
                    let t = self.bind_context_operand(t, &mut body, &mut clear);
                    op(Op::Reset, Type::None, vec![var, t])
                }
                ("get", []) => self.context_get(None, var, None, call)?,
                ("get", [arg]) => {
                    let fallback = Self::string_value(self.expr(arg)?);
                    let fallback = self.bind_context_operand(fallback, &mut body, &mut clear);
                    self.context_get(None, var, Some(fallback), call)?
                }
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "unsupported ContextVar callback signature",
                    ));
                }
            })
        } else {
            None
        };
        let invoked = if let Some(invoked) = bound {
            invoked
        } else {
            let callback = self.expr(fn_node)?;
            let (params, returns) = self.types.signature(callback.ty).ok_or_else(|| {
                error(
                    self.source,
                    fn_node,
                    "Context.run requires a callable with an exact signature",
                )
            })?;
            if args.len() != params.len() {
                return Err(error(
                    self.source,
                    call,
                    "Context.run currently requires the complete positional callback signature",
                ));
            }
            // A reference to an exact, unbound synchronous definition has no
            // evaluation effects. Retaining its direct edge also avoids
            // spuriously treating every other same-signature callback as recursive.
            let direct = match &callback.kind {
                ExprKind::Callable {
                    function,
                    receiver: None,
                }
                | ExprKind::Builtin(Builtin::StaticFunction(function), _)
                    if !matches!(returns, Type::Coroutine(_)) =>
                {
                    Some(*function)
                }
                _ => None,
            };
            let callback = direct
                .is_none()
                .then(|| self.bind_context_operand(callback, &mut body, &mut clear));
            let mut values = Vec::new();
            for (arg, ty) in args.iter().zip(params) {
                let value = self.expr_expected(arg, ty)?;
                values.push(self.bind_context_operand(value, &mut body, &mut clear));
            }
            if let Some(function) = direct {
                self.calls.insert(function);
                Expr {
                    ty: returns,
                    kind: ExprKind::Call(function, ir::CallArgs::positional(values)),
                }
            } else if matches!(returns, Type::Coroutine(_)) {
                self.indirect_coroutine(callback.unwrap(), values, returns)
            } else {
                Expr {
                    ty: returns,
                    kind: ExprKind::Invoke {
                        callable: Box::new(callback.unwrap()),
                        args: ir::CallArgs::positional(values),
                    },
                }
            }
        };
        let returns = invoked.ty;
        let previous = op(Op::Enter, Type::Context, vec![ctx.clone()]);
        let previous = self.bind_context_operand(previous, &mut body, &mut clear);
        let result = self.hidden(returns);
        clear.push(result);
        body.push(Statement::Try {
            body: vec![Statement::Assign(result, invoked)],
            handlers: Vec::new(),
            orelse: Vec::new(),
            finalbody: vec![Statement::Expr(op(
                Op::Leave,
                Type::None,
                vec![ctx, previous],
            ))],
        });
        Ok(Expr {
            ty: returns,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear,
            },
        })
    }
}
