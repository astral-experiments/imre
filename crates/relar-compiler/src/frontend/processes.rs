//! Closed process signatures preserve evaluation order and exact owned results.

use super::*;
use crate::ir::{PathOperation, ProcessOperation};

fn operation(operation: ProcessOperation, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Process(operation), args),
    }
}

fn boolean(value: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(value),
    }
}

fn absent(ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Absent(Box::new(Expr {
            ty: Type::None,
            kind: ExprKind::None,
        })),
    }
}

impl Checker<'_> {
    /// Literal mode arguments establish a text-only result. Dynamic values are
    /// rooted in source order before their permutation into the runtime ABI.
    pub(super) fn process_run(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let strings = self.types.list(Type::Str);
        let directory = self.types.optional(Type::Str);
        let environment = self.types.optional(self.types.dict(Type::Str, Type::Str));
        let params = [
            ("args", strings, None),
            ("cwd", directory, Some(absent(directory))),
            ("env", environment, Some(absent(environment))),
            ("check", Type::Bool, Some(boolean(false))),
            ("capture_output", Type::Bool, None),
            ("encoding", Type::Str, None),
            ("text", Type::Bool, Some(boolean(true))),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (name, ty, default))| Parameter {
            name: name.to_owned(),
            ty,
            kind: if index == 0 {
                ParameterKind::PositionalOrKeyword
            } else {
                ParameterKind::KeywordOnly
            },
            default,
        })
        .collect::<Vec<_>>();
        let supplied = self.arguments_with(call, &params, None, |checker, index, argument| {
            match index {
                1 => checker.process_directory(argument),
                4 | 6 => {
                    if !matches!(argument, ast::Expr::BooleanLiteral(value) if value.value) {
                        return Err(error(checker.source, argument, "process capture_output and text require literal True"));
                    }
                    checker.expr(argument)
                }
                5 => {
                    if !matches!(argument, ast::Expr::StringLiteral(value) if value.value.to_string() == "utf-8") {
                        return Err(error(checker.source, argument, "process encoding requires literal 'utf-8'"));
                    }
                    checker.expr(argument)
                }
                _ => checker.expr_expected(argument, params[index].ty),
            }
        })?;
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let mut values = vec![None; supplied.values.len()];
        for (index, value) in supplied.values.into_iter().enumerate() {
            // The validated mode literals have no effects and need no runtime storage.
            if supplied.order[..4].contains(&index) {
                let slot = self.hidden(value.ty);
                values[index] = Some(Expr {
                    ty: value.ty,
                    kind: ExprKind::Local(slot),
                });
                body.push(Statement::Assign(slot, value));
                clear.push(slot);
            }
        }
        let args = supplied.order[..4]
            .iter()
            .map(|&index| values[index].take().unwrap())
            .collect();
        Ok(Expr {
            ty: Type::CompletedProcess,
            kind: ExprKind::Scope {
                body,
                value: Box::new(operation(
                    ProcessOperation::Run,
                    Type::CompletedProcess,
                    args,
                )),
                clear,
            },
        })
    }

    /// Path conversion is sealed and pure; optional paths retain their absence.
    fn process_directory(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(expression)?;
        let expected = self.types.optional(Type::Str);
        if value.ty == Type::Path {
            return self.coerce(
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Path(PathOperation::Text), vec![value]),
                },
                expected,
                expression,
            );
        }
        if self.types.payload(value.ty) == Some(Type::Path) {
            let slot = self.hidden(value.ty);
            let local = Expr {
                ty: value.ty,
                kind: ExprKind::Local(slot),
            };
            let test = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::IsNone, vec![local.clone()]),
            };
            let text = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(
                    Builtin::Path(PathOperation::Text),
                    vec![Expr {
                        ty: Type::Path,
                        kind: ExprKind::Unwrap(Box::new(local)),
                    }],
                ),
            };
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    value: Box::new(Expr {
                        ty: expected,
                        kind: ExprKind::Conditional(
                            Box::new(test),
                            Box::new(absent(expected)),
                            Box::new(self.coerce(text, expected, expression)?),
                        ),
                    }),
                    clear: vec![slot],
                },
            });
        }
        self.coerce(value, expected, expression)
    }

    pub(super) fn environ_copy(
        &self,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        if attribute.attr.as_str() != "copy" || !call.arguments.args.is_empty() {
            return Err(error(
                self.source,
                call,
                "os.environ supports only copy() without arguments",
            ));
        }
        Ok(operation(
            ProcessOperation::EnvironCopy,
            self.types.dict(Type::Str, Type::Str),
            vec![],
        ))
    }

    pub(super) fn process_attribute(
        &self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let (field, ty) = match attribute.attr.as_str() {
            "args" => (0, self.types.list(Type::Str)),
            "returncode" => (1, Type::Int),
            "stdout" => (2, Type::Str),
            "stderr" => (3, Type::Str),
            _ => {
                return Err(error(
                    self.source,
                    attribute,
                    "unsupported CompletedProcess attribute",
                ));
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Field(Box::new(receiver), field),
        })
    }

    pub(super) fn process_method(
        &self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        if attribute.attr.as_str() != "check_returncode" || !call.arguments.args.is_empty() {
            return Err(error(
                self.source,
                call,
                "CompletedProcess supports only check_returncode() without arguments",
            ));
        }
        Ok(operation(
            ProcessOperation::CheckReturnCode,
            Type::None,
            vec![receiver],
        ))
    }

    /// Handlers share the Exception type; the runtime guards each fixed payload
    /// projection and raises AttributeError for other exception categories.
    pub(super) fn process_error_attribute(
        &self,
        receiver: &Expr,
        attribute: &ast::ExprAttribute,
    ) -> Option<Expr> {
        let (field, ty) = match attribute.attr.as_str() {
            "cmd" => (0, self.types.list(Type::Str)),
            "returncode" => (1, Type::Int),
            "stdout" => (2, Type::Str),
            "stderr" => (3, Type::Str),
            "output" => (4, Type::Str),
            _ => return None,
        };
        Some(operation(
            ProcessOperation::ErrorField(field),
            ty,
            vec![receiver.clone()],
        ))
    }
}
