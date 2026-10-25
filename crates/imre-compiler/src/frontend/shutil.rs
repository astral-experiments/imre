//! Bind filesystem arguments in Python evaluation order, retaining the destination's type.

use super::*;

impl Checker<'_> {
    /// Emit a native function value with the same ownership and indirect-call ABI as a def.
    fn shutil_helper(
        &mut self,
        site: &impl Ranged,
        locals: Vec<Type>,
        result: Type,
        op: ir::ProcessOperation,
        args: Vec<Expr>,
        receiver: Option<Expr>,
    ) -> Expr {
        let ty = self
            .types
            .callable(locals[usize::from(receiver.is_some())..].to_vec(), result);
        let id = self.helpers.reserve();
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                calls: HashSet::new(),
                class_reads: HashSet::new(),
                recursion_error: self.graph.modules[self.module].error(
                    site,
                    "recursive calls through this shutil operation are unsupported",
                ),
                function: ir::Function {
                    params: (0..locals.len()).collect(),
                    locals,
                    returns: result,
                    body: vec![Statement::Return(Expr {
                        ty: result,
                        kind: ExprKind::Builtin(Builtin::Process(op), args),
                    })],
                },
            },
        );
        Expr {
            ty,
            kind: ExprKind::Callable {
                function: id,
                receiver: receiver.map(Box::new),
            },
        }
    }

    fn copy_result(&self, op: ir::CopyOperation, dst: Type) -> Type {
        match op {
            ir::CopyOperation::Mode | ir::CopyOperation::Stat | ir::CopyOperation::Xattr => {
                Type::None
            }
            ir::CopyOperation::Copy | ir::CopyOperation::Copy2 if dst == Type::Path => {
                self.types.union(&[Type::Str, Type::Path]).unwrap()
            }
            _ => dst,
        }
    }

    /// Pass a copying or removal routine as a value, specializing its path types from a Callable annotation.
    pub(super) fn shutil_function_value(
        &mut self,
        op: Intrinsic,
        expected: Option<Type>,
        site: &ast::Expr,
    ) -> Result<Expr, String> {
        let params = expected
            .and_then(|ty| self.types.signature(ty))
            .map(|(args, _)| args)
            .unwrap_or_else(|| {
                if matches!(op, Intrinsic::RmTree) {
                    vec![Type::Str]
                } else {
                    vec![Type::Str; 2]
                }
            });
        let path = |ty| matches!(ty, Type::Str | Type::Bytes | Type::Path);
        let local = |i: usize| Expr {
            ty: params[i],
            kind: ExprKind::Local(i),
        };
        let flag = |v| Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(v),
        };
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let (op, result, args) = match (op, params.as_slice()) {
            (Intrinsic::Copy(op), [src, dst]) if path(*src) && path(*dst) => (
                ir::ProcessOperation::Copy(op),
                self.copy_result(op, *dst),
                vec![local(0), local(1), flag(true)],
            ),
            (Intrinsic::RmTree, [src] | [src, Type::Bool]) if path(*src) => (
                ir::ProcessOperation::RmTree,
                Type::None,
                vec![
                    self.fspath_value(local(0), site)?,
                    if params.len() == 2 {
                        local(1)
                    } else {
                        flag(false)
                    },
                    none.clone(),
                    none.clone(),
                    none,
                ],
            ),
            _ => {
                return Err(error(
                    self.source,
                    site,
                    "shutil callback requires its positional path arguments and optional rmtree ignore_errors flag",
                ));
            }
        };
        Ok(self.shutil_helper(site, params, result, op, args, None))
    }

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
        Ok(self.shutil_helper(
            call,
            locals,
            result,
            ir::ProcessOperation::IgnorePatterns,
            args,
            Some(context),
        ))
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
                (0, Type::None | Type::Path | Type::PurePath(_))
                | (2 | 3, Type::None)
                | (4, Type::None | Type::Int | Type::Str) => Ok(v),
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
        let result = self.copy_result(op, dst);
        Ok(self.ordered_builtin(
            Builtin::Process(ir::ProcessOperation::Copy(op)),
            result,
            args,
        ))
    }
}
