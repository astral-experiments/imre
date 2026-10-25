//! Sealed pathlib operations retain concrete path storage and explicit UTF-8 I/O.

use super::*;
use crate::ir::PathOperation;

pub(super) fn operation(kind: PathOperation, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Path(kind), args),
    }
}

fn parameter(name: &str, ty: Type, default: Option<Expr>) -> Parameter {
    Parameter {
        name: name.to_owned(),
        ty,
        kind: ParameterKind::PositionalOrKeyword,
        default,
        sign_only: false,
    }
}

fn boolean(value: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(value),
    }
}

pub(super) fn file_open_mode(mode: &str) -> Option<(Type, Builtin)> {
    Some(match mode {
        "r" => (Type::TextIO, Builtin::TextOpen(0)),
        "w" => (Type::TextIO, Builtin::TextOpen(1)),
        "rb" => (Type::BinaryIO, Builtin::BinaryOpen(0)),
        "wb" => (Type::BinaryIO, Builtin::BinaryOpen(1)),
        "ab" => (Type::BinaryIO, Builtin::BinaryOpen(2)),
        "xb" => (Type::BinaryIO, Builtin::BinaryOpen(3)),
        _ => return None,
    })
}

impl Checker<'_> {
    /// Accept the original filesystem spelling and bind the directory descriptor by keyword.
    pub(super) fn os_unlink(&mut self, call: &ast::ExprCall, remove: bool) -> Result<Expr, String> {
        let fd = self.types.optional(Type::Int);
        let params = [
            parameter("path", Type::Str, None),
            Parameter {
                kind: ParameterKind::KeywordOnly,
                ..parameter(
                    "dir_fd",
                    fd,
                    Some(Expr {
                        ty: fd,
                        kind: ExprKind::Absent(Box::new(Expr {
                            ty: Type::None,
                            kind: ExprKind::None,
                        })),
                    }),
                )
            },
        ];
        let args = self.arguments_with(call, &params, None, |checker, index, expression| {
            if index == 0 {
                checker.fspath_argument(expression)
            } else {
                checker.expr_expected(expression, fd)
            }
        })?;
        Ok(self.ordered_builtin(
            Builtin::Process(ir::ProcessOperation::Unlink { remove }),
            Type::None,
            args,
        ))
    }

    /// Path-like values are exactly strings or native paths; no user protocol runs.
    pub(super) fn path_argument(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(expression)?;
        self.path_text(value, expression)
    }

    /// Preserve exact str/bytes values and unwrap the spelling of a native Path.
    pub(super) fn fspath_argument(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(expression)?;
        self.fspath_value(value, expression)
    }

    pub(super) fn fspath_value(&self, value: Expr, expression: &ast::Expr) -> Result<Expr, String> {
        match value.ty {
            Type::Str | Type::Bytes => Ok(value),
            Type::Path | Type::PurePath(_) => {
                Ok(operation(PathOperation::Text, Type::Str, vec![value]))
            }
            _ => Err(error(
                self.source,
                expression,
                "expected str, bytes or pathlib.Path",
            )),
        }
    }

    pub(super) fn path_text(&self, value: Expr, node: &impl Ranged) -> Result<Expr, String> {
        match value.ty {
            Type::Str => Ok(value),
            Type::JsonRoot | Type::JsonValue => self.coerce(value, Type::Str, node),
            ty if self
                .types
                .members(ty)
                .is_some_and(|members| members == [Type::Str, Type::Path]) =>
            {
                Ok(operation(PathOperation::UnionText, Type::Str, vec![value]))
            }
            Type::Path | Type::PurePath(_) => {
                Ok(operation(PathOperation::Text, Type::Str, vec![value]))
            }
            _ => Err(error(
                self.source,
                node,
                "a filesystem path requires str or pathlib.Path",
            )),
        }
    }

    pub(super) fn directory_manager(
        &mut self,
        intrinsic: Intrinsic,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let temporary = intrinsic == Intrinsic::TemporaryDirectory;
        let value = if temporary {
            self.temporary_directory(call)?
        } else {
            self.reject_builtin_keywords(call)?;
            match call.arguments.args.as_ref() {
                [] => Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                },
                [arg] => self.expr(arg)?,
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "nullcontext accepts one path or string value",
                    ));
                }
            }
        };
        let source = value.ty;
        let domain = self.types.union(&[Type::Str, Type::Path]).unwrap();
        let value = self.coerce(value, self.types.optional(domain), call)?;
        Ok(operation(
            PathOperation::ContextNew { temporary },
            self.types.directory_context(source),
            vec![value],
        ))
    }

    pub(super) fn path_constructor(
        &mut self,
        call: &ast::ExprCall,
        ty: Type,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        calls::reject_unpacking(call, self.source)?;
        let inputs = call
            .arguments
            .args
            .iter()
            .map(|arg| self.expr(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let checked = inputs
            .iter()
            .any(|value| self.types.payload(value.ty).is_some());
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let mut parts = Vec::new();
        for (value, syntax) in inputs.into_iter().zip(&call.arguments.args) {
            let mut part = if checked {
                // Method lookup and each argument expression run before the constructor
                // inspects any part. In particular a later argument may raise first.
                let slot = self.hidden(value.ty);
                body.push(Statement::Assign(slot, value));
                clear.push(slot);
                self.local(slot)
            } else {
                value
            };
            if let Some(payload) = self.types.payload(part.ty) {
                let missing = Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::IsNone, vec![part.clone()]),
                };
                let error = Statement::Raise(Some(Expr { ty: Type::Exception, kind: ExprKind::Exception {
                    kind: exception_kind("TypeError").unwrap(),
                    message: Some(Box::new(Expr { ty: Type::Str, kind: ExprKind::Str(
                        "argument should be a str or an os.PathLike object where __fspath__ returns a str, not 'NoneType'".into()) })),
                }}));
                part = Expr {
                    ty: payload,
                    kind: ExprKind::Scope {
                        body: vec![Statement::If(missing, vec![error], vec![])],
                        value: Box::new(Expr {
                            ty: payload,
                            kind: ExprKind::Unwrap(Box::new(part)),
                        }),
                        clear: vec![],
                    },
                };
            }
            parts.push((part, syntax));
        }
        let mut parts = parts.into_iter();
        let mut path = match ty {
            Type::Path => {
                let first = if let Some((part, syntax)) = parts.next() {
                    if matches!(part.ty, Type::PurePath(ir::PathFlavor::Windows)) {
                        operation(PathOperation::AsPosix, Type::Str, vec![part])
                    } else {
                        self.path_text(part, syntax)?
                    }
                } else {
                    unpacking::string("")
                };
                operation(PathOperation::New, ty, vec![first])
            }
            Type::PurePath(flavor) => operation(PathOperation::PureNew(flavor), ty, vec![]),
            _ => unreachable!(),
        };
        for (part, syntax) in parts {
            path = self.path_join(path, part, syntax)?;
        }
        if checked {
            Ok(Expr {
                ty,
                kind: ExprKind::Scope {
                    body,
                    clear,
                    value: Box::new(path),
                },
            })
        } else {
            Ok(path)
        }
    }

    pub(super) fn system_path_call(
        &mut self,
        intrinsic: Intrinsic,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        self.reject_builtin_keywords(call)?;
        let args = call.arguments.args.as_ref();
        match (intrinsic, args) {
            (Intrinsic::GetSysconfigPath, [ast::Expr::StringLiteral(name)])
                if name.value.to_str() == "scripts" =>
            {
                Ok(operation(PathOperation::Scripts, Type::Str, Vec::new()))
            }
            (Intrinsic::OsPathExists, [arg]) => Ok(operation(
                PathOperation::OsExists,
                Type::Bool,
                vec![self.path_argument(arg)?],
            )),
            _ => Err(error(
                self.source,
                call,
                "supported path calls are sysconfig.get_path('scripts') and os.path.exists(path)",
            )),
        }
    }

    /// Both operands are evaluated in source order, including reflected str / Path.
    pub(super) fn path_join(
        &self,
        left: Expr,
        right: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let left = if left.ty.is_path() {
            left
        } else {
            let first = self.path_text(left, node)?;
            if let Type::PurePath(flavor) = right.ty {
                operation(
                    PathOperation::Join,
                    right.ty,
                    vec![
                        operation(PathOperation::PureNew(flavor), right.ty, vec![]),
                        first,
                    ],
                )
            } else {
                operation(PathOperation::New, Type::Path, vec![first])
            }
        };
        let ty = left.ty;
        if right.ty.is_path() {
            return Ok(operation(PathOperation::JoinObject, ty, vec![left, right]));
        }
        Ok(operation(
            PathOperation::Join,
            ty,
            vec![left, self.path_text(right, node)?],
        ))
    }

    pub(super) fn path_attribute(
        &self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let (kind, ty) = match attribute.attr.as_str() {
            "name" => (PathOperation::Name, Type::Str),
            "stem" => (PathOperation::Stem, Type::Str),
            "suffix" => (PathOperation::Suffix, Type::Str),
            "parent" => (PathOperation::Parent, receiver.ty),
            "drive" => (PathOperation::Drive, Type::Str),
            "root" => (PathOperation::Root, Type::Str),
            "anchor" => (PathOperation::Anchor, Type::Str),
            _ => {
                return Err(error(
                    self.source,
                    attribute,
                    "unsupported native path attribute",
                ));
            }
        };
        Ok(operation(kind, ty, vec![receiver]))
    }

    pub(super) fn path_method(
        &mut self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let name = attribute.attr.as_str();
        if matches!(name, "match" | "full_match") {
            let full = name == "full_match";
            let optional_bool = self.types.optional(Type::Bool);
            let default = self.coerce(
                Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                },
                optional_bool,
                call,
            )?;
            let params = [
                parameter(
                    if full { "pattern" } else { "path_pattern" },
                    Type::Str,
                    None,
                ),
                Parameter {
                    kind: ParameterKind::KeywordOnly,
                    ..parameter("case_sensitive", optional_bool, Some(default))
                },
            ];
            let arguments =
                self.arguments_with(call, &params, Some(receiver), |checker, index, value| {
                    if index == 0 {
                        let pattern = checker.expr(value)?;
                        if pattern.ty.is_path() {
                            Ok(pattern)
                        } else {
                            checker.path_text(pattern, value)
                        }
                    } else {
                        checker.expr_expected(value, optional_bool)
                    }
                })?;
            return Ok(self.ordered_path_operation(
                PathOperation::Match { full },
                Type::Bool,
                arguments,
            ));
        }
        if name == "joinpath" {
            self.reject_builtin_keywords(call)?;
            calls::reject_unpacking(call, self.source)?;
            if call.arguments.args.is_empty() {
                // Joining an empty segment gives the result its own Path identity.
                return self.path_join(receiver, unpacking::string(""), call);
            }
            let mut path = receiver;
            for argument in &call.arguments.args {
                let child = self.expr(argument)?;
                path = self.path_join(path, child, argument)?;
            }
            return Ok(path);
        }
        if matches!(receiver.ty, Type::PurePath(_))
            && !matches!(name, "as_posix" | "is_absolute" | "with_name")
        {
            return Err(error(
                self.source,
                call,
                "unsupported native PurePath method",
            ));
        }
        if matches!(name, "read_text" | "write_text" | "write_bytes") {
            return self.path_io(receiver, name, call);
        }
        if name == "open" {
            return self.path_open(receiver, call);
        }
        if matches!(
            name,
            "mkdir" | "unlink" | "replace" | "resolve" | "with_name"
        ) {
            return self.path_call(receiver, name, call);
        }
        if matches!(name, "iterdir" | "glob") {
            return Err(error(
                self.source,
                call,
                "Path iterators require an immediate loop, comprehension, or iterable consumer",
            ));
        }
        let (kind, ty, arity) = match name {
            "absolute" => (PathOperation::Absolute, Type::Path, 0),
            "as_posix" => (PathOperation::AsPosix, Type::Str, 0),
            "exists" => (PathOperation::Exists, Type::Bool, 0),
            "is_file" => (PathOperation::IsFile, Type::Bool, 0),
            "is_dir" => (PathOperation::IsDir, Type::Bool, 0),
            "is_absolute" => (PathOperation::IsAbsolute, Type::Bool, 0),
            "read_bytes" => (PathOperation::ReadBytes, Type::Bytes, 0),
            "rmdir" => (PathOperation::Rmdir, Type::None, 0),
            _ => return Err(error(self.source, call, "unsupported native Path method")),
        };
        if call.arguments.args.len() != arity {
            return Err(error(
                self.source,
                call,
                &format!("Path.{name} requires {arity} positional arguments"),
            ));
        }
        self.reject_builtin_keywords(call)?;
        Ok(operation(kind, ty, vec![receiver]))
    }

    /// Keep all arguments alive in source order. Path.write_text and write_bytes
    /// validate their data before opening the file, even when other options are invalid.
    fn path_io(
        &mut self,
        receiver: Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        if name == "read_text"
            && call.arguments.args.is_empty()
            && call.arguments.keywords.is_empty()
        {
            return Ok(operation(
                PathOperation::ReadText,
                Type::Str,
                vec![receiver],
            ));
        }
        let write = name != "read_text";
        let binary = name == "write_bytes";
        let expected = if binary { Type::Bytes } else { Type::Str };
        let mut params = Vec::new();
        if write {
            params.push(parameter("data", expected, None));
        }
        if !binary {
            let optional = self.types.optional(Type::Str);
            let absent = Expr {
                ty: optional,
                kind: ExprKind::Absent(Box::new(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                })),
            };
            for key in ["encoding", "errors", "newline"] {
                params.push(parameter(key, optional, Some(absent.clone())));
            }
        }
        let mut arguments =
            self.arguments_with(call, &params, Some(receiver), |checker, index, value| {
                if write && index == 0 {
                    let data = checker.expr(value)?;
                    let ty = data.ty;
                    if matches!(ty, Type::StrLiteral(_)) {
                        return checker.coerce(data, Type::Str, value);
                    }
                    if matches!(ty, Type::Optional(_) | Type::Union(_) | Type::None) {
                        let union = checker.types.union(&[Type::Str, Type::Bytes]).unwrap();
                        let payload = checker.types.payload(ty).unwrap_or(ty);
                        if !matches!(
                            payload,
                            Type::None | Type::Str | Type::Bytes | Type::StrLiteral(_)
                        ) && payload != union
                        {
                            return Err(error(
                                checker.source,
                                value,
                                "narrow the Path write data to a concrete type",
                            ));
                        }
                        return checker.coerce(data, checker.types.optional(union), value);
                    }
                    Ok(data)
                } else {
                    checker.expr_expected(value, params[index].ty)
                }
            })?;
        let data = write.then(|| arguments.values[arguments.order[1]].ty);
        let kind = if data.is_some_and(|ty| self.types.payload(ty).is_some()) {
            PathOperation::WriteUnion { binary }
        } else if let Some(actual) = data.filter(|ty| *ty != expected) {
            let name = match actual {
                Type::List(_) | Type::ListInt => "list".to_owned(),
                Type::Tuple(_) => "tuple".to_owned(),
                Type::Dict(_) => "dict".to_owned(),
                Type::Set(_) => "set".to_owned(),
                Type::Class(id) => self.classes.declarations.definitions[id].name.clone(),
                Type::StrEnum(_) => {
                    return Err(error(
                        self.source,
                        call,
                        "use str(member) or member.value for Path text I/O",
                    ));
                }
                _ => self.type_name(actual),
            };
            let message = if binary {
                format!("memoryview: a bytes-like object is required, not '{name}'")
            } else {
                format!("data must be str, not {name}")
            };
            arguments.order.push(arguments.values.len());
            arguments.values.push(unpacking::string(&message));
            PathOperation::WriteTypeError
        } else if binary {
            PathOperation::WriteBytes
        } else if write {
            PathOperation::WriteText
        } else {
            PathOperation::ReadText
        };
        Ok(self.ordered_path_operation(kind, if write { Type::Int } else { Type::Str }, arguments))
    }

    /// Method arguments retain source evaluation order even when keywords bind
    /// a different ABI position. Every operand stays rooted through the syscall.
    fn path_call(
        &mut self,
        receiver: Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let (kind, returns, params) = match name {
            "mkdir" => (
                PathOperation::Mkdir,
                Type::None,
                vec![
                    parameter(
                        "mode",
                        Type::Int,
                        Some(Expr {
                            ty: Type::Int,
                            kind: ExprKind::Int("511".to_owned()),
                        }),
                    ),
                    parameter("parents", Type::Bool, Some(boolean(false))),
                    parameter("exist_ok", Type::Bool, Some(boolean(false))),
                ],
            ),
            "unlink" => (
                PathOperation::Unlink,
                Type::None,
                vec![parameter("missing_ok", Type::Bool, Some(boolean(false)))],
            ),
            "replace" => (
                PathOperation::Replace,
                Type::Path,
                vec![parameter("target", Type::Str, None)],
            ),
            "resolve" => (
                PathOperation::Resolve,
                Type::Path,
                vec![parameter("strict", Type::Bool, Some(boolean(false)))],
            ),
            "with_name" => (
                PathOperation::WithName,
                receiver.ty,
                vec![parameter("name", Type::Str, None)],
            ),
            _ => unreachable!("only path methods with parameters have this signature"),
        };
        let arguments =
            self.arguments_with(call, &params, Some(receiver), |checker, index, value| {
                if name == "replace" {
                    checker.path_argument(value)
                } else {
                    checker.expr_expected(value, params[index].ty)
                }
            })?;
        Ok(self.ordered_path_operation(kind, returns, arguments))
    }

    /// Select the existing stream ABI statically so file operations and context
    /// manager cleanup use exactly the same typed handle as builtin `open`.
    fn path_open(&mut self, receiver: Expr, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        if call.arguments.args.len() > 1 {
            return Err(error(
                self.source,
                call,
                "Path.open accepts one positional mode",
            ));
        }
        let mut mode = call.arguments.args.first();
        let mut encoding = None;
        for keyword in &call.arguments.keywords {
            match keyword.arg.as_ref().map(|arg| arg.as_str()) {
                Some("mode") if mode.is_none() => mode = Some(&keyword.value),
                Some("encoding") if encoding.is_none() => encoding = Some(keyword),
                _ => {
                    return Err(error(
                        self.source,
                        keyword,
                        "unexpected or repeated Path.open argument",
                    ));
                }
            }
        }
        let mode = match mode {
            None => "r",
            Some(ast::Expr::StringLiteral(mode)) => {
                check_string(mode, self.source)?;
                mode.value.to_str()
            }
            Some(mode) => {
                return Err(error(
                    self.source,
                    mode,
                    "Path.open requires a literal mode",
                ));
            }
        };
        let (ty, builtin) = match mode {
            "rt" | "tr" => file_open_mode("r"),
            "wt" | "tw" => file_open_mode("w"),
            "br" => file_open_mode("rb"),
            "bw" => file_open_mode("wb"),
            "ba" => file_open_mode("ab"),
            "bx" => file_open_mode("xb"),
            "a" | "at" | "ta" => Some((Type::TextIO, Builtin::TextOpen(2))),
            "x" | "xt" | "tx" => Some((Type::TextIO, Builtin::TextOpen(3))),
            _ => file_open_mode(mode),
        }
        .ok_or_else(|| error(self.source, call, "unsupported native Path.open mode"))?;
        if let Some(encoding) = encoding {
            if ty == Type::BinaryIO {
                return Err(error(
                    self.source,
                    encoding,
                    "binary Path.open does not accept an encoding",
                ));
            }
            self.utf8_encoding_keyword(encoding)?;
        }
        let path = operation(PathOperation::Text, Type::Str, vec![receiver]);
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(builtin, vec![path]),
        })
    }

    /// A temporary-directory manager exists only at this immediate `with`
    /// boundary. Its owned name is retained privately for finally cleanup.
    pub(super) fn temporary_directory(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let directory = self.types.optional(Type::Str);
        let mut params = vec![
            parameter(
                "dir",
                directory,
                Some(Expr {
                    ty: directory,
                    kind: ExprKind::Absent(Box::new(Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    })),
                }),
            ),
            parameter(
                "prefix",
                Type::Str,
                Some(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str("tmp".to_owned()),
                }),
            ),
            parameter(
                "suffix",
                Type::Str,
                Some(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str(String::new()),
                }),
            ),
        ];
        for param in &mut params {
            param.kind = ParameterKind::KeywordOnly;
        }
        let arguments = self.arguments_with(call, &params, None, |checker, index, value| {
            if index == 0 {
                checker.optional_path_argument(value)
            } else {
                checker.expr_expected(value, params[index].ty)
            }
        })?;
        Ok(self.ordered_path_operation(PathOperation::TemporaryDirectory, Type::Str, arguments))
    }

    fn ordered_path_operation(
        &mut self,
        kind: PathOperation,
        returns: Type,
        arguments: ir::CallArgs,
    ) -> Expr {
        let mut body = Vec::new();
        let mut clear = Vec::new();
        for value in arguments.values {
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
        }
        let args = arguments
            .order
            .iter()
            .map(|&index| self.local(clear[index]))
            .collect();
        Expr {
            ty: returns,
            kind: ExprKind::Scope {
                body,
                value: Box::new(operation(kind, returns, args)),
                clear,
            },
        }
    }

    /// Bounded glob syntax has no recursive traversal or dynamic matching protocol.
    /// Matching itself stays native and preserves pathlib's treatment of dotfiles.
    pub(super) fn path_iterator(
        &self,
        receiver: Expr,
        call: &ast::ExprCall,
        glob: bool,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        self.reject_builtin_keywords(call)?;
        let mut args = vec![receiver];
        if glob {
            let [ast::Expr::StringLiteral(pattern)] = call.arguments.args.as_ref() else {
                return Err(error(
                    self.source,
                    call,
                    "Path.glob requires one literal single-component pattern",
                ));
            };
            check_string(pattern, self.source)?;
            let value = pattern.value.to_str();
            if value.is_empty() || value == "." || value.contains('/') || value.contains("**") {
                return Err(error(
                    self.source,
                    pattern,
                    "Path.glob supports nonempty single-component patterns without recursive '**'",
                ));
            }
            args.push(Expr {
                ty: Type::Str,
                kind: ExprKind::Str(value.into()),
            });
        } else if !call.arguments.args.is_empty() {
            return Err(error(self.source, call, "Path.iterdir takes no arguments"));
        }
        Ok(operation(
            if glob {
                PathOperation::Glob
            } else {
                PathOperation::Iterdir
            },
            Type::Iterator,
            args,
        ))
    }
}
