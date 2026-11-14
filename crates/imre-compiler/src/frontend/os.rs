use super::*;
use crate::ir::{FdOperation, ProcessOperation};

mod copy_file_range;
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

        if op == CopyFileRange {
            return self.copy_file_range(call);
        }
        let pair = match op {
            Read => Some(("read", Type::Bytes)),
            Write => Some(("write", Type::Int)),
            TcSetPgrp => Some(("tcsetpgrp", Type::None)),
            _ => None,
        };
        if let Some((name, returns)) = pair {
            return self.posix_positional(call, name, 2, returns, |checker, scope, args| {
                let Ok([fd, data]) = <[Expr; 2]>::try_from(args) else {
                    unreachable!()
                };
                checker.graph.checked_overload(checker.module, call.range());
                for argument in &call.arguments.args {
                    checker
                        .graph
                        .checked_argument_type(checker.module, argument.range());
                }
                let index = checker.index_word(fd, &call.func, false)?;
                // Convert the descriptor before applying the second argument's protocol.
                let fd = scope.hold(
                    checker,
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(
                            Builtin::process(ProcessOperation::Fd(TransferFd)),
                            vec![index],
                        ),
                    },
                );
                let data = if op != Write {
                    checker.index_word(data, &call.func, false)?
                } else if data.ty == Type::Bytes {
                    data
                } else {
                    let owner = checker.known_nonbuffer_type(data.ty).ok_or_else(|| {
                        error(
                            checker.source,
                            call,
                            "os.write requires a known native bytes-like argument",
                        )
                    })?;
                    return Ok(posix_type_error(
                        returns,
                        format!("a bytes-like object is required, not '{owner}'"),
                    ));
                };
                Ok(Expr {
                    ty: returns,
                    kind: ExprKind::Builtin(
                        Builtin::process(ProcessOperation::Fd(op)),
                        vec![fd, data],
                    ),
                })
            });
        }
        if op == PtsName {
            return self.posix_positional(call, "ptsname", 1, Type::Str, |checker, _, args| {
                let value = args.into_iter().next().expect("one bound ptsname argument");
                let fd = match value.ty {
                    Type::Int | Type::Bool => value,
                    Type::TextIO | Type::BinaryIO => Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::FileFileno, vec![value]),
                    },
                    _ => {
                        return Err(
                            checker.object_runtime_export("os.ptsname file descriptor", call)
                        );
                    }
                };
                let mut args = vec![fd];
                if args[0].ty == Type::Bool {
                    args.push(unpacking::string(
                        checker.runtime_module_name(checker.module),
                    ));
                    args.push(unpacking::string(checker.source_filename()));
                    args.push(Expr::integer(i64::from(checker.source_line(call.start()))));
                }
                Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::process(ProcessOperation::Fd(op)), args),
                })
            });
        }
        let indexed = match op {
            Pipe2 => Some(("pipe2", self.types.tuple(vec![Type::Int; 2]))),
            TcGetPgrp => Some(("tcgetpgrp", Type::Int)),
            _ => None,
        };
        if let Some((name, returns)) = indexed {
            return self.posix_positional(call, name, 1, returns, |checker, _, args| {
                let value = args.into_iter().next().expect("one bound argument");
                checker.graph.checked_overload(checker.module, call.range());
                let index = checker.index_word(value, &call.func, false)?;
                Ok(Expr {
                    ty: returns,
                    kind: ExprKind::Builtin(
                        Builtin::process(ProcessOperation::Fd(op)),
                        vec![index],
                    ),
                })
            });
        }
        if matches!(op, Open | Unlink | Remove) {
            return self.os_path_at(call, op);
        }
        let param = |name: &str| Parameter {
            name: name.into(),
            kind: if matches!(op, Close | DeviceEncoding | Dup2) {
                ParameterKind::PositionalOrKeyword
            } else {
                ParameterKind::PositionalOnly
            },
            ty: Type::Int,
            default: None,
            sign_only: false,
        };
        let mut params = if matches!(op, Pipe | OpenPty) {
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
                inherit.default = Some(Expr::integer(1));
                params.push(inherit);
            }
            _ => {}
        }
        let args = self.arguments_with(call, &params, None, |checker, index, arg| {
            let mut value = checker.expr(arg)?;
            if op == DeviceEncoding {
                return checker.index_word(value, arg, false);
            }
            if (op == SetBlocking && index == 1) || (op == Dup2 && index == 2) {
                value = checker.truth_value(value);
            }
            if value.ty == Type::Bool {
                Ok(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::Int(None), vec![value]),
                })
            } else {
                checker.coerce(value, Type::Int, arg)
            }
        })?;
        let returns = match op {
            Close | SetBlocking | SetInheritable | Unlink | Remove => Type::None,
            TransferFd | Write | Open | Seek | Dup | Dup2 => Type::Int,
            CopyFileRange | CheckCopyCount | CheckCopyOffset => {
                unreachable!("copy_file_range has its own argument conversion")
            }
            Read => Type::Bytes,
            Pipe | Pipe2 | OpenPty => self.types.tuple(vec![Type::Int, Type::Int]),
            Isatty | GetBlocking | GetInheritable => Type::Bool,
            DeviceEncoding => self.types.optional(Type::Str),
            PtsName | TcGetPgrp | TcSetPgrp => unreachable!("terminal call handled above"),
        };
        Ok(self.ordered_builtin(Builtin::process(ProcessOperation::Fd(op)), returns, args))
    }

    fn os_path_at(&mut self, call: &ast::ExprCall, op: FdOperation) -> Result<Expr, String> {
        let dir_fd = self.types.optional(Type::Int);
        let mut params = vec![("path", Type::Str, None)];
        if op == FdOperation::Open {
            params.extend([
                ("flags", Type::Int, None),
                ("mode", Type::Int, Some(Expr::integer(511))),
            ]);
        }
        params.push(("dir_fd", dir_fd, Some(Expr::none())));
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
            Builtin::process(ProcessOperation::Fd(op)),
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
