//! Resolve the protocol method requested by the bundled HTTP dispatcher.
use super::*;

impl Checker<'_> {
    /// Look up a bound do_* callable on the actual handler layout. The caller's
    /// Python try/except implements hasattr's AttributeError suppression.
    pub(super) fn request_method_lookup(
        &mut self,
        call: &ast::ExprCall,
    ) -> Result<Option<Expr>, String> {
        if !self.unshadowed_builtin(&call.func, "getattr") || !call.arguments.keywords.is_empty() {
            return Ok(None);
        }
        let [owner, key] = call.arguments.args.as_ref() else {
            return Ok(None);
        };
        if !crate::modules::http_command_key(key) {
            return Ok(None);
        }
        let receiver = self.expr(owner)?;
        let Type::Class(base) = receiver.ty else {
            return Ok(None);
        };
        if !self
            .graph
            .is_http_handler(self.classes.declarations.definitions[base].definition)
        {
            return Ok(None);
        }
        let mut scope = calls::ArgumentScope::default();
        let receiver = scope.hold(self, receiver);
        let key = self.expr_expected(key, Type::Str)?;
        let key = scope.hold(self, key);
        let result = self.types.callable(vec![], Type::None);
        let value = self.dispatch_record(base, receiver, call, |checker, class, receiver| {
            let declared = &checker.classes.declarations.definitions[class];
            if declared.fields.iter().any(|field| field.name.starts_with("do_"))
                || declared.static_bindings.keys().any(|name| name.starts_with("do_"))
            {
                return Err(error(
                    checker.source, call, "HTTP command attributes require an instance method",
                ));
            }
            let methods = &checker.classes.methods[class];
            let mut fallback = if let Some(sig) = methods.get("__getattr__").cloned() {
                if sig.params.len() != 2 || sig.params[1].ty != Type::Str || sig.returns != result {
                    return Err(error(
                        checker.source, call,
                        "an HTTP handler's dynamic command lookup requires a bound zero-argument Callable",
                    ));
                }
                checker.calls.insert(sig.id);
                Expr {
                    ty: result,
                    kind: ExprKind::Call(
                        sig.id, ir::CallArgs::positional(vec![receiver.clone(), key.clone()]),
                    ),
                }
            } else {
                let exception = Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind("AttributeError").unwrap(),
                        message: Some(Box::new(key.clone())),
                    },
                };
                Expr {
                    ty: result,
                    kind: ExprKind::Builtin(Builtin::RaiseException, vec![exception]),
                }
            };
            let mut commands: Vec<_> = methods.iter()
                .filter(|(name, _)| name.starts_with("do_"))
                .map(|(name, sig)| (name.clone(), sig.clone()))
                .collect();
            commands.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, sig) in commands {
                let bound = checker.callable_value(&sig, Some(receiver.clone()), Some(result), call)?;
                checker.expect(bound.ty, result, call)?;
                let test = Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Compare(
                        vec![key.clone(), unpacking::string(name)], vec![ir::Compare::Equal],
                    ),
                };
                fallback = Expr {
                    ty: result,
                    kind: ExprKind::Conditional(Box::new(test), Box::new(bound), Box::new(fallback)),
                };
            }
            Ok(fallback)
        })?;
        Ok(Some(scope.finish(value)))
    }
}
