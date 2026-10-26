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
    /// Convert a nullable string to text and evaluate its operand once.
    pub(super) fn optional_text(&mut self, value: Expr) -> Expr {
        let mut scope = calls::ArgumentScope::default();
        let local = scope.hold(self, value);
        let absent = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::IsNone, vec![local.clone()]),
        };
        let present = Expr {
            ty: Type::Str,
            kind: ExprKind::Unwrap(Box::new(local)),
        };
        scope.finish(Expr {
            ty: Type::Str,
            kind: ExprKind::Conditional(
                Box::new(absent),
                Box::new(unpacking::string("None")),
                Box::new(present),
            ),
        })
    }

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
            self.join_parts(
                unpacking::string(" "),
                source,
                call,
                stdlib::JoinStyle::Shell,
            )
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
            Type::Deque(_) => "deque",
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

    /// Bind splitlines and no-argument text predicates after evaluating the receiver; an explicit
    /// keepends uses Python truth, including its fallible source-class protocol.
    pub(super) fn splitlines_or_predicate(
        &mut self,
        receiver: Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let split = name == "splitlines";
        let returns = if split {
            self.types.list(Type::Str)
        } else {
            Type::Bool
        };
        let args = &call.arguments;
        let count = args.args.len() + args.keywords.len();
        if count == 0 {
            let kind = if split {
                // Preserve the existing bytes.decode().splitlines() path and
                // direct-iteration fusion for the no-argument method.
                match receiver.kind {
                    _ if let Some(separator) = self.joined_lines.splits.get(&call.range()) => {
                        ExprKind::Builtin(
                            Builtin::JoinedSplitLines,
                            vec![
                                receiver,
                                Expr {
                                    ty: Type::Str,
                                    kind: ExprKind::Str(separator.clone()),
                                },
                            ],
                        )
                    }
                    ExprKind::Builtin(Builtin::Decode, bytes) => {
                        ExprKind::Builtin(Builtin::DecodeSplitLines, bytes)
                    }
                    _ => ExprKind::Builtin(Builtin::SplitLines, vec![receiver]),
                }
            } else {
                let predicate = match name {
                    "isalnum" => ir::TextPredicate::Alnum,
                    "isspace" => ir::TextPredicate::Space,
                    "isupper" => ir::TextPredicate::Upper,
                    _ => unreachable!("resolved text predicate"),
                };
                ExprKind::Builtin(
                    Builtin::Text(TextOperation::Predicate(predicate)),
                    vec![receiver],
                )
            };
            return Ok(Expr { ty: returns, kind });
        }
        // Typeshed limits keepends to bool; the runtime truth-tests any
        // represented value. Defer only this resolved method's overload.
        if split {
            self.graph.checked_overload(self.module, call.range());
        } else {
            for arg in &args.args {
                self.graph.checked_excess_argument(self.module, arg.range());
            }
            self.graph
                .checked_keywords(self.module, args.keywords.iter().map(Ranged::range));
        }
        if split && count == 1 {
            let flag = match (args.args.as_ref(), args.keywords.as_ref()) {
                ([flag], []) => Some(flag),
                ([], [kw]) if kw.arg.as_deref() == Some("keepends") => Some(&kw.value),
                _ => None,
            };
            if let Some(flag) = flag {
                let value = self.expr(flag)?;
                let keep = self.truth_value(value);
                return Ok(Expr {
                    ty: returns,
                    kind: ExprKind::Builtin(
                        Builtin::Text(TextOperation::SplitLineEnds),
                        vec![receiver, keep],
                    ),
                });
            }
        }
        // Invalid signatures are checked after evaluating all operands, but do
        // not truth-test the would-be option.
        let mut values = vec![receiver];
        for arg in &args.args {
            values.push(self.expr(arg)?);
        }
        for kw in &args.keywords {
            values.push(self.expr(&kw.value)?);
        }
        let message = if !split && !args.keywords.is_empty() {
            format!("str.{name}() takes no keyword arguments")
        } else if !split {
            format!("str.{name}() takes no arguments ({count} given)")
        } else if count > 1 {
            let qualifier = if args.args.is_empty() { "keyword " } else { "" };
            format!("splitlines() takes at most 1 {qualifier}argument ({count} given)")
        } else {
            format!(
                "splitlines() got an unexpected keyword argument '{}'",
                args.keywords[0].arg.as_deref().unwrap()
            )
        };
        Ok(calls::type_error(returns, values, message))
    }

    fn translation_argument(&mut self, argument: &ast::Expr) -> Result<Expr, String> {
        if let ast::Expr::Dict(dict) = argument {
            if dict.items.is_empty() {
                // The empty literal has no observable key/value layout.
                return self.dictionary(dict, Some((Type::Int, Type::Int)));
            }
            // Choose the storage before lowering any operands. The ordinary dictionary
            // constructor then still evaluates every key and value once, in source order.
            let scalar = |e: &ast::Expr| {
                if matches!(e, ast::Expr::NoneLiteral(_)) {
                    Some(Type::None)
                } else {
                    self.graph.session.scalar_type(self.module, e)
                }
            };
            let shape = (|| {
                let key = scalar(dict.items[0].key.as_ref()?)?;
                if !self.types.hashable_key(key) {
                    return None;
                }
                let mut members = Vec::new();
                let mut nullable = false;
                for item in &dict.items {
                    if scalar(item.key.as_ref()?)? != key {
                        return None;
                    }
                    let value = scalar(&item.value)?;
                    if value == Type::None {
                        nullable = true;
                    } else if !members.contains(&value) {
                        members.push(value);
                    }
                }
                let value = match members.as_slice() {
                    [] => Type::None,
                    [one] => *one,
                    [Type::Str, Type::Int] | [Type::Int, Type::Str] => self.types.string_or_int(),
                    members => self.types.union(members)?,
                };
                Some((
                    key,
                    if nullable {
                        self.types.optional(value)
                    } else {
                        value
                    },
                ))
            })();
            if let Some(shape) = shape {
                return self.dictionary(dict, Some(shape));
            }
        }
        self.expr(argument)
    }

    /// Preserve ordinal lookup semantics only for plain tables without Python callbacks.
    pub(super) fn translate_method(
        &mut self,
        source: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = &call.arguments;
        if let ([table], []) = (args.args.as_ref(), args.keywords.as_ref()) {
            let value = self.translation_argument(table)?;
            let Some((key, member)) = self.types.mapping(value.ty) else {
                return Err(error(
                    self.source,
                    table,
                    "str.translate requires an exact plain dictionary",
                ));
            };
            let member = self.types.payload(member).unwrap_or(member);
            if self.types.is_counter(value.ty)
                || self.types.is_default_list_dict(value.ty)
                || !matches!(key, Type::Int | Type::Str | Type::Bytes | Type::Tuple(_))
                || matches!(member, Type::StrEnum(_))
            {
                return Err(error(
                    self.source,
                    table,
                    "unsupported str.translate dictionary layout or missing-key protocol",
                ));
            }
            self.graph.checked_overload(self.module, call.range());
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(
                    Builtin::Text(TextOperation::Translate),
                    vec![source, value],
                ),
            });
        }
        self.graph
            .checked_missing_argument(self.module, call.range());
        for kw in &args.keywords {
            if kw.arg.as_deref() == Some("table") {
                self.graph
                    .checked_positional_keyword(self.module, kw.range());
            }
        }
        self.graph
            .checked_keywords(self.module, args.keywords.iter().map(Ranged::range));
        for extra in args.args.iter().skip(1) {
            self.graph
                .checked_excess_argument(self.module, extra.range());
        }
        let mut values = vec![source];
        for argument in &args.args {
            values.push(self.translation_argument(argument)?);
        }
        for kw in &args.keywords {
            values.push(self.translation_argument(&kw.value)?);
        }
        let message = if args.keywords.is_empty() {
            format!(
                "str.translate() takes exactly one argument ({} given)",
                args.args.len()
            )
        } else {
            "str.translate() takes no keyword arguments".into()
        };
        values.push(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(message))),
            },
        });
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::RaiseException, values),
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
            "upper" => (TextOperation::Upper, Type::Str, vec![]),
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
        let arguments =
            self.arguments_with(call, &parameters, Some(receiver), |checker, i, arg| {
                let parameter = &parameters[i];
                if name == "find" && parameter.name == "start" {
                    checker.find_start(arg, parameter.ty)
                } else {
                    checker.direct_argument(arg, parameter.ty, parameter.sign_only)
                }
            })?;
        Ok(self.ordered_builtin(Builtin::Text(operation), returns, arguments))
    }

    /// Check each search bound with the expected integer type, including conditional arms.
    fn find_start(&mut self, node: &ast::Expr, optional: Type) -> Result<Expr, String> {
        if let ast::Expr::If(branch) = node {
            let value = self.conditional_with(branch, false, |checker, arm| {
                checker.find_start(arm, optional)
            })?;
            return self.coerce(value, optional, node);
        }
        let value = self.expr_expected_if(node, optional, |checker, ty| {
            matches!(checker.types.payload(ty).unwrap_or(ty), Type::IntEnum(_))
        })?;
        let value = self.optional_enum_integer(value, node)?;
        self.coerce(value, optional, node)
    }

    /// CPython turns a missing or incompatible split('\n') into a catchable TypeError.
    pub(super) fn dedent_call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let source = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([arg], []) => arg,
            ([], [kw]) if kw.arg.as_deref() == Some("text") => &kw.value,
            _ => return self.text_function(call, "text", TextOperation::Dedent),
        };
        let value = self.expr(source)?;
        let name = match value.ty {
            Type::None => "NoneType",
            Type::Int => "int",
            Type::Bool => "bool",
            Type::Float => "float",
            Type::Bytes => "bytes",
            Type::List(_) | Type::ListInt | Type::ByteOutput => "list",
            Type::Dict(_) => "dict",
            Type::Set(_) => "set",
            Type::Tuple(_) => "tuple",
            _ => {
                let value = self.coerce(value, Type::Str, source)?;
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Text(TextOperation::Dedent), vec![value]),
                });
            }
        };
        let exception = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(format!(
                    "expected str object, not '{name}'"
                )))),
            },
        };
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::RaiseException, vec![value, exception]),
        })
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
