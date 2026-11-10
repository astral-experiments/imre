//! POSIX glob takes its pattern through splitroot; recursive glob first joins it to "**".
use super::*;

fn exception(kind: &str, message: Expr) -> Expr {
    Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind(kind).unwrap(),
            message: Some(Box::new(message)),
        },
    }
}

impl Checker<'_> {
    pub(super) fn glob_pattern(
        &mut self,
        args: &[Expr],
        pattern: Expr,
        recursive: bool,
        at: &ast::Expr,
    ) -> Result<Expr, String> {
        if let Some(selected) = self.map_path_protocol_arms(&pattern, at, |checker, arm| {
            checker.glob_pattern(args, arm, recursive, at)
        })? {
            return Ok(selected);
        }
        if let Type::Class(class) = pattern.ty
            && self.classes.has_concrete_subclasses(class)
        {
            return self.dispatch_record(class, pattern, at, |checker, _, receiver| {
                checker.glob_exact_pattern(args, receiver, recursive, at)
            });
        }
        self.glob_exact_pattern(args, pattern, recursive, at)
    }

    fn glob_exact_pattern(
        &mut self,
        args: &[Expr],
        pattern: Expr,
        recursive: bool,
        at: &ast::Expr,
    ) -> Result<Expr, String> {
        let cursor = self.types.cursor(Type::Path);
        let glob = |text: Expr| {
            let mut args = args.to_vec();
            args[1] = text;
            operation(PathOperation::Glob { recursive }, cursor, args)
        };
        let syscall = (!recursive).then_some(("_path_splitroot_ex", "path"));
        let converted = match pattern.ty {
            Type::Class(id) => self.exact_fspath(id, pattern.clone(), at, syscall)?,
            Type::Bytes | Type::None | Type::Int | Type::Bool | Type::Float => {
                self.fspath_protocol(pattern.clone(), at, None, syscall)?
            }
            _ => return Ok(glob(self.path_text(pattern, at)?)),
        };
        let text = self.glob_spelling(converted, recursive, at)?;
        if recursive {
            // genericpath._check_arg_types inspects the original operand even when
            // its __fspath__ callback raised during posixpath.join.
            let message = if pattern.ty == Type::Bytes {
                "Can't mix strings and bytes in path components".to_owned()
            } else {
                format!(
                    "join() argument must be str, bytes, or os.PathLike object, not '{}'",
                    self.path_argument_type_name(pattern.ty)
                )
            };
            let cause = Expr::absent(self.types.optional(Type::Exception));
            let error = Expr {
                ty: Type::Exception,
                kind: ExprKind::Builtin(
                    Builtin::ExceptionWithCause,
                    vec![exception("TypeError", unpacking::string(message)), cause],
                ),
            };
            let slot = self.hidden(Type::Str);
            let converted = Expr {
                ty: Type::Str,
                kind: ExprKind::Scope {
                    body: vec![Statement::Try {
                        body: vec![Statement::Assign(slot, text)],
                        handlers: vec![ir::ExceptHandler {
                            matcher: crate::ir::ExceptionMatcher::Categories(
                                ["TypeError", "AttributeError", "BytesWarning"]
                                    .map(|s| exception_kind(s).unwrap())
                                    .to_vec(),
                            ),
                            name: None,
                            body: vec![Statement::Raise(Some(error))],
                        }],
                        orelse: vec![],
                        finalbody: vec![],
                    }],
                    value: Box::new(self.local(slot)),
                    clear: vec![slot],
                },
            };
            return Ok(glob(converted));
        }
        if !matches!(pattern.ty, Type::Class(_)) {
            return Ok(glob(text));
        }
        // Glob::new raises ValueError only for a relative pattern with no parts.
        // Convert __fspath__ before this handler; render the original only after
        // leaving it, so __str__ runs in its original exception context.
        let mut scope = calls::ArgumentScope::default();
        let text = scope.hold(self, text);
        let result = self.hidden(cursor);
        let invalid = self.hidden(Type::Bool);
        let display = self.record_text(pattern, at)?;
        let repr = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::Repr, vec![display]),
        };
        let message = unpacking::concat(unpacking::string("Unacceptable pattern: "), repr);
        Ok(scope.finish(Expr {
            ty: cursor,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(invalid, Expr::boolean(false)),
                    Statement::Try {
                        body: vec![Statement::Assign(result, glob(text))],
                        handlers: vec![ir::ExceptHandler {
                            matcher: crate::ir::ExceptionMatcher::Categories(vec![
                                exception_kind("ValueError").unwrap(),
                            ]),
                            name: None,
                            body: vec![Statement::Assign(invalid, Expr::boolean(true))],
                        }],
                        orelse: vec![],
                        finalbody: vec![],
                    },
                    Statement::If(
                        self.local(invalid),
                        vec![Statement::Raise(Some(exception("ValueError", message)))],
                        vec![],
                    ),
                ],
                value: Box::new(self.local(result)),
                clear: vec![invalid, result],
            },
        }))
    }

    fn glob_spelling(
        &mut self,
        value: Expr,
        recursive: bool,
        at: &ast::Expr,
    ) -> Result<Expr, String> {
        if let Some(selected) = self.map_path_protocol_arms(&value, at, |checker, arm| {
            checker.glob_spelling(arm, recursive, at)
        })? {
            return Ok(selected);
        }
        if value.ty != Type::Bytes {
            return Ok(Self::string_value(value));
        }
        if recursive {
            // Joining the string prefix invokes startswith('/') on the bytes result.
            return Ok(calls::type_error(
                Type::Str,
                vec![value],
                "startswith first arg must be bytes or a tuple of bytes, not str".into(),
            ));
        }
        // splitroot preserves bytes. An absolute root is rejected before pathlib
        // attempts to split the relative bytes with the string separator.
        let mut scope = calls::ArgumentScope::default();
        let bytes = scope.hold(self, value);
        let first = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::BytesGet, vec![bytes.clone(), Expr::integer(0)]),
        };
        let absolute = Expr {
            ty: Type::Bool,
            kind: ExprKind::Boolean(
                true,
                vec![
                    self.truth_value(bytes),
                    pipelines::compare(Compare::Equal, first, Expr::integer(b'/')),
                ],
            ),
        };
        let error = Expr {
            ty: Type::Exception,
            kind: ExprKind::Conditional(
                Box::new(absolute),
                Box::new(exception(
                    "NotImplementedError",
                    unpacking::string("Non-relative patterns are unsupported"),
                )),
                Box::new(exception(
                    "TypeError",
                    unpacking::string("a bytes-like object is required, not 'str'"),
                )),
            ),
        };
        Ok(scope.finish(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::RaiseException, vec![error]),
        }))
    }
}
