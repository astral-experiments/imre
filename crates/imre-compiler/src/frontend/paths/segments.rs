//! Convert constructor arguments in Python call order, retaining native path objects for joins.

use super::*;

fn exact_segment(ty: Type) -> bool {
    ty.is_path() || matches!(ty, Type::Str | Type::StrLiteral(_) | Type::StrEnum(_))
}

fn segment_exception(actual: &str) -> Expr {
    Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("TypeError").unwrap(),
            message: Some(Box::new(unpacking::string(format!(
                "argument should be a str or an os.PathLike object where __fspath__ returns a str, not '{actual}'"
            )))),
        },
    }
}

fn bad_segment(value: Expr, actual: &str) -> Expr {
    Expr {
        ty: Type::Str,
        kind: ExprKind::Builtin(
            Builtin::RaiseException,
            vec![value, segment_exception(actual)],
        ),
    }
}

impl Checker<'_> {
    /// A method first evaluates its receiver; joinpath also includes it as the first segment.
    pub(super) fn path_segments(
        &mut self,
        call: &ast::ExprCall,
        ty: Type,
        receiver: Option<(Expr, bool)>,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        if let Some(first) = call.arguments.keywords.first() {
            let method = match &receiver {
                None => "__init__",
                Some((_, true)) => "joinpath",
                Some((_, false)) => "with_segments",
            };
            for keyword in &call.arguments.keywords {
                self.graph
                    .checked_keywords(self.module, std::iter::once(keyword.range()));
                if keyword
                    .arg
                    .as_ref()
                    .is_some_and(|key| key.as_str() == "self")
                {
                    self.graph
                        .checked_repeated_keyword(self.module, keyword.range());
                }
            }
            let key = first.arg.as_ref().unwrap().as_str();
            let message = if key == "self" {
                format!("PurePath.{method}() got multiple values for argument 'self'")
            } else {
                format!("PurePath.{method}() got an unexpected keyword argument '{key}'")
            };
            let mut scope = calls::ArgumentScope::default();
            if let Some((receiver, _)) = receiver {
                scope.hold(self, receiver);
            }
            return Ok(scope.finish(self.sys_type_error(call, ty, message)?));
        }
        let mut inputs = call
            .arguments
            .args
            .iter()
            .map(|arg| self.expr(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let check = inputs.iter().any(|value| !exact_segment(value.ty));
        let mut scope = calls::ArgumentScope::default();
        let base = receiver.and_then(|(value, append)| {
            let value = if check || !append {
                scope.hold(self, value)
            } else {
                value
            };
            append.then_some(value)
        });
        if check {
            inputs = inputs
                .into_iter()
                .map(|part| scope.hold(self, part))
                .collect();
        }
        let mut parts = Vec::with_capacity(inputs.len());
        for (value, at) in inputs.into_iter().zip(&call.arguments.args) {
            let value = self.constructor_part(value, at, ty)?;
            let value = if check {
                scope.hold(self, value)
            } else {
                value
            };
            parts.push((value, at));
        }
        let mut parts = parts.into_iter();
        let mut path = if let Some(base) = base {
            if parts.len() == 0 {
                // joinpath() still returns a fresh path.
                self.path_join(base, unpacking::string(""), call)?
            } else {
                base
            }
        } else if let Type::PurePath(flavor) = ty {
            operation(PathOperation::PureNew(flavor), ty, vec![])
        } else if let Some((first, at)) = parts.next() {
            self.concrete_from_part(first, at)?
        } else {
            operation(PathOperation::New, Type::Path, vec![unpacking::string("")])
        };
        for (part, at) in parts {
            path = self.path_join(path, part, at)?;
        }
        Ok(scope.finish(path))
    }

    /// A foreign pure Windows path contributes its portable spelling to a POSIX constructor.
    pub(super) fn concrete_from_part(&self, part: Expr, at: &ast::Expr) -> Result<Expr, String> {
        let text = self.segment_text(part, Type::Path, at)?;
        Ok(operation(PathOperation::New, Type::Path, vec![text]))
    }

    fn segment_text(&self, part: Expr, dest: Type, at: &ast::Expr) -> Result<Expr, String> {
        if matches!(part.ty, Type::PurePath(ir::PathFlavor::Windows))
            && !matches!(dest, Type::PurePath(ir::PathFlavor::Windows))
        {
            Ok(operation(PathOperation::AsPosix, Type::Str, vec![part]))
        } else {
            self.path_text(part, at)
        }
    }

    /// Native paths retain their raw segments; other objects must supply text through os.fspath.
    pub(super) fn constructor_part(
        &mut self,
        value: Expr,
        at: &ast::Expr,
        dest: Type,
    ) -> Result<Expr, String> {
        if value.ty.is_path() || value.ty == Type::Str {
            return Ok(value);
        }
        if matches!(value.ty, Type::StrEnum(_)) {
            return Ok(Self::enum_text(value));
        }
        if matches!(value.ty, Type::StrLiteral(_)) {
            return self.coerce(value, Type::Str, at);
        }
        if let Some((slot, test, first, second)) = self.protocol_alternatives(value.ty) {
            let mut a = self.constructor_part(first, at, dest)?;
            let mut b = self.constructor_part(second, at, dest)?;
            // A raising arm produces no value. Keep the other arm's native Path
            // so joining it can retain its raw components.
            if matches!(a.kind, ExprKind::Builtin(Builtin::RaiseException, _)) {
                a.ty = b.ty;
            } else if matches!(b.kind, ExprKind::Builtin(Builtin::RaiseException, _)) {
                b.ty = a.ty;
            } else if a.ty != b.ty {
                a = self.segment_text(a, dest, at)?;
                b = self.segment_text(b, dest, at)?;
            }
            return Ok(Expr {
                ty: a.ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    value: Box::new(Expr {
                        ty: a.ty,
                        kind: ExprKind::Conditional(Box::new(test), Box::new(a), Box::new(b)),
                    }),
                    clear: vec![slot],
                },
            });
        }
        if let Type::Class(id) = value.ty {
            if self.classes.has_concrete_subclasses(id) {
                return self.dispatch_record(id, value, at, |checker, class, receiver| {
                    checker.class_segment(class, receiver, at, dest)
                });
            }
            return self.class_segment(id, value, at, dest);
        }
        if matches!(value.ty, Type::JsonRoot | Type::JsonValue) {
            return self.path_text(value, at);
        }
        let actual = self.path_argument_type_name(value.ty);
        Ok(bad_segment(value, &actual))
    }

    fn class_segment(
        &mut self,
        class: usize,
        value: Expr,
        at: &ast::Expr,
        dest: Type,
    ) -> Result<Expr, String> {
        let name = self.classes.declarations.definitions[class].name.clone();
        if !self.classes.methods[class].contains_key("__fspath__") {
            return Ok(bad_segment(value, &name));
        }
        let protocol = self.exact_fspath(class, value, at, None)?;
        let result = self.hidden(protocol.ty);
        let failed = self.hidden(Type::Bool);
        let checked = self.constructor_part(self.local(result), at, dest)?;
        // The constructor catches TypeError from os.fspath only. In particular, a
        // valid bytes result is rejected outside this handler and names 'bytes'.
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(failed, boolean(false)),
                    Statement::Try {
                        body: vec![Statement::Assign(result, protocol)],
                        handlers: vec![ir::ExceptHandler {
                            kinds: vec![exception_kind("TypeError").unwrap()],
                            name: None,
                            body: vec![Statement::Assign(failed, boolean(true))],
                        }],
                        orelse: vec![],
                        finalbody: vec![],
                    },
                    Statement::If(
                        self.local(failed),
                        vec![Statement::Raise(Some(segment_exception(&name)))],
                        vec![],
                    ),
                ],
                value: Box::new(checked),
                clear: vec![result, failed],
            },
        })
    }
}
