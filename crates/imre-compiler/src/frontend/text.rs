//! Binds native text methods without losing keyword evaluation order.

use super::*;
use crate::ir::TextOperation;

/// Constructs a fixed signature slot for the shared argument binder.
fn parameter(name: &str, ty: Type, kind: ParameterKind, default: Option<Expr>) -> Parameter {
    Parameter {
        name: name.to_owned(),
        ty,
        kind,
        default,
        sign_only: false,
    }
}

/// Supplies an absent optional value without changing its concrete type.
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
    pub(super) fn shlex_split(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let flag = |name, value| {
            parameter(
                name,
                Type::Bool,
                ParameterKind::PositionalOrKeyword,
                Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(value),
                }),
            )
        };
        let parameters = [
            parameter(
                "s",
                self.types.optional(Type::Str),
                ParameterKind::PositionalOrKeyword,
                None,
            ),
            flag("comments", false),
            flag("posix", true),
        ];
        let args = self.arguments(call, &parameters, None)?;
        let result = self.types.list(Type::Str);
        Ok(self.ordered_builtin(Builtin::Text(TextOperation::ShlexSplit), result, args))
    }

    pub(super) fn shlex_call(
        &mut self,
        call: &ast::ExprCall,
        operation: Intrinsic,
    ) -> Result<Expr, String> {
        let quote = matches!(operation, Intrinsic::ShlexQuote);
        let name = if quote { "s" } else { "split_command" };
        let source = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([arg], []) => arg,
            ([], [kw]) if kw.arg.as_deref() == Some(name) => &kw.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    &format!("expected one argument {name:?}"),
                ));
            }
        };
        if quote {
            let value = self.expr(source)?;
            self.shlex_quote_value(value, call)
        } else {
            self.join_parts(unpacking::string(" "), source, call, true)
        }
    }

    /// CPython checks truth before validating the type: every false value quotes as two apostrophes.
    pub(super) fn shlex_quote_value(
        &mut self,
        value: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if matches!(value.ty, Type::Str | Type::StrLiteral(_)) {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Text(TextOperation::ShlexQuote), vec![value]),
            });
        }
        if let Some(inner) = self.types.payload(value.ty) {
            let slot = self.hidden(value.ty);
            let local = self.local(slot);
            let absent = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::IsNone, vec![local.clone()]),
            };
            let quoted = self.shlex_quote_value(
                Expr {
                    ty: inner,
                    kind: ExprKind::Unwrap(Box::new(local)),
                },
                call,
            )?;
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    value: Box::new(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Conditional(
                            Box::new(absent),
                            Box::new(unpacking::string("''")),
                            Box::new(quoted),
                        ),
                    }),
                    clear: vec![slot],
                },
            });
        }
        let name = match value.ty {
            Type::None => "NoneType",
            Type::Bool => "bool",
            Type::Int => "int",
            Type::Float => "float",
            Type::Bytes => "bytes",
            Type::ListInt | Type::List(_) => "list",
            Type::Tuple(_) => "tuple",
            Type::Dict(_) => "dict",
            Type::Set(_) => "set",
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "shlex.quote needs a supported concrete argument type",
                ));
            }
        };
        let exception = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(format!(
                    "expected string object, got '{name}'"
                )))),
            },
        };
        let raised = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::RaiseException, vec![exception]),
        };
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Conditional(
                Box::new(self.truth_value(value)),
                Box::new(raised),
                Box::new(unpacking::string("''")),
            ),
        })
    }

    /// Checks the bounded native signatures while retaining source evaluation order.
    pub(super) fn text_method(
        &mut self,
        receiver: Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        use ParameterKind::{PositionalOnly, PositionalOrKeyword};
        if matches!(name, "startswith" | "endswith")
            && call.arguments.keywords.is_empty()
            && let [argument] = call.arguments.args.as_ref()
        {
            let affix = self.expr(argument)?;
            let op = if name == "startswith" {
                TextOperation::StartsWith
            } else {
                TextOperation::EndsWith
            };
            let check = |text, affix| Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::Text(op), vec![text, affix]),
            };
            let Some(types) = self.types.fields(affix.ty) else {
                return Ok(check(receiver, self.coerce(affix, Type::Str, argument)?));
            };
            if !types
                .iter()
                .all(|ty| matches!(ty, Type::Str | Type::StrLiteral(_)))
            {
                return Err(error(
                    self.source,
                    argument,
                    "prefix or suffix tuple elements must be str",
                ));
            }
            let text = self.hidden(receiver.ty);
            let mut body = vec![Statement::Assign(text, receiver)];
            let mut clear = vec![text];
            let affixes = if let ExprKind::Tuple(values) = &affix.kind
                && values
                    .iter()
                    .all(|value| matches!(value.kind, ExprKind::Str(_)))
            {
                values.clone()
            } else {
                let tuple = self.hidden(affix.ty);
                body.push(Statement::Assign(tuple, affix));
                clear.insert(0, tuple);
                types
                    .iter()
                    .enumerate()
                    .map(|(i, &ty)| Expr {
                        ty,
                        kind: ExprKind::Field(Box::new(self.local(tuple)), i),
                    })
                    .collect()
            };
            let matches = affixes
                .into_iter()
                .map(|affix| check(self.local(text), affix))
                .collect::<Vec<_>>();
            let kind = if matches.is_empty() {
                ExprKind::Bool(false)
            } else {
                ExprKind::Boolean(false, matches)
            };
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Scope {
                    body,
                    value: Box::new(Expr {
                        ty: Type::Bool,
                        kind,
                    }),
                    clear,
                },
            });
        }
        // Existing infallible entry points avoid a pending-error check for these
        // common signatures, whose arguments cannot request an invalid split.
        if call.arguments.args.is_empty()
            && call.arguments.keywords.is_empty()
            && matches!(name, "split" | "rstrip")
        {
            return Ok(Expr {
                ty: if name == "split" {
                    self.types.list(Type::Str)
                } else {
                    Type::Str
                },
                kind: ExprKind::Builtin(
                    if name == "split" {
                        Builtin::Split
                    } else {
                        Builtin::RStrip
                    },
                    vec![receiver],
                ),
            });
        }
        let optional_string = self.types.optional(Type::Str);
        let (operation, returns, parameters) = match name {
            "lower" => (TextOperation::Lower, Type::Str, vec![]),
            "capitalize" => (TextOperation::Capitalize, Type::Str, vec![]),
            "split" | "rsplit" => (
                TextOperation::Split(name == "rsplit"),
                self.types.list(Type::Str),
                vec![
                    parameter(
                        "sep",
                        optional_string,
                        PositionalOrKeyword,
                        Some(absent(optional_string)),
                    ),
                    parameter(
                        "maxsplit",
                        Type::Int,
                        PositionalOrKeyword,
                        Some(Expr {
                            ty: Type::Int,
                            kind: ExprKind::Int("-1".to_owned()),
                        }),
                    ),
                ],
            ),
            "strip" | "lstrip" | "rstrip" => (
                TextOperation::Strip(match name {
                    "lstrip" => 1,
                    "rstrip" => 2,
                    _ => 0,
                }),
                Type::Str,
                vec![parameter(
                    "chars",
                    optional_string,
                    PositionalOnly,
                    Some(absent(optional_string)),
                )],
            ),
            "startswith" | "endswith" => (
                if name == "startswith" {
                    TextOperation::StartsWith
                } else {
                    TextOperation::EndsWith
                },
                Type::Bool,
                vec![parameter(
                    if name == "startswith" {
                        "prefix"
                    } else {
                        "suffix"
                    },
                    Type::Str,
                    PositionalOnly,
                    None,
                )],
            ),
            "find" => {
                let optional_index = self.types.optional(Type::Int);
                (
                    TextOperation::Find,
                    Type::Int,
                    vec![
                        parameter("sub", Type::Str, PositionalOnly, None),
                        parameter(
                            "start",
                            optional_index,
                            PositionalOnly,
                            Some(absent(optional_index)),
                        ),
                    ],
                )
            }
            "partition" => (
                TextOperation::Partition,
                self.types.tuple(vec![Type::Str; 3]),
                vec![parameter("sep", Type::Str, PositionalOnly, None)],
            ),
            _ => unreachable!("only supported text methods reach this binder"),
        };
        let arguments = self.arguments(call, &parameters, Some(receiver))?;
        Ok(self.ordered_builtin(Builtin::Text(operation), returns, arguments))
    }

    /// Binds standard-library functions taking one positional or named string.
    pub(super) fn text_function(
        &mut self,
        call: &ast::ExprCall,
        name: &str,
        operation: TextOperation,
    ) -> Result<Expr, String> {
        let parameters = [parameter(
            name,
            Type::Str,
            ParameterKind::PositionalOrKeyword,
            None,
        )];
        let arguments = self.arguments(call, &parameters, None)?;
        Ok(self.ordered_builtin(Builtin::Text(operation), Type::Str, arguments))
    }
}
