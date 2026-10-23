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

impl Checker<'_> {
    /// Path-like values are exactly strings or native paths; no user protocol runs.
    pub(super) fn path_argument(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(expression)?;
        self.path_text(value, expression)
    }

    fn path_text(&self, value: Expr, node: &impl Ranged) -> Result<Expr, String> {
        match value.ty {
            Type::Str => Ok(value),
            Type::Path => Ok(operation(PathOperation::Text, Type::Str, vec![value])),
            _ => Err(error(
                self.source,
                node,
                "a filesystem path requires str or pathlib.Path",
            )),
        }
    }

    pub(super) fn path_constructor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let mut args = call.arguments.args.iter();
        let first = if let Some(argument) = args.next() {
            self.path_argument(argument)?
        } else {
            Expr {
                ty: Type::Str,
                kind: ExprKind::Str(String::new()),
            }
        };
        let mut result = operation(PathOperation::New, Type::Path, vec![first]);
        for argument in args {
            let next = self.path_argument(argument)?;
            result = operation(PathOperation::Join, Type::Path, vec![result, next]);
        }
        Ok(result)
    }

    /// Both operands are evaluated in source order, including reflected str / Path.
    pub(super) fn path_join(
        &self,
        left: Expr,
        right: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let left = if left.ty == Type::Path {
            left
        } else {
            operation(
                PathOperation::New,
                Type::Path,
                vec![self.path_text(left, node)?],
            )
        };
        let right = self.path_text(right, node)?;
        Ok(operation(
            PathOperation::Join,
            Type::Path,
            vec![left, right],
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
            "parent" => (PathOperation::Parent, Type::Path),
            _ => {
                return Err(error(
                    self.source,
                    attribute,
                    "supported Path attributes are name, stem, suffix, and parent",
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
        if matches!(name, "mkdir" | "unlink" | "replace" | "resolve") {
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
            "as_posix" => (PathOperation::Text, Type::Str, 0),
            "exists" => (PathOperation::Exists, Type::Bool, 0),
            "is_file" => (PathOperation::IsFile, Type::Bool, 0),
            "is_dir" => (PathOperation::IsDir, Type::Bool, 0),
            "is_absolute" => (PathOperation::IsAbsolute, Type::Bool, 0),
            "read_text" => (PathOperation::ReadText, Type::Str, 0),
            "write_text" => (PathOperation::WriteText, Type::Int, 1),
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
        match kind {
            PathOperation::ReadText if call.arguments.keywords.is_empty() => {}
            PathOperation::ReadText | PathOperation::WriteText => self.utf8_encoding(call)?,
            _ => self.reject_builtin_keywords(call)?,
        }
        let mut args = vec![receiver];
        if let Some(value) = call.arguments.args.first() {
            args.push(self.expr_expected(value, Type::Str)?);
        }
        Ok(operation(kind, ty, args))
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
