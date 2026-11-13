//! Binds native text methods without losing keyword evaluation order.

use super::calls::parameter;
use super::*;
use crate::ir::TextOperation;
use ruff_python_ast::CmpOp;

/// A contiguous slice has no fallible stride or user-defined indexing operation.
fn unit_string_slice(value: &Expr) -> bool {
    let ExprKind::Builtin(Builtin::Slice, parts) = &value.kind else {
        return false;
    };
    matches!(parts.as_slice(), [_, _, _, step] if ir::omitted(step)
        || matches!(&step.kind, ExprKind::Present(one) if matches!(&one.kind, ExprKind::Int(n) if n == "1")))
}

/// Evaluates each receiver and its bounds in Python order. An unsliced operand
/// uses omitted bounds, so the runtime can borrow its entire UTF-8 buffer.
pub(super) fn slice_equality(
    types: &Types,
    values: &mut Vec<Expr>,
    ops: &[ast::CmpOp],
) -> Option<Expr> {
    let ([left, right], [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)]) = (values.as_slice(), ops)
    else {
        return None;
    };
    if ![left, right]
        .iter()
        .all(|v| matches!(v.ty, Type::Str | Type::StrLiteral(_)))
        || !(unit_string_slice(left) || unit_string_slice(right))
    {
        return None;
    }
    let absent = Expr::absent(types.optional(Type::Int));
    let mut args = Vec::with_capacity(6);
    for value in values.drain(..) {
        if unit_string_slice(&value) {
            let ExprKind::Builtin(_, mut parts) = value.kind else {
                unreachable!()
            };
            parts.pop();
            args.extend(parts);
        } else {
            args.extend([value, absent.clone(), absent.clone()]);
        }
    }
    let equal = Expr {
        ty: Type::Bool,
        kind: ExprKind::Builtin(Builtin::StringSliceEqual, args),
    };
    Some(if *op == ast::CmpOp::NotEq {
        Expr::not(equal)
    } else {
        equal
    })
}

/// Constructs a fixed signature slot for the shared argument binder.
impl Checker<'_> {
    pub(super) fn unicode_character_property(
        &mut self,
        call: &ast::ExprCall,
        property: ir::UnicodeProperty,
    ) -> Result<Expr, String> {
        let name = property.name();
        calls::reject_unpacking(call, self.source)?;
        let ([arg], []) = (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) else {
            return self.one_positional_error(
                call,
                &format!("unicodedata.{name}"),
                "chr",
                Type::Str,
            );
        };
        let value = Self::string_value(self.evaluated_argument(arg)?);
        self.graph.checked_overload(self.module, call.range());
        if matches!(value.ty, Type::Str | Type::StrLiteral(_)) {
            // chr accepts lone surrogates, which cannot be materialized as native UTF-8.
            // Query these properties directly from the checked ordinal.
            let kind = if matches!(
                property,
                ir::UnicodeProperty::Category | ir::UnicodeProperty::Bidirectional
            ) && let ExprKind::Builtin(Builtin::Chr, ordinal) = value.kind
            {
                ExprKind::Builtin(Builtin::UnicodePropertyOfOrdinal(property), ordinal)
            } else {
                ExprKind::Builtin(
                    Builtin::Text(TextOperation::UnicodeProperty(property)),
                    vec![value],
                )
            };
            return Ok(Expr {
                ty: Type::Str,
                kind,
            });
        }
        let actual = match value.ty {
            Type::None => "None",
            Type::BuiltinClass => "type",
            Type::BytesIO => "_io.BytesIO",
            Type::StringIO => "_io.StringIO",
            Type::Deque(_) => "collections.deque",
            Type::RegexPattern | Type::RegexBytesPattern => "re.Pattern",
            Type::RegexMatch | Type::RegexBytesMatch => "re.Match",
            Type::Calendar(ir::CalendarType::Date) => "datetime.date",
            Type::Calendar(ir::CalendarType::DateTime) => "datetime.datetime",
            Type::Calendar(ir::CalendarType::Delta) => "datetime.timedelta",
            Type::Calendar(ir::CalendarType::Zone) => "datetime.timezone",
            Type::VarTuple(_) => "tuple",
            ty => self.exact_argument_type_name(ty).ok_or_else(|| {
                error(
                    self.source,
                    arg,
                    &format!("{name} requires str or an exact argument type"),
                )
            })?,
        };
        let actual = &actual[..actual.floor_char_boundary(50)];
        let message = format!("{name}() argument must be a unicode character, not {actual}");
        Ok(calls::type_error(Type::Str, vec![value], message))
    }

    pub(super) fn unicode_normalize(
        &mut self,
        call: &ast::ExprCall,
        check: bool,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let name = if check { "is_normalized" } else { "normalize" };
        let returns = if check { Type::Bool } else { Type::Str };
        let args = &call.arguments;
        let ([form, text], []) = (args.args.as_ref(), args.keywords.as_ref()) else {
            let message = if args.keywords.is_empty() {
                format!("{name} expected 2 arguments, got {}", args.args.len())
            } else {
                format!("unicodedata.{name}() takes no keyword arguments")
            };
            return self.positional_argument_error(
                call,
                &["form", "unistr"],
                returns,
                unpacking::string(message),
            );
        };
        self.graph.checked_overload(self.module, call.range());
        let mut scope = calls::ArgumentScope::default();
        let form = self.evaluated_argument(form)?;
        let text = self.evaluated_argument(text)?;
        let [form, text] = [form, text].map(|value| scope.hold(self, Self::string_value(value)));
        for (position, value) in [(1, &form), (2, &text)] {
            if !matches!(value.ty, Type::Str | Type::StrLiteral(_)) {
                let actual = match value.ty {
                    Type::None => "None",
                    ty => self.exact_argument_type_name(ty).ok_or_else(|| {
                        error(
                            self.source,
                            call,
                            "normalization requires str or an exact argument type",
                        )
                    })?,
                };
                return Ok(scope.finish(calls::type_error(
                    returns,
                    vec![],
                    format!("{name}() argument {position} must be str, not {actual}"),
                )));
            }
        }
        let normalized = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(
                Builtin::Text(TextOperation::Normalize),
                vec![form, text.clone()],
            ),
        };
        Ok(scope.finish(if check {
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Compare(vec![normalized, text], vec![CmpOp::Eq]),
            }
        } else {
            normalized
        }))
    }

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
        let flag = |name, value| parameter(name, Type::Bool, Some(Expr::boolean(value)));
        let parameters = [
            parameter("s", self.types.optional(Type::Str), None),
            flag("comments", false),
            flag("posix", true),
        ];
        let args = self.arguments_with(call, &parameters, None, |checker, i, arg| {
            if i == 0 {
                checker.optional_scalar_argument(arg, parameters[i].ty)
            } else {
                checker.expr_expected(arg, parameters[i].ty)
            }
        })?;
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
        let value = Self::enum_text(value);
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

    /// Keep the source method binding when the Python frame can pass the original operands.
    pub(super) fn bound_text_result(
        &mut self,
        call: &ast::ExprCall,
        field: &ast::ExprAttribute,
        value: Expr,
    ) -> Expr {
        let bound = match &value.kind {
            ExprKind::Builtin(Builtin::Text(_), args) => {
                call.arguments.is_empty() && args.len() == 1
            }
            ExprKind::Builtin(Builtin::StringReplace, _) => {
                registered::values::fixed_positional_arguments(call)
            }
            _ => false,
        };
        if bound {
            self.frame_arguments = FrameArguments::Python;
            if let Some(sites) = &mut self.fixed_sites {
                sites.method(call, field, &value);
            }
        }
        value
    }

    /// Bind splitlines and no-argument string methods after evaluating the receiver; an explicit
    /// keepends uses Python truth, including its fallible source-class protocol.
    pub(super) fn splitlines_or_unary_text(
        &mut self,
        receiver: Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let split = name == "splitlines";
        let mapping = matches!(name, "title" | "swapcase");
        let returns = if mapping {
            Type::Str
        } else if split {
            self.types.list(Type::Str)
        } else {
            Type::Bool
        };
        let args = &call.arguments;
        let count = args.args.len() + args.keywords.len();
        if count == 0 {
            let kind = if mapping {
                ExprKind::Builtin(
                    Builtin::Text(TextOperation::CaseMap {
                        title: name == "title",
                    }),
                    vec![receiver],
                )
            } else if split {
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
                    "isascii" => ir::TextPredicate::Ascii,
                    "isalpha" => ir::TextPredicate::Alpha,
                    "isdigit" => ir::TextPredicate::Digit,
                    "isnumeric" => ir::TextPredicate::Numeric,
                    "isspace" => ir::TextPredicate::Space,
                    "islower" => ir::TextPredicate::Lower,
                    "istitle" => ir::TextPredicate::Title,
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
        // Typeshed overloads splitlines and the case mappings. Their runtime
        // paths check these arguments after evaluating the operands.
        if split || mapping {
            self.graph.checked_overload(self.module, call.range());
        }
        if !split {
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
                return self.dictionary(dict, Some((Type::Int, Type::Int)), None);
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
                return self.dictionary(dict, Some(shape), None);
            }
        }
        self.expr(argument)
    }

    /// Convert the positional-only field width after evaluating the string receiver.
    pub(super) fn zfill(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
        copy: bool,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let ([width], []) = (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) else {
            let error = self.one_positional_error(call, "str.zfill", "width", Type::Str)?;
            self.graph.checked_overload(self.module, call.range());
            return Ok(self.evaluated_receiver(Some(receiver), error));
        };
        let value = self.expr(width)?;
        // An indirect descriptor call has no syntactic receiver to root a Python __index__.
        if self.graph.reachable_only()
            && !matches!(value.ty, Type::Int | Type::Bool | Type::IntEnum(_))
            && !matches!(call.func.as_ref(), ast::Expr::Attribute(attr) if attr.attr.as_str() == "zfill")
        {
            return Err(self.object_builtin_error("indirect str.zfill width", width));
        }
        let size = self
            .index_word(value, width, false)
            .map_err(|_| self.object_builtin_error("str.zfill width", width))?;
        self.graph.checked_argument_type(self.module, width.range());
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(
                Builtin::Text(TextOperation::ZFill { copy }),
                vec![receiver, size],
            ),
        })
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
        use ParameterKind::PositionalOnly;
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
                    parameter("sep", optional_string, Some(Expr::absent(optional_string))),
                    parameter("maxsplit", Type::Int, Some(Expr::integer(-1))),
                ],
            ),
            "strip" | "lstrip" | "rstrip" => (
                TextOperation::Strip(match name {
                    "lstrip" => 1,
                    "rstrip" => 2,
                    _ => 0,
                }),
                Type::Str,
                vec![PositionalOnly.parameter(
                    "chars",
                    optional_string,
                    Some(Expr::absent(optional_string)),
                )],
            ),
            "startswith" | "endswith" => (
                if name == "startswith" {
                    TextOperation::StartsWith
                } else {
                    TextOperation::EndsWith
                },
                Type::Bool,
                vec![PositionalOnly.parameter(
                    if name == "startswith" {
                        "prefix"
                    } else {
                        "suffix"
                    },
                    Type::Str,
                    None,
                )],
            ),
            "find" => {
                let mut parameters = vec![PositionalOnly.parameter("sub", Type::Str, None)];
                // An omitted bound belongs to the native search ABI. A Python
                // method selected on a live receiver sees only supplied operands.
                if !matches!(call.arguments.args.as_ref(), [sub] if !sub.is_starred_expr())
                    || !call.arguments.keywords.is_empty()
                {
                    let start = self.types.optional(Type::Int);
                    parameters.push(PositionalOnly.parameter(
                        "start",
                        start,
                        Some(Expr::absent(start)),
                    ));
                }
                (TextOperation::Find, Type::Int, parameters)
            }
            "partition" => (
                TextOperation::Partition,
                self.types.tuple(vec![Type::Str; 3]),
                vec![PositionalOnly.parameter("sep", Type::Str, None)],
            ),
            _ => unreachable!("only supported text methods reach this binder"),
        };
        let arguments =
            self.arguments_with(call, &parameters, Some(receiver), |checker, i, arg| {
                let parameter = &parameters[i];
                if name == "find" && parameter.name == "start" {
                    let bound = checker.optional_scalar_argument(arg, parameter.ty)?;
                    // An Int is already the nonzero word accepted by the search ABI.
                    Ok(match bound.kind {
                        ExprKind::Present(value) if value.ty == Type::Int => *value,
                        _ => bound,
                    })
                } else {
                    checker.direct_argument(arg, parameter.ty, parameter.sign_only)
                }
            })?;
        Ok(self.ordered_builtin(Builtin::Text(operation), returns, arguments))
    }

    /// Check each conditional arm in context and convert it to the requested scalar.
    fn optional_scalar_argument(
        &mut self,
        node: &ast::Expr,
        optional: Type,
    ) -> Result<Expr, String> {
        if let ast::Expr::If(branch) = node {
            let value = self.conditional_with(branch, false, |checker, arm| {
                checker.optional_scalar_argument(arm, optional)
            })?;
            return self.coerce(value, optional, node);
        }
        let scalar = self.types.payload(optional).unwrap();
        let value = self.expr_expected_if(node, optional, |checker, ty| {
            Self::scalar_conversion(checker.types.payload(ty).unwrap_or(ty), scalar).is_some()
        })?;
        let value = self.optional_scalar_value(value, scalar, node)?;
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
        let parameters = [parameter(name, Type::Str, None)];
        let arguments = self.arguments(call, &parameters, None)?;
        Ok(self.ordered_builtin(Builtin::Text(operation), Type::Str, arguments))
    }
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn compare_only_contiguous_exact_string_slices() {
        for (expression, fused) in [
            ("a[lo:hi] == b", true),
            ("a != b[lo:hi:1]", true),
            ("a[:hi] == b[lo:]", true),
            ("a[::lo] == b", false),
            ("a[::-1] != b", false),
            ("a[:hi] == b == a", false),
            ("a[:hi] < b", false),
        ] {
            let source = format!(
                "def check(a: str, b: str, lo: int, hi: int) -> bool:\n    return {expression}\n"
            );
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.contains("call i32 @imre_str_slice_equal("),
                fused,
                "{expression}"
            );
        }
        let source = "def check(a: bytes, b: bytes) -> bool:\n    return a[:2] == b\n";
        let llvm = compile(source, &CompileOptions::default()).unwrap();
        assert!(!llvm.contains("call i32 @imre_str_slice_equal("));
    }
}
