//! Bind text stream operations without losing the cursor when a StringIO is used as TextIO.

use super::*;
use ir::StringIoOperation as Op;

/// The C constructor accepts keywords; its cursor and read methods take positional arguments only.
fn parameter(name: &str, ty: Type, kind: ParameterKind, default: Option<Expr>) -> Parameter {
    Parameter {
        name: name.into(),
        ty,
        kind,
        default,
        sign_only: false,
    }
}

/// Stream capabilities raise on a closed StringIO, including those that are false.
fn string_io_flag(receiver: &Expr, enabled: bool) -> Expr {
    let open = Expr {
        ty: Type::Bool,
        kind: ExprKind::Builtin(Builtin::StringIo(Op::Capability), vec![receiver.clone()]),
    };
    if enabled {
        open
    } else {
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Not(Box::new(open)),
        }
    }
}

impl Checker<'_> {
    pub(super) fn string_io_attribute(&self, receiver: &Expr, name: &str) -> Option<Expr> {
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let (check, value) = match name {
            "newlines" => {
                return Some(Expr {
                    ty: self.types.text_newlines_result(),
                    kind: ExprKind::Builtin(
                        Builtin::StringIo(Op::Newlines),
                        vec![receiver.clone()],
                    ),
                });
            }
            "encoding" | "errors" => (receiver.clone(), none),
            "line_buffering" => return Some(string_io_flag(receiver, false)),
            _ => return None,
        };
        Some(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Expr(check)],
                value: Box::new(value),
                clear: Vec::new(),
            },
        })
    }

    fn string_io_write(
        &self,
        receiver: Expr,
        text: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if matches!(text.ty, Type::Str | Type::StrLiteral(_)) {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(
                    Builtin::TextWrite,
                    vec![receiver, self.coerce(text, Type::Str, node)?],
                ),
            });
        }
        let name = match text.ty {
            Type::None => "NoneType",
            Type::Int => "int",
            Type::Bool => "bool",
            Type::Float => "float",
            Type::Bytes => "bytes",
            Type::List(_) | Type::ListInt => "list",
            Type::Tuple(_) => "tuple",
            Type::Dict(_) => "dict",
            Type::Set(_) => "set",
            Type::Class(id) => &self.classes.declarations.definitions[id].name,
            _ => return Err(error(self.source, node, "unsupported StringIO write value")),
        };
        let exception = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(format!(
                    "string argument expected, got '{name}'"
                )))),
            },
        };
        Ok(Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::RaiseException, vec![receiver, text, exception]),
        })
    }

    /// Use the ordinary iteration lowering. A type error in a later line must
    /// leave earlier writes visible and the input at that failing item.
    fn string_io_writelines(&mut self, file: Expr, lines: &ast::Expr) -> Result<Expr, String> {
        let slot = self.hidden(Type::StringIO);
        let mut body = vec![Statement::Assign(slot, file)];
        let mut clear = vec![slot];
        let open = Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(Builtin::TextEnter, vec![self.local(slot)]),
        });
        // A fresh empty list needs a layout. Otherwise infer the iterable's
        // actual element type: writelines checks each item only after pulling.
        let hint =
            matches!(lines, ast::Expr::List(list) if list.elts.is_empty()).then_some(Type::Str);
        let input = self.expansion_input(lines, hint)?;
        let input = match input {
            Iteration::Container {
                iterable,
                items: false,
                start: None,
            } if self.types.fields(iterable.ty).is_some() => {
                let captured = self.hidden(iterable.ty);
                body.push(Statement::Assign(captured, iterable));
                clear.push(captured);
                body.push(open);
                Iteration::Container {
                    iterable: self.local(captured),
                    items: false,
                    start: None,
                }
            }
            input => {
                let mut input = self.prepare_iteration(input, lines, false)?;
                input.evaluate.push(open);
                Iteration::Prepared(input)
            }
        };
        body.extend(self.expand_items(input, lines, |checker, item| {
            Ok(vec![Statement::Expr(checker.string_io_write(
                checker.local(slot),
                item,
                lines,
            )?)])
        })?);
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body,
                clear,
                value: Box::new(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }),
            },
        })
    }
    /// Like the file iteration path, iter checks closed state immediately, and
    /// returns the original stream with its shared character position.
    pub(super) fn open_string_io(&mut self, stream: Expr) -> Expr {
        let slot = self.hidden(Type::StringIO);
        Expr {
            ty: Type::StringIO,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(slot, stream),
                    Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::TextEnter, vec![self.local(slot)]),
                    }),
                ],
                value: Box::new(self.local(slot)),
                clear: vec![slot],
            },
        }
    }

    /// Distinguish the omitted newline (LF) from a supplied None (universal newlines).
    pub(super) fn string_io(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let string = self.types.optional(Type::Str);
        let none = self.coerce(
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            },
            string,
            call,
        )?;
        let lf = self.coerce(
            Expr {
                ty: Type::Str,
                kind: ExprKind::Str("\n".into()),
            },
            string,
            call,
        )?;
        let params = [
            parameter(
                "initial_value",
                string,
                ParameterKind::PositionalOrKeyword,
                Some(none),
            ),
            parameter(
                "newline",
                string,
                ParameterKind::PositionalOrKeyword,
                Some(lf),
            ),
        ];
        let args = self.arguments(call, &params, None)?;
        Ok(self.ordered_builtin(Builtin::StringIo(Op::New), Type::StringIO, args))
    }

    /// Keep the stream rooted across user iteration and check each item at write time.
    pub(super) fn string_io_protocol(
        &mut self,
        receiver: &Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Option<Expr>, String> {
        let value = match (name, call.arguments.args.as_ref()) {
            ("write", [value]) => {
                let text = self.expr(value)?;
                self.string_io_write(receiver.clone(), text, value)?
            }
            ("writelines", [lines]) => self.string_io_writelines(receiver.clone(), lines)?,
            ("__iter__", []) => self.open_string_io(receiver.clone()),
            ("__next__", []) => self.next_value(receiver.clone(), &[], call)?,
            ("writable" | "readable" | "seekable", []) => string_io_flag(receiver, true),
            ("isatty", []) => Expr {
                ty: Type::Bool,
                // The inherited _IOBase method uses its own closed-file error.
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::TextEnter, vec![receiver.clone()]),
                    })],
                    value: Box::new(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Bool(false),
                    }),
                    clear: Vec::new(),
                },
            },
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    /// Keep the receiver rooted while evaluating explicit arguments in source order.
    pub(super) fn string_io_method(
        &mut self,
        file: Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let integer = self.types.optional(Type::Int);
        let none = self.coerce(
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            },
            integer,
            call,
        )?;
        let pos = ParameterKind::PositionalOnly;
        let size = || parameter("size", integer, pos, Some(none.clone()));
        let (op, returns, params) = match name {
            "getvalue" => (Op::Value, Type::Str, vec![]),
            "readlines" => (Op::Readlines, self.types.list(Type::Str), vec![size()]),
            "read" | "readline" => (Op::Read, Type::Str, vec![size()]),
            "seek" => (
                Op::Seek,
                Type::Int,
                vec![
                    parameter("pos", Type::Int, pos, None),
                    parameter("whence", Type::Int, pos, Some(logging::number(0))),
                ],
            ),
            "tell" => (Op::Position, Type::Int, vec![]),
            "truncate" => (Op::Position, Type::Int, vec![size()]),
            _ => unreachable!("only StringIO-specific method names reach this binder"),
        };
        let mut args = self.arguments(call, &params, Some(file))?;
        let mut append = |value| {
            args.order.push(args.values.len());
            args.values.push(value);
        };
        if name == "tell" {
            append(none);
        }
        if matches!(op, Op::Read | Op::Position) {
            append(Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(matches!(name, "readline" | "truncate")),
            });
        }
        Ok(self.ordered_builtin(Builtin::StringIo(op), returns, args))
    }
}
