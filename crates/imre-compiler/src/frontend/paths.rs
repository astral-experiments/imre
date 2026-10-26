//! Sealed pathlib operations retain concrete path storage and explicit UTF-8 I/O.

use super::*;
use crate::ir::PathOperation;

mod samefile;
mod segments;
mod stat;

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
        self.path_segments(call, ty, None)
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
            "info" if receiver.ty == Type::Path => (PathOperation::Info, Type::PathInfo),
            "name" => (PathOperation::Name, Type::Str),
            "stem" => (PathOperation::Stem, Type::Str),
            "suffix" => (PathOperation::Suffix, Type::Str),
            "suffixes" => (PathOperation::Suffixes, self.types.list(Type::Str)),
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

    /// CPython's tuple contains integral seconds, while its time attributes use the subsecond stat data.
    pub(super) fn stat_result_attribute(
        &self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
        declaration: &crate::types::NamedTuple,
    ) -> Result<Expr, String> {
        let name = attribute.attr.as_str();
        if name == "__match_args__" {
            // The three indexed timestamps are unnamed in CPython's struct sequence.
            let positional = crate::types::NamedTuple {
                name: declaration.name.clone(),
                fields: declaration.fields[..7].to_vec(),
            };
            return self.namedtuple_instance_attribute(attribute, receiver, &positional);
        }
        let times = ["st_atime", "st_mtime", "st_ctime", "st_birthtime"];
        if let Some(index) = times.iter().position(|field| *field == name) {
            return Ok(operation(
                PathOperation::StatTime(index as u32),
                Type::Float,
                vec![receiver],
            ));
        }
        let fields = [
            "st_atime_ns",
            "st_mtime_ns",
            "st_ctime_ns",
            "st_blksize",
            "st_blocks",
            "st_rdev",
            "st_flags",
            "st_gen",
            "st_birthtime_ns",
            "n_fields",
            "n_sequence_fields",
            "n_unnamed_fields",
        ];
        if let Some(index) = fields.iter().position(|field| *field == name) {
            return Ok(operation(
                PathOperation::StatExtraInt(index as u32),
                Type::Int,
                vec![receiver],
            ));
        }
        self.namedtuple_instance_attribute(attribute, receiver, declaration)
    }

    pub(super) fn path_method(
        &mut self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let name = attribute.attr.as_str();
        if receiver.ty == Type::Path && name == "samefile" {
            return self.path_samefile(receiver, call);
        }
        if receiver.ty == Type::Path && matches!(name, "stat" | "lstat") {
            return self.path_stat(receiver, name, call);
        }
        if matches!(receiver.ty, Type::Path | Type::PathInfo) {
            let info = receiver.ty == Type::PathInfo;
            use ir::PathQuery;
            let query = match name {
                "exists" => Some(PathQuery::Exists),
                "is_dir" => Some(PathQuery::Directory),
                "is_file" => Some(PathQuery::File),
                "is_symlink" => Some(PathQuery::Symlink),
                "is_fifo" if !info => Some(PathQuery::Fifo),
                "is_socket" if !info => Some(PathQuery::Socket),
                "is_block_device" if !info => Some(PathQuery::BlockDevice),
                "is_char_device" if !info => Some(PathQuery::CharDevice),
                _ => None,
            };
            if let Some(query) = query {
                let kind = PathOperation::Query(query);
                if matches!(
                    query,
                    PathQuery::Exists | PathQuery::Directory | PathQuery::File
                ) {
                    let param = Parameter {
                        kind: ParameterKind::KeywordOnly,
                        ..parameter("follow_symlinks", Type::Bool, Some(boolean(true)))
                    };
                    let args = self.arguments_with(
                        call,
                        &[param],
                        Some(receiver),
                        |checker, _, value| {
                            let value = checker.expr(value)?;
                            Ok(checker.truth_value(value))
                        },
                    )?;
                    return Ok(self.ordered_path_operation(kind, Type::Bool, args));
                }
                if !call.arguments.args.is_empty() {
                    return Err(error(
                        self.source,
                        call,
                        &format!("{name} requires no positional arguments"),
                    ));
                }
                self.reject_builtin_keywords(call)?;
                let follow = !matches!(query, PathQuery::Symlink);
                return Ok(operation(kind, Type::Bool, vec![receiver, boolean(follow)]));
            }
            if info {
                return Err(error(self.source, call, "unsupported Path.info method"));
            }
        }
        if matches!(name, "relative_to" | "is_relative_to") {
            let ty = receiver.ty;
            let (op, result) = if name == "relative_to" {
                (PathOperation::RelativeTo, ty)
            } else {
                (PathOperation::IsRelativeTo, Type::Bool)
            };
            if call.arguments.args.is_empty()
                && call.arguments.keywords.iter().all(|kw| {
                    name == "relative_to"
                        && kw.arg.as_ref().is_some_and(|a| a.as_str() == "walk_up")
                })
            {
                let mut values = vec![receiver];
                for kw in &call.arguments.keywords {
                    values.push(self.expr(&kw.value)?);
                }
                self.graph
                    .checked_missing_argument(self.module, call.range());
                return Ok(self.bad_relative_argument(
                    values,
                    result,
                    format!("PurePath.{name}() missing 1 required positional argument: 'other'"),
                ));
            }
            let mut params = vec![parameter("other", Type::Str, None)];
            if name == "relative_to" {
                params.push(Parameter {
                    kind: ParameterKind::KeywordOnly,
                    ..parameter("walk_up", Type::Bool, Some(boolean(false)))
                });
            }
            let arguments =
                self.arguments_with(call, &params, Some(receiver), |checker, i, arg| {
                    if i != 0 {
                        return checker.expr_expected(arg, Type::Bool);
                    }
                    let value = checker.expr(arg)?;
                    if value.ty.is_path()
                        || matches!(
                            value.ty,
                            Type::Bytes | Type::None | Type::Int | Type::Float | Type::Bool
                        )
                    {
                        return Ok(value);
                    }
                    let text = checker.path_text(value, arg)?;
                    Ok(if let Type::PurePath(flavor) = ty {
                        let base = operation(PathOperation::PureNew(flavor), ty, vec![]);
                        operation(PathOperation::Join, ty, vec![base, text])
                    } else {
                        operation(PathOperation::New, Type::Path, vec![text])
                    })
                })?;
            let other = arguments.values[arguments.order[1]].ty;
            if !other.is_path() {
                let kind = if other == Type::None {
                    "NoneType".to_owned()
                } else {
                    self.type_name(other)
                };
                return Ok(self.bad_relative_argument(arguments.values, result, format!(
                    "argument should be a str or an os.PathLike object where __fspath__ returns a str, not '{kind}'")));
            }
            return Ok(self.ordered_path_operation(op, result, arguments));
        }
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
        if matches!(name, "joinpath" | "with_segments") {
            return self.path_segments(call, receiver.ty, Some((receiver, name == "joinpath")));
        }
        if matches!(receiver.ty, Type::PurePath(_))
            && !matches!(
                name,
                "as_posix" | "is_absolute" | "with_name" | "with_stem" | "with_suffix"
            )
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
            "mkdir"
                | "unlink"
                | "replace"
                | "resolve"
                | "with_name"
                | "with_stem"
                | "with_suffix"
                | "symlink_to"
                | "hardlink_to"
        ) {
            return self.path_call(receiver, name, call);
        }
        if matches!(name, "iterdir" | "glob" | "rglob" | "walk") {
            return self.path_iterator(receiver, call, name);
        }
        let (kind, ty, arity) = match name {
            "absolute" => (PathOperation::Absolute, Type::Path, 0),
            "as_posix" => (PathOperation::AsPosix, Type::Str, 0),
            "is_absolute" => (PathOperation::IsAbsolute, Type::Bool, 0),
            "read_bytes" => (PathOperation::ReadBytes, Type::Bytes, 0),
            "readlink" => (PathOperation::Readlink, Type::Path, 0),
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
            "symlink_to" => (
                PathOperation::Link(ir::PathLink::Symlink),
                Type::None,
                vec![
                    parameter("target", Type::Str, None),
                    parameter("target_is_directory", Type::Bool, Some(boolean(false))),
                ],
            ),
            "hardlink_to" => (
                PathOperation::Link(ir::PathLink::Hardlink),
                Type::None,
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
                vec![parameter("name", self.types.optional(Type::Str), None)],
            ),
            "with_stem" => (
                PathOperation::WithStem,
                receiver.ty,
                vec![parameter("stem", self.types.optional(Type::Str), None)],
            ),
            "with_suffix" => (
                PathOperation::WithSuffix,
                receiver.ty,
                vec![parameter("suffix", self.types.optional(Type::Str), None)],
            ),
            _ => unreachable!("only path methods with parameters have this signature"),
        };
        let arguments =
            self.arguments_with(call, &params, Some(receiver), |checker, index, value| {
                if matches!(name, "symlink_to" | "hardlink_to") {
                    checker.expr(value)
                } else if name == "replace" {
                    checker.path_argument(value)
                } else {
                    checker.expr_expected(value, params[index].ty)
                }
            })?;
        if let PathOperation::Link(link) = kind {
            let mut scope = calls::ArgumentScope::default();
            // The Python method receives every expression before os.link/os.symlink
            // asks for a filename or the truth of target_is_directory.
            let values: Vec<_> = arguments
                .values
                .into_iter()
                .map(|v| scope.hold(self, v))
                .collect();
            let mut args: Vec<_> = arguments.order.iter().map(|&i| values[i].clone()).collect();
            let symlink = matches!(link, ir::PathLink::Symlink);
            let operation_name = if symlink { "symlink" } else { "link" };
            let source = self.syscall_fspath(args[1].clone(), &call.func, operation_name, "src")?;
            args[1] = scope.hold(self, source);
            if symlink {
                let flag = args.pop().unwrap();
                if flag.ty != Type::Bool {
                    scope.hold(
                        self,
                        operation(
                            PathOperation::Link(ir::PathLink::CheckSymlink),
                            Type::None,
                            args.clone(),
                        ),
                    );
                    let flag = self.truth_value(flag);
                    scope.hold(self, flag);
                }
            }
            return Ok(scope.finish(operation(kind, returns, args)));
        }
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
        let mut newline = None;
        for keyword in &call.arguments.keywords {
            match keyword.arg.as_ref().map(|arg| arg.as_str()) {
                Some("mode") if mode.is_none() => mode = Some(&keyword.value),
                Some("encoding") if encoding.is_none() => encoding = Some(keyword),
                Some("newline") if newline.is_none() => newline = Some(&keyword.value),
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
        let (ty, mut builtin) = match mode {
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
        let mut args = vec![path];
        if let Some(encoding) = encoding {
            args.push(self.expr(&encoding.value)?);
        }
        if let Some(newline) = newline {
            builtin = self.text_newline(newline, builtin)?;
        }
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(builtin, args),
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

    fn bad_relative_argument(&mut self, values: Vec<Expr>, ty: Type, message: String) -> Expr {
        // Python evaluates the receiver and all arguments before it checks the
        // signature or attempts to construct a path from the 'other' value.
        let mut body = Vec::new();
        let mut clear = Vec::new();
        for value in values {
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
        }
        body.push(Statement::Raise(Some(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(&message))),
            },
        })));
        let unreachable = if ty == Type::Bool {
            boolean(false)
        } else {
            self.local(clear[0])
        };
        Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(unreachable),
                clear,
            },
        }
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
        let args: Vec<_> = arguments
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

    /// Construct the iterator at the call site. Globs bind and validate their pattern
    /// now, but defer every directory scan until the consumer first requests a path.
    pub(super) fn path_iterator(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
        name: &str,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        if name == "iterdir" {
            self.reject_builtin_keywords(call)?;
            if !call.arguments.args.is_empty() {
                return Err(error(self.source, call, "Path.iterdir takes no arguments"));
            }
            return Ok(operation(
                PathOperation::Iterdir,
                self.types.cursor(Type::Path),
                vec![receiver],
            ));
        }
        if name == "walk" {
            let none = Expr {
                ty: Type::None,
                kind: ExprKind::None,
            };
            let params = [
                parameter("top_down", Type::Bool, Some(boolean(true))),
                parameter("on_error", Type::None, Some(none)),
                parameter("follow_symlinks", Type::Bool, Some(boolean(false))),
            ];
            let args = self.arguments(call, &params, Some(receiver))?;
            let names = self.types.list(Type::Str);
            let row = self.types.tuple(vec![Type::Path, names, names]);
            return Ok(self.ordered_path_operation(
                PathOperation::Walk,
                self.types.cursor(row),
                args,
            ));
        }
        let optional = self.types.optional(Type::Bool);
        let default = self.coerce(
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            },
            optional,
            call,
        )?;
        let params = [
            parameter("pattern", Type::Str, None),
            Parameter {
                kind: ParameterKind::KeywordOnly,
                ..parameter("case_sensitive", optional, Some(default))
            },
            Parameter {
                kind: ParameterKind::KeywordOnly,
                ..parameter("recurse_symlinks", Type::Bool, Some(boolean(false)))
            },
        ];
        let args = self.arguments_with(call, &params, Some(receiver), |checker, i, value| {
            if i == 0 {
                checker.path_argument(value)
            } else {
                checker.expr_expected(value, params[i].ty)
            }
        })?;
        Ok(self.ordered_path_operation(
            PathOperation::Glob {
                recursive: name == "rglob",
            },
            self.types.cursor(Type::Path),
            args,
        ))
    }
}
