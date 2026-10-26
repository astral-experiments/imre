//! Build each dictionary in key order, retaining the same evaluated value for every entry.

use super::*;

impl Checker<'_> {
    /// Resolve the builtin class method and bound mappings whose shape is already known.
    pub(super) fn fromkeys_call(
        &mut self,
        call: &ast::ExprCall,
        shape: Option<(Type, Type)>,
    ) -> Result<Option<Expr>, String> {
        let ast::Expr::Attribute(method) = &*call.func else {
            return Ok(None);
        };
        if method.attr.as_str() != "fromkeys" {
            return Ok(None);
        }
        if self.unshadowed_builtin(&method.value, "dict") {
            return self.dict_fromkeys(call, shape, None).map(Some);
        }
        let receiver = match &*method.value {
            ast::Expr::Dict(literal) if literal.items.is_empty() => {
                self.expr_expected(&method.value, self.types.dict(Type::Int, Type::None))?
            }
            ast::Expr::Dict(_) => self.expr(&method.value)?,
            ast::Expr::Name(name)
                if self
                    .names
                    .get(name.id.as_str())
                    .is_some_and(|slot| self.types.mapping(self.locals[*slot]).is_some()) =>
            {
                self.expr(&method.value)?
            }
            _ => return Ok(None),
        };
        self.dict_fromkeys(call, shape, Some(receiver)).map(Some)
    }

    pub(super) fn dict_fromkeys(
        &mut self,
        call: &ast::ExprCall,
        shape: Option<(Type, Type)>,
        receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        let mut outer = calls::ArgumentScope::default();
        let receiver = receiver.map(|owner| outer.hold(self, owner));
        let args = call.arguments.args.as_slice();
        let error_type = shape.map_or(Type::None, |(key, value)| self.types.dict(key, value));
        let ordered = receiver
            .as_ref()
            .is_some_and(|owner| self.types.is_ordered_dict(owner.ty));
        let binding_error = if ordered {
            ordered_fromkeys_binding_error(call)
        } else if !call.arguments.keywords.is_empty() {
            Some("dict.fromkeys() takes no keyword arguments".to_string())
        } else if args.is_empty() {
            Some("fromkeys expected at least 1 argument, got 0".to_string())
        } else if args.len() > 2 {
            Some(format!(
                "fromkeys expected at most 2 arguments, got {}",
                args.len()
            ))
        } else {
            None
        };
        if let Some(reason) = binding_error {
            for arg in args {
                outer.discard(self, arg)?;
            }
            for kw in &call.arguments.keywords {
                if kw.arg.is_none() {
                    return Err(error(
                        self.source,
                        kw,
                        "dict.fromkeys keyword unpacking is unsupported",
                    ));
                }
                outer.discard(self, &kw.value)?;
            }
            // Counter.fromkeys(iterable, v=None) accepts names and raises its
            // own exception after the caller has evaluated all arguments.
            let named = |name| call.arguments.find_keyword(name).is_some();
            let binds_counter = match (args.len(), call.arguments.keywords.len()) {
                (0, 1) => named("iterable"),
                (0, 2) => named("iterable") && named("v"),
                (1, 1) => named("v"),
                _ => false,
            };
            if !ordered
                && binds_counter
                && let Some(owner) = receiver
            {
                let (key, value) = shape.unwrap_or((Type::Int, Type::None));
                let target = self.fromkeys_target(Some(owner), key, value);
                outer.hold(self, target);
            }
            return Ok(outer.finish(fromkeys_error(reason, error_type)));
        }
        let named = |name| call.arguments.find_keyword(name).map(|kw| &kw.value);
        let source = args
            .first()
            .or_else(|| named("iterable"))
            .expect("bound iterable");
        let value_arg = args.get(1).or_else(|| named("value"));
        let hint = shape.map(|(key, _)| key);
        // A statically empty source may adopt any immutable key representation.
        let mut input = if matches!(source, ast::Expr::List(list) if list.elts.is_empty()) {
            let key = hint.unwrap_or(Type::Int);
            Iteration::Container {
                iterable: self.expr_expected(source, self.types.list(key))?,
                items: false,
                start: None,
            }
        } else {
            self.expansion_input(source, hint)?
        };
        if let Iteration::Container { iterable, .. } = &mut input
            && matches!(iterable.ty, Type::StrLiteral(_))
        {
            *iterable = Self::string_value(iterable.clone());
        }
        let value = match (value_arg, shape) {
            (Some(value), Some((_, ty))) => self.expr_expected(value, ty)?,
            (Some(value), None) => self.expr(value)?,
            (None, _) => Expr {
                ty: Type::None,
                kind: ExprKind::None,
            },
        };
        // When value= is supplied first, evaluate it before creating the iterable.
        let value = if value_arg.is_some_and(|arg| arg.start() < source.start()) {
            outer.hold(self, value)
        } else {
            value
        };
        if let Iteration::Container { iterable, .. } = &input
            && matches!(
                iterable.ty,
                Type::None | Type::Int | Type::Bool | Type::Float
            )
        {
            let reason = format!("'{}' object is not iterable", self.type_name(iterable.ty));
            outer.hold(self, iterable.clone());
            let value_type = value.ty;
            outer.hold(self, value);
            if let Some(owner) = receiver {
                let target =
                    self.fromkeys_target(Some(owner), hint.unwrap_or(Type::Int), value_type);
                outer.hold(self, target);
            }
            return Ok(outer.finish(fromkeys_error(reason, error_type)));
        }
        let mut input = self.prepare_iteration(input, source, false)?;
        self.hint_empty_iteration(&input, Some(hint.unwrap_or(Type::Int)));
        let key_type = self.locals[input.value];
        if !self.types.hashable_key(key_type) {
            return Err(error(
                self.source,
                source,
                "dict.fromkeys requires supported immutable keys",
            ));
        }
        if let Some(key) = hint {
            self.expect(key, key_type, source)?;
        }
        let result = self.fromkeys_target(receiver, key_type, value.ty);
        let ty = result.ty;
        let kept = self.hidden(value.ty);
        let target = self.hidden(ty);
        // Evaluate the iterable expression and the value before asking for the
        // iterator. A generator expression acquires its own outer iterator earlier.
        input.evaluate.push(Statement::Assign(kept, value));
        input.evaluate.push(Statement::Assign(target, result));
        let body = self.expand_items(Iteration::Prepared(input), source, |checker, key| {
            Ok(vec![Statement::SetItem(
                checker.local(target),
                key,
                checker.local(kept),
            )])
        })?;
        Ok(outer.finish(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(target)),
                clear: vec![target, kept],
            },
        }))
    }

    fn fromkeys_target(&self, receiver: Option<Expr>, key: Type, value: Type) -> Expr {
        let ty = self.types.dict(key, value);
        let empty = Expr {
            ty,
            kind: ExprKind::Dict {
                key,
                value,
                entries: Vec::new(),
            },
        };
        match receiver {
            Some(owner) => Expr {
                ty: if self.types.is_ordered_dict(owner.ty) {
                    self.types.ordered_dict(key, value)
                } else {
                    ty
                },
                kind: ExprKind::Builtin(Builtin::DictFromKeysTarget, vec![owner, empty]),
            },
            None => empty,
        }
    }
}

fn ordered_fromkeys_binding_error(call: &ast::ExprCall) -> Option<String> {
    let positional = call.arguments.args.len();
    let count = positional + call.arguments.keywords.len();
    if count > 2 {
        let kind = if positional == 0 {
            "keyword arguments"
        } else {
            "arguments"
        };
        return Some(format!("fromkeys() takes at most 2 {kind} ({count} given)"));
    }
    if positional == 0 && call.arguments.find_keyword("iterable").is_none() {
        return Some("fromkeys() missing required argument 'iterable' (pos 1)".to_string());
    }
    for kw in &call.arguments.keywords {
        match kw.arg.as_deref() {
            Some("iterable") if positional > 0 => {
                return Some(
                    "argument for fromkeys() given by name ('iterable') and position (1)"
                        .to_string(),
                );
            }
            Some("iterable" | "value") => {}
            Some(name) => {
                return Some(format!(
                    "fromkeys() got an unexpected keyword argument '{name}'"
                ));
            }
            None => return Some("dict.fromkeys keyword unpacking is unsupported".to_string()),
        }
    }
    None
}

fn fromkeys_error(message: String, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(
            Builtin::RaiseException,
            vec![Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind: exception_kind("TypeError").unwrap(),
                    message: Some(Box::new(unpacking::string(message))),
                },
            }],
        ),
    }
}
