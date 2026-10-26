//! Native path protocol used by os.fspath, filesystem codecs, and POSIX normcase.

use super::*;

mod descriptor;
pub(super) use descriptor::protocol_call;

impl Checker<'_> {
    /// Bind the public function before running the user's path protocol.
    pub(super) fn public_fspath(
        &mut self,
        call: &ast::ExprCall,
        operation: Intrinsic,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = call.arguments.args.as_ref();
        let keywords = call.arguments.keywords.as_ref();
        let (name, parameter, result_type) = match operation {
            Intrinsic::FsPath => ("fspath", "path", Type::Str),
            Intrinsic::PosixNormcase => ("normcase", "s", Type::Str),
            Intrinsic::FsDecode => ("_fscodec.<locals>.fsdecode", "filename", Type::Str),
            Intrinsic::FsEncode => ("_fscodec.<locals>.fsencode", "filename", Type::Bytes),
            _ => unreachable!(),
        };
        let path = match (args, keywords) {
            ([path], []) => path,
            ([], [kw]) if kw.arg.as_ref().is_some_and(|n| n.as_str() == parameter) => &kw.value,
            _ => {
                let count = args.len() + keywords.len();
                let message = if operation == Intrinsic::FsPath {
                    match (args.is_empty(), count) {
                        (true, 0 | 1) => {
                            "fspath() missing required argument 'path' (pos 1)".to_owned()
                        }
                        (true, _) => {
                            format!("fspath() takes at most 1 keyword argument ({count} given)")
                        }
                        _ => format!("fspath() takes at most 1 argument ({count} given)"),
                    }
                } else if let Some(key) = keywords
                    .iter()
                    .filter_map(|kw| kw.arg.as_ref())
                    .find(|n| n.as_str() != parameter || !args.is_empty())
                {
                    if key.as_str() == parameter {
                        format!("{name}() got multiple values for argument '{parameter}'")
                    } else {
                        format!("{name}() got an unexpected keyword argument '{key}'")
                    }
                } else if args.is_empty() {
                    format!("{name}() missing 1 required positional argument: '{parameter}'")
                } else {
                    format!(
                        "{name}() takes 1 positional argument but {} were given",
                        args.len()
                    )
                };
                self.graph.checked_overload(self.module, call.range());
                if args.is_empty() && !keywords.iter().any(|k| k.arg.as_deref() == Some(parameter))
                {
                    self.graph
                        .checked_missing_argument(self.module, call.range());
                }
                for arg in args.iter().skip(1) {
                    self.graph.checked_excess_argument(self.module, arg.range());
                }
                for key in keywords {
                    if key.arg.as_deref() == Some(parameter) {
                        self.graph
                            .checked_repeated_keyword(self.module, key.range());
                    } else {
                        self.graph
                            .checked_keywords(self.module, std::iter::once(key.range()));
                    }
                }
                return self.sys_type_error(call, result_type, message);
            }
        };
        let value = self.expr(path)?;
        let result = self.fspath(value, path)?;
        // The stubs omit invalid descriptors. The native operation above checks
        // their binding and return type, so its own outer overload is accounted for.
        self.graph.checked_overload(self.module, call.range());
        if matches!(operation, Intrinsic::FsDecode | Intrinsic::FsEncode) {
            self.graph.checked_argument_type(self.module, path.range());
            self.filesystem_codec(result, path, result_type)
        } else {
            Ok(result)
        }
    }

    /// Preserve a matching path owner and convert the other arm with Python's startup codec.
    fn filesystem_codec(
        &mut self,
        mut path: Expr,
        at: &ast::Expr,
        result: Type,
    ) -> Result<Expr, String> {
        if matches!(path.kind, ExprKind::Builtin(Builtin::RaiseException, _)) {
            path.ty = result;
            return Ok(path);
        }
        if let Some(selected) = self.map_path_protocol_arms(&path, at, |checker, arm| {
            checker.filesystem_codec(arm, at, result)
        })? {
            return Ok(selected);
        }
        let op = match (path.ty, result) {
            (Type::Bytes, Type::Str) => Builtin::CodecDecode,
            (Type::Str | Type::StrLiteral(_) | Type::StrEnum(_), Type::Bytes) => {
                Builtin::CodecEncode
            }
            _ => return Ok(path),
        };
        let path = Self::string_value(path);
        let slot = self.hidden(path.ty);
        let setting = |query| Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::SysEncoding(query), vec![]),
        };
        let codec = Expr {
            ty: Type::CodecInfo,
            kind: ExprKind::Builtin(
                Builtin::CodecLookup,
                vec![setting(ir::SysEncoding::Filesystem)],
            ),
        };
        Ok(Expr {
            ty: result,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, path)],
                value: Box::new(Expr {
                    ty: result,
                    kind: ExprKind::Builtin(
                        op,
                        vec![
                            codec,
                            self.local(slot),
                            setting(ir::SysEncoding::FilesystemErrors),
                        ],
                    ),
                }),
                clear: vec![slot],
            },
        })
    }

    /// Resolve one path-like value, preserving its return type and any user exception.
    pub(super) fn fspath(&mut self, value: Expr, at: &ast::Expr) -> Result<Expr, String> {
        self.fspath_protocol(value, at, None, None)
    }

    /// OS path converters name the operation and parameter if the protocol is missing.
    pub(super) fn syscall_fspath(
        &mut self,
        value: Expr,
        at: &ast::Expr,
        operation: &str,
        parameter: &str,
    ) -> Result<Expr, String> {
        self.fspath_protocol(value, at, None, Some((operation, parameter)))
    }

    /// A path protocol result must be a string or bytes; do not recursively call
    /// another object's protocol. Nullable and tagged inputs are evaluated once.
    pub(super) fn fspath_protocol(
        &mut self,
        value: Expr,
        at: &ast::Expr,
        returned_by: Option<usize>,
        syscall: Option<(&str, &str)>,
    ) -> Result<Expr, String> {
        if matches!(
            value.ty,
            Type::Str | Type::Bytes | Type::StrLiteral(_) | Type::StrEnum(_)
        ) || self
            .types
            .members(value.ty)
            .is_some_and(|members| members == [Type::Str, Type::Bytes])
        {
            // OS consumers need the string spelling; os.fspath itself preserves subclasses.
            return Ok(if syscall.is_some() {
                Self::string_value(value)
            } else {
                value
            });
        }
        if returned_by.is_none() && matches!(value.ty, Type::Path | Type::PurePath(_)) {
            return self.fspath_value(value, at);
        }
        if let Some(selected) = self.map_path_protocol_arms(&value, at, |checker, arm| {
            checker.fspath_protocol(arm, at, returned_by, syscall)
        })? {
            return Ok(selected);
        }
        if returned_by.is_none()
            && let Type::Class(id) = value.ty
        {
            if self.classes.has_concrete_subclasses(id) {
                return self.dispatch_record(id, value, at, |checker, class, receiver| {
                    checker.exact_fspath(class, receiver, at, syscall)
                });
            }
            return self.exact_fspath(id, value, at, syscall);
        }
        Ok(self.fspath_type_error(value, returned_by, syscall))
    }

    /// Check one actual protocol operand or descriptor result, including None.
    pub(super) fn map_path_protocol_arms(
        &mut self,
        value: &Expr,
        at: &ast::Expr,
        mut check: impl FnMut(&mut Self, Expr) -> Result<Expr, String>,
    ) -> Result<Option<Expr>, String> {
        let Some((slot, test, first, second)) = self.protocol_alternatives(value.ty) else {
            return Ok(None);
        };
        let mut left = check(self, first)?;
        let mut right = check(self, second)?;
        // A branch that only raises supplies no Python value to the join.
        if matches!(left.kind, ExprKind::Builtin(Builtin::RaiseException, _)) {
            left.ty = right.ty;
        } else if matches!(right.kind, ExprKind::Builtin(Builtin::RaiseException, _)) {
            right.ty = left.ty;
        }
        let ty = self.types.join(left.ty, right.ty).ok_or_else(|| {
            error(
                self.source,
                at,
                "path protocol branches require compatible native return types",
            )
        })?;
        Ok(Some(Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, value.clone())],
                value: Box::new(Expr {
                    ty,
                    kind: ExprKind::Conditional(
                        Box::new(test),
                        Box::new(self.coerce(left, ty, at)?),
                        Box::new(self.coerce(right, ty, at)?),
                    ),
                }),
                clear: vec![slot],
            },
        }))
    }

    pub(super) fn exact_fspath(
        &mut self,
        id: usize,
        receiver: Expr,
        at: &ast::Expr,
        syscall: Option<(&str, &str)>,
    ) -> Result<Expr, String> {
        let Some(method) = self.classes.methods[id].get("__fspath__").cloned() else {
            return Ok(self.fspath_type_error(receiver, None, syscall));
        };
        let result = self.call_fspath_descriptor(id, receiver, &method, at, syscall)?;
        self.fspath_protocol(result, at, Some(id), syscall)
    }

    fn fspath_type_error(
        &self,
        evaluated: Expr,
        returned_by: Option<usize>,
        syscall: Option<(&str, &str)>,
    ) -> Expr {
        let actual = self.path_argument_type_name(evaluated.ty);
        let message = match (returned_by, syscall) {
            (Some(id), _) => format!(
                "expected {}.__fspath__() to return str or bytes, not {actual}",
                self.classes.declarations.definitions[id].name
            ),
            (None, Some((operation, parameter))) => format!(
                "{operation}: {parameter} should be string, bytes or os.PathLike, not {actual}"
            ),
            (None, None) => format!("expected str, bytes or os.PathLike object, not {actual}"),
        };
        Self::path_operand_type_error(evaluated, Type::Str, message)
    }

    /// Evaluate the operand before raising an error about its actual Python type.
    fn path_operand_type_error(evaluated: Expr, result: Type, message: String) -> Expr {
        let exception = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(message))),
            },
        };
        Expr {
            ty: result,
            kind: ExprKind::Builtin(Builtin::RaiseException, vec![evaluated, exception]),
        }
    }
    /// Use the Python object type name when reporting an invalid filesystem operand.
    pub(super) fn path_argument_type_name(&self, ty: Type) -> String {
        match ty {
            Type::None => "NoneType".to_owned(),
            Type::List(_) | Type::ListInt | Type::ByteOutput => "list".to_owned(),
            Type::Dict(_) => "dict".to_owned(),
            Type::Set(_) => "set".to_owned(),
            Type::Tuple(_) => "tuple".to_owned(),
            Type::Class(id) => self.classes.declarations.definitions[id].name.clone(),
            ty => self.type_name(ty),
        }
    }
}
