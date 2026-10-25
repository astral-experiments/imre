//! Bind filesystem arguments in Python evaluation order, retaining the destination's type.

use super::*;

impl Checker<'_> {
    pub(super) fn shutil_ignore_patterns(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        calls::reject_unpacking(call, self.source)?;
        let values = call
            .arguments
            .args
            .iter()
            .map(|pattern| self.expr(pattern))
            .collect::<Result<Vec<_>, _>>()?;
        let ty = values.first().map_or(Type::Str, |v| v.ty);
        if !matches!(ty, Type::Str | Type::Bytes) || values.iter().any(|v| v.ty != ty) {
            return Err(error(
                self.source,
                call,
                "ignore_patterns requires string or bytes patterns of the same type",
            ));
        }
        let list = self.types.list(ty);
        let context = Expr {
            ty: self.types.tuple(vec![list]),
            kind: ExprKind::Tuple(vec![Expr {
                ty: list,
                kind: ExprKind::List(values),
            }]),
        };
        let locals = vec![context.ty, ty, list];
        let args = locals
            .iter()
            .enumerate()
            .map(|(i, &ty)| Expr {
                ty,
                kind: ExprKind::Local(i),
            })
            .collect();
        let result = self.types.set(ty);
        let id = self.helpers.reserve();
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                calls: HashSet::new(),
                class_reads: HashSet::new(),
                recursion_error: self.graph.modules[self.module].error(
                    call,
                    "recursive calls through ignore_patterns are unsupported",
                ),
                function: ir::Function {
                    params: vec![0, 1, 2],
                    locals,
                    returns: result,
                    body: vec![Statement::Return(Expr {
                        ty: result,
                        kind: ExprKind::Builtin(
                            Builtin::Process(ir::ProcessOperation::IgnorePatterns),
                            args,
                        ),
                    })],
                },
            },
        );
        Ok(Expr {
            ty: self.types.callable(vec![ty, list], result),
            kind: ExprKind::Callable {
                function: id,
                receiver: Some(Box::new(context)),
            },
        })
    }

    pub(super) fn shutil_copytree(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let boolean = |v| Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(v),
        };
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let params = [
            ("src", Type::Str, None),
            ("dst", Type::Str, None),
            ("symlinks", Type::Bool, Some(boolean(false))),
            ("ignore", Type::None, Some(none)),
            ("copy_function", Type::Bool, Some(boolean(true))),
            ("ignore_dangling_symlinks", Type::Bool, Some(boolean(false))),
            ("dirs_exist_ok", Type::Bool, Some(boolean(false))),
        ]
        .map(|(name, ty, default)| Parameter {
            name: name.into(),
            ty,
            kind: ParameterKind::PositionalOrKeyword,
            default,
            sign_only: false,
        });
        let args = self.arguments_with(call, &params, None, |checker, i, expr| match i {
            0 | 1 => {
                let value = checker.expr(expr)?;
                if matches!(value.ty, Type::Str | Type::Bytes | Type::Path) {
                    Ok(value)
                } else {
                    Err(error(
                        checker.source,
                        expr,
                        "expected str, bytes or pathlib.Path",
                    ))
                }
            }
            2 | 5 | 6 => checker.expr_expected(expr, Type::Bool),
            3 => checker.expr(expr),
            4 => match checker.resolve(expr) {
                Some(Binding::Intrinsic(Intrinsic::Copy(ir::CopyOperation::Copy))) => {
                    Ok(boolean(false))
                }
                Some(Binding::Intrinsic(Intrinsic::Copy(ir::CopyOperation::Copy2))) => {
                    Ok(boolean(true))
                }
                _ => {
                    let value = checker.expr(expr)?;
                    if checker.types.signature(value.ty).is_none() {
                        return Err(error(
                            checker.source,
                            expr,
                            "copy_function must accept source and destination paths",
                        ));
                    }
                    Ok(value)
                }
            },
            _ => unreachable!(),
        })?;
        let path_type = |arg: usize| match args.values[args.order[arg]].ty {
            Type::Path => Type::Str,
            other => other,
        };
        let filter = &args.values[args.order[3]];
        if filter.ty != Type::None {
            let ty = path_type(0);
            let Some((params, result)) = self.types.signature(filter.ty) else {
                return Err(error(
                    self.source,
                    call,
                    "ignore must accept a directory and its entry names",
                ));
            };
            if params != [ty, self.types.list(ty)]
                || self
                    .types
                    .element(result)
                    .or_else(|| self.types.set_element(result))
                    != Some(ty)
            {
                return Err(error(
                    self.source,
                    call,
                    "ignore must accept a path and a list of names and return a list or set of the same names",
                ));
            }
        }
        let copy = &args.values[args.order[4]];
        if let Some((params, _)) = self.types.signature(copy.ty)
            && params != [path_type(0), path_type(1)]
        {
            return Err(error(
                self.source,
                call,
                "copy_function parameters must match the two filesystem path types",
            ));
        }
        let path = self.types.union(&[Type::Str, Type::Bytes]).unwrap();
        let issue = self.types.tuple(vec![path, path, Type::Str]);
        let result = args.values[args.order[1]].ty;
        Ok(self.ordered_builtin(
            Builtin::Process(ir::ProcessOperation::CopyTree(issue)),
            result,
            args,
        ))
    }

    pub(super) fn shutil_rmtree(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let params = [
            ("path", Type::Str, None),
            (
                "ignore_errors",
                Type::Bool,
                Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(false),
                }),
            ),
            ("onerror", Type::None, Some(none.clone())),
            ("onexc", Type::None, Some(none.clone())),
            ("dir_fd", Type::Int, Some(none)),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (name, ty, default))| Parameter {
            name: name.into(),
            ty,
            kind: if i < 3 {
                ParameterKind::PositionalOrKeyword
            } else {
                ParameterKind::KeywordOnly
            },
            default,
            sign_only: false,
        })
        .collect::<Vec<_>>();
        let args = self.arguments_with(call, &params, None, |checker, i, expr| {
            if i == 1 {
                return checker.expr_expected(expr, Type::Bool);
            }
            let v = checker.expr(expr)?;
            match (i, v.ty) {
                (0, Type::None) | (2 | 3, Type::None) | (4, Type::None | Type::Int | Type::Str) => {
                    Ok(v)
                }
                (0, _) => checker.fspath_value(v, expr),
                (4, _) if checker.types.payload(v.ty) == Some(Type::Int) => Ok(v),
                (4, _) => Err(error(
                    checker.source,
                    expr,
                    "rmtree dir_fd requires an integer or None",
                )),
                _ => Err(error(
                    checker.source,
                    expr,
                    "rmtree error callbacks require a supported callable",
                )),
            }
        })?;
        Ok(self.ordered_builtin(
            Builtin::Process(ir::ProcessOperation::RmTree),
            Type::None,
            args,
        ))
    }

    pub(super) fn shutil_copy(
        &mut self,
        op: ir::CopyOperation,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let params = [
            ("src", Type::Str, ParameterKind::PositionalOrKeyword, None),
            ("dst", Type::Str, ParameterKind::PositionalOrKeyword, None),
            (
                "follow_symlinks",
                Type::Bool,
                ParameterKind::KeywordOnly,
                Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(true),
                }),
            ),
        ]
        .map(|(name, ty, kind, default)| Parameter {
            name: name.into(),
            ty,
            kind,
            default,
            sign_only: false,
        });
        let args = self.arguments_with(call, &params, None, |checker, i, expr| {
            if i == 2 {
                return checker.expr_expected(expr, Type::Bool);
            }
            let value = checker.expr(expr)?;
            if matches!(value.ty, Type::Str | Type::Bytes | Type::Path) {
                Ok(value)
            } else {
                Err(error(
                    checker.source,
                    expr,
                    "expected str, bytes or pathlib.Path",
                ))
            }
        })?;
        let dst = args.values[args.order[1]].ty;
        let result = match op {
            ir::CopyOperation::Mode | ir::CopyOperation::Stat | ir::CopyOperation::Xattr => {
                Type::None
            }
            ir::CopyOperation::Copy | ir::CopyOperation::Copy2 if dst == Type::Path => {
                self.types.union(&[Type::Str, Type::Path]).unwrap()
            }
            _ => dst,
        };
        Ok(self.ordered_builtin(
            Builtin::Process(ir::ProcessOperation::Copy(op)),
            result,
            args,
        ))
    }
}
