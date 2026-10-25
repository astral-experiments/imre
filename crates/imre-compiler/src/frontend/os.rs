use super::*;
use crate::ir::{FdOperation, ProcessOperation};

mod credentials;

impl Checker<'_> {
    /// Evaluate every argument before binding a fixed positional POSIX signature.
    fn posix_positional(
        &mut self,
        call: &ast::ExprCall,
        name: &str,
        size: usize,
        returns: Type,
        lower: impl FnOnce(&mut Self, &mut calls::ArgumentScope, Vec<Expr>) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        let mut scope = calls::ArgumentScope::default();
        let mut supplied = Vec::new();
        let mut excess = Vec::new();
        for arg in &call.arguments.args {
            let before = supplied.len();
            if let ast::Expr::Starred(spread) = arg {
                supplied.extend(scope.spread(self, spread)?);
            } else {
                let value = self.expr(arg)?;
                supplied.push(scope.hold(self, value));
            }
            if supplied.len() > size && supplied.len() > before {
                excess.push(arg.range());
            }
        }
        for keyword in &call.arguments.keywords {
            if keyword.arg.is_none() {
                return Err(error(
                    self.source,
                    keyword,
                    "expanded keyword arguments are unsupported",
                ));
            }
            let value = self.expr(&keyword.value)?;
            scope.hold(self, value);
        }
        let count = supplied.len();
        let message = if !call.arguments.keywords.is_empty() {
            Some(format!("posix.{name}() takes no keyword arguments"))
        } else if size == count {
            None
        } else {
            Some(match size {
                0 => format!("posix.{name}() takes no arguments ({count} given)"),
                1 => format!("posix.{name}() takes exactly one argument ({count} given)"),
                _ => format!("{name} expected {size} arguments, got {count}"),
            })
        };
        if let Some(message) = message {
            // Mark only the bad binding sites; errors inside evaluated operands remain visible.
            if count < size {
                self.graph
                    .checked_missing_argument(self.module, call.range());
            }
            for range in excess {
                self.graph.checked_excess_argument(self.module, range);
            }
            for keyword in &call.arguments.keywords {
                self.graph
                    .checked_positional_keyword(self.module, keyword.range());
            }
            self.graph.checked_keywords(
                self.module,
                call.arguments.keywords.iter().map(Ranged::range),
            );
            return Ok(scope.finish(posix_type_error(returns, message)));
        }
        let value = lower(self, &mut scope, supplied)?;
        Ok(scope.finish(value))
    }

    pub(super) fn fd_call(
        &mut self,
        call: &ast::ExprCall,
        op: FdOperation,
    ) -> Result<Expr, String> {
        use FdOperation::*;

        if op == Pipe2 {
            let returns = self.types.tuple(vec![Type::Int; 2]);
            return self.posix_positional(call, "pipe2", 1, returns, |checker, _, args| {
                let flags = args.into_iter().next().expect("one bound pipe2 argument");
                checker.graph.checked_overload(checker.module, call.range());
                let flags = checker.index_word(flags, &call.func, false)?;
                Ok(Expr {
                    ty: returns,
                    kind: ExprKind::Builtin(
                        Builtin::Process(ProcessOperation::Fd(Pipe2)),
                        vec![flags],
                    ),
                })
            });
        }
        if matches!(op, Open | Unlink | Remove) {
            return self.os_path_at(call, op);
        }
        let param = |name: &str| Parameter {
            name: name.into(),
            kind: if matches!(op, Close | Dup2) {
                ParameterKind::PositionalOrKeyword
            } else {
                ParameterKind::PositionalOnly
            },
            ty: Type::Int,
            default: None,
            sign_only: false,
        };
        let mut params = if op == Pipe {
            vec![]
        } else {
            vec![param("fd")]
        };
        match op {
            Seek => params.extend([param("position"), param("whence")]),
            SetBlocking => params.push(param("blocking")),
            SetInheritable => params.push(param("inheritable")),
            Dup2 => {
                params.push(param("fd2"));
                let mut inherit = param("inheritable");
                inherit.default = Some(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Int("1".into()),
                });
                params.push(inherit);
            }
            _ => {}
        }
        let args = self.arguments_with(call, &params, None, |checker, index, arg| {
            let mut value = checker.expr(arg)?;
            if (op == SetBlocking && index == 1) || (op == Dup2 && index == 2) {
                value = checker.truth_value(value);
            }
            if value.ty == Type::Bool {
                Ok(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                })
            } else {
                checker.coerce(value, Type::Int, arg)
            }
        })?;
        let returns = match op {
            Close | SetBlocking | SetInheritable | Unlink | Remove => Type::None,
            Open | Seek | Dup | Dup2 => Type::Int,
            Pipe | Pipe2 => self.types.tuple(vec![Type::Int, Type::Int]),
            Isatty | GetBlocking | GetInheritable => Type::Bool,
        };
        Ok(self.ordered_builtin(Builtin::Process(ProcessOperation::Fd(op)), returns, args))
    }

    fn os_path_at(&mut self, call: &ast::ExprCall, op: FdOperation) -> Result<Expr, String> {
        let dir_fd = self.types.optional(Type::Int);
        let mut params = vec![("path", Type::Str, None)];
        if op == FdOperation::Open {
            params.extend([
                ("flags", Type::Int, None),
                (
                    "mode",
                    Type::Int,
                    Some(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Int("511".into()),
                    }),
                ),
            ]);
        }
        params.push((
            "dir_fd",
            dir_fd,
            Some(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            }),
        ));
        let params: Vec<_> = params
            .into_iter()
            .map(|(name, ty, default)| Parameter {
                name: name.into(),
                ty,
                default,
                sign_only: false,
                kind: if name == "dir_fd" {
                    ParameterKind::KeywordOnly
                } else {
                    ParameterKind::PositionalOrKeyword
                },
            })
            .collect();
        let args = self.arguments_with(call, &params, None, |checker, index, arg| {
            if index == 0 {
                checker.fspath_argument(arg)
            } else {
                checker.expr_expected(arg, params[index].ty)
            }
        })?;
        Ok(self.ordered_builtin(
            Builtin::Process(ProcessOperation::Fd(op)),
            if op == FdOperation::Open {
                Type::Int
            } else {
                Type::None
            },
            args,
        ))
    }
}

fn posix_type_error(ty: Type, message: String) -> Expr {
    let exception = Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("TypeError").unwrap(),
            message: Some(Box::new(unpacking::string(message))),
        },
    };
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::RaiseException, vec![exception]),
    }
}
