//! Bind filesystem arguments in Python evaluation order, retaining the destination's type.

use super::*;

mod registry;

impl Checker<'_> {
    pub(super) fn shutil_archive_formats(
        &mut self,
        call: &ast::ExprCall,
        unpack: bool,
    ) -> Result<Expr, String> {
        self.arguments(call, &[], None)?;
        let mut fields = vec![Type::Str];
        if unpack {
            fields.push(self.types.list(Type::Str));
        }
        fields.push(Type::Str);
        let row = self.types.tuple(fields);
        Ok(Expr {
            ty: self.types.list(row),
            kind: ExprKind::Builtin(
                Builtin::Process(ir::ProcessOperation::ArchiveFormats { unpack, row }),
                vec![],
            ),
        })
    }

    /// Preserve argument evaluation order before the archive library opens any path.
    pub(super) fn shutil_archive(
        &mut self,
        call: &ast::ExprCall,
        create: bool,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let opt = self.types.optional(Type::Str);
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let absent = self.coerce(none.clone(), opt, call)?;
        let zero = Expr {
            ty: Type::Int,
            kind: ExprKind::Int("0".into()),
        };
        let (operation, returns, specs) = if create {
            (
                ir::ProcessOperation::MakeArchive,
                Type::Str,
                vec![
                    ("base_name", Type::Str, None),
                    ("format", Type::Str, None),
                    ("root_dir", opt, Some(absent.clone())),
                    ("base_dir", opt, Some(absent.clone())),
                    ("verbose", Type::Int, Some(zero.clone())),
                    ("dry_run", Type::Int, Some(zero)),
                    ("owner", opt, Some(absent.clone())),
                    ("group", opt, Some(absent)),
                    ("logger", Type::None, Some(none)),
                ],
            )
        } else {
            (
                ir::ProcessOperation::UnpackArchive,
                Type::None,
                vec![
                    ("filename", Type::Str, None),
                    ("extract_dir", opt, Some(absent.clone())),
                    ("format", opt, Some(absent.clone())),
                    ("filter", opt, Some(absent)),
                ],
            )
        };
        let params: Vec<_> = specs
            .into_iter()
            .map(|(name, ty, default)| Parameter {
                name: name.into(),
                ty,
                default,
                sign_only: false,
                kind: if name == "filter" {
                    ParameterKind::KeywordOnly
                } else {
                    ParameterKind::PositionalOrKeyword
                },
            })
            .collect();
        let args = self.arguments_with(call, &params, None, |checker, i, expr| {
            if i == 0 && create {
                let v = checker.expr(expr)?;
                if matches!(v.ty, Type::Path | Type::PurePath(_)) {
                    return Ok(v);
                }
                return checker.path_text(v, expr);
            }
            if i == 0 {
                let v = checker.expr(expr)?;
                return checker.path_text(v, expr);
            }
            if (create && matches!(i, 2 | 3)) || (!create && i == 1) {
                let v = checker.expr(expr)?;
                if create && i == 2 {
                    let ty = checker.types.payload(v.ty).unwrap_or(v.ty);
                    if ty.is_path()
                        || checker.types.members(ty).is_some_and(|members| {
                            members
                                .iter()
                                .all(|ty| matches!(ty, Type::Str | Type::Path))
                        })
                    {
                        return Ok(v);
                    }
                }
                let v = if v.ty == Type::None || v.ty == opt {
                    v
                } else {
                    checker.path_text(v, expr)?
                };
                return checker.coerce(v, opt, expr);
            }
            if create && i == 5 {
                let value = checker.expr(expr)?;
                if matches!(value.ty, Type::Int | Type::Bool) {
                    return Ok(value);
                }
                return Err(error(
                    checker.source,
                    expr,
                    "make_archive dry_run requires a native integer or boolean",
                ));
            }
            checker.expr_expected(expr, params[i].ty)
        })?;
        Ok(self.ordered_builtin(Builtin::Process(operation), returns, args))
    }

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
                source_frame: false,
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

    /// Pass a filesystem operation as a value, specializing paths from a Callable annotation.
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
            (Intrinsic::Move, [src, dst]) if path(*src) && path(*dst) => (
                ir::ProcessOperation::Move(self.shutil_tree_issue()),
                self.move_result(*dst),
                vec![local(0), local(1), flag(true)],
            ),
            (Intrinsic::DestInSrc, [src, dst]) if path(*src) && path(*dst) => (
                ir::ProcessOperation::DestInSrc,
                Type::Bool,
                vec![local(0), local(1)],
            ),
            (Intrinsic::Copy(op), [src, dst]) if path(*src) && path(*dst) => (
                ir::ProcessOperation::Copy(op),
                self.copy_result(op, *dst),
                vec![local(0), local(1), flag(true)],
            ),
            (Intrinsic::RmTree, [src] | [src, Type::Bool])
                if matches!(*src, Type::Str | Type::Bytes) || src.is_path() =>
            {
                (
                    ir::ProcessOperation::RmTree,
                    Type::None,
                    vec![
                        local(0),
                        if params.len() == 2 {
                            local(1)
                        } else {
                            flag(false)
                        },
                        none.clone(),
                        none.clone(),
                        none,
                    ],
                )
            }
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

    fn shutil_path_arg(&mut self, expr: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(expr)?;
        if matches!(value.ty, Type::Str | Type::Bytes | Type::Path) {
            Ok(value)
        } else {
            Err(error(
                self.source,
                expr,
                "expected str, bytes or pathlib.Path",
            ))
        }
    }

    /// A builtin copier needs only the metadata flag; a custom copier keeps its callable owner.
    fn shutil_copy_function(&mut self, expr: &ast::Expr) -> Result<Expr, String> {
        let stat = match self.resolve(expr) {
            Some(Binding::Intrinsic(Intrinsic::Copy(ir::CopyOperation::Copy))) => false,
            Some(Binding::Intrinsic(Intrinsic::Copy(ir::CopyOperation::Copy2))) => true,
            _ => {
                let value = self.expr(expr)?;
                let name = match value.ty {
                    Type::None => Some("NoneType"),
                    Type::Bool => Some("bool"),
                    Type::Int => Some("int"),
                    Type::Float => Some("float"),
                    Type::Str | Type::StrLiteral(_) => Some("str"),
                    Type::Bytes => Some("bytes"),
                    _ => None,
                };
                if let Some(name) = name {
                    // A successful rename, an empty directory, or a symlink need
                    // never invoke the copier. Evaluate the supplied object now.
                    return Ok(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Scope {
                            body: vec![Statement::Expr(value)],
                            clear: vec![],
                            value: Box::new(unpacking::string(format!(
                                "'{name}' object is not callable"
                            ))),
                        },
                    });
                }
                if self.types.signature(value.ty).is_none() {
                    return Err(error(
                        self.source,
                        expr,
                        "copy_function must accept source and destination paths",
                    ));
                }
                return Ok(value);
            }
        };
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(stat),
        })
    }

    fn shutil_tree_issue(&self) -> Type {
        let path = self.types.union(&[Type::Str, Type::Bytes]).unwrap();
        self.types.tuple(vec![path, path, Type::Str])
    }

    /// A rename to the same directory returns None; a joined destination is text.
    fn move_result(&self, dst: Type) -> Type {
        if dst == Type::Path {
            self.types
                .union(&[Type::Str, Type::Path, Type::None])
                .unwrap()
        } else {
            self.types.optional(dst)
        }
    }

    pub(super) fn shutil_move(
        &mut self,
        call: &ast::ExprCall,
        inside: bool,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let params = [
            ("src", Type::Str, None),
            ("dst", Type::Str, None),
            (
                "copy_function",
                Type::Bool,
                Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(true),
                }),
            ),
        ]
        .map(|(name, ty, default)| Parameter {
            name: name.into(),
            ty,
            kind: ParameterKind::PositionalOrKeyword,
            default,
            sign_only: false,
        });
        let args = self.arguments_with(
            call,
            &params[..if inside { 2 } else { 3 }],
            None,
            |checker, i, expr| {
                if i < 2 {
                    checker.shutil_path_arg(expr)
                } else {
                    checker.shutil_copy_function(expr)
                }
            },
        )?;
        let dst = args.values[args.order[1]].ty;
        let (op, result) = if inside {
            (ir::ProcessOperation::DestInSrc, Type::Bool)
        } else {
            if let Some((params, _)) = self.types.signature(args.values[args.order[2]].ty) {
                let accepts = |param: Type, actual: Type| {
                    let accepts = |ty| {
                        param == ty || self.types.members(param).is_some_and(|v| v.contains(&ty))
                    };
                    // File fallback passes the original Path; recursion and joining yield str.
                    accepts(actual) && (actual != Type::Path || accepts(Type::Str))
                };
                if params.len() != 2
                    || !accepts(params[0], args.values[args.order[0]].ty)
                    || !accepts(params[1], dst)
                {
                    return Err(error(
                        self.source,
                        call,
                        "move copy_function must accept both original and joined paths",
                    ));
                }
            }
            let result = self.move_result(dst);
            (ir::ProcessOperation::Move(self.shutil_tree_issue()), result)
        };
        Ok(self.ordered_builtin(Builtin::Process(op), result, args))
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
            0 | 1 => checker.shutil_path_arg(expr),
            2 | 5 | 6 => checker.expr_expected(expr, Type::Bool),
            3 => checker.expr(expr),
            4 => checker.shutil_copy_function(expr),
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
        let issue = self.shutil_tree_issue();
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
