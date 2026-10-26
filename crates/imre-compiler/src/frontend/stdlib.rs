//! Eager typed library operations reuse the existing iterator and callback IR.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum JoinStyle {
    Plain,
    Shell,
    WindowsCommand,
}

#[derive(Clone, Copy)]
pub(super) enum ReverseSequence {
    List,
    Deque,
}

/// All leaves must be plain local reads so no later operand can change the
/// keys captured by an earlier union. Bound the number of membership probes.
fn local_key_union(expr: &Expr, dictionaries: &mut Vec<Expr>) -> bool {
    match &expr.kind {
        ExprKind::Builtin(Builtin::KeyUnion, sources) => sources
            .iter()
            .all(|source| local_key_union(source, dictionaries)),
        ExprKind::Local(_) if matches!(expr.ty, Type::Dict(_)) && dictionaries.len() < 4 => {
            dictionaries.push(expr.clone());
            true
        }
        _ => false,
    }
}

impl Checker<'_> {
    /// Captures arguments before acquisition, materializes the source before key
    /// calls, and keeps the source argument rooted until the sorted result escapes.
    pub(super) fn sorted_call(
        &mut self,
        call: &ast::ExprCall,
        element: Option<Type>,
    ) -> Result<Expr, String> {
        if call.arguments.args.len() != 1 {
            return Err(error(
                self.source,
                call,
                "sorted requires one positional iterable",
            ));
        }
        if let Some(sorted) = self.string_list_sorted_suffix(call, element)? {
            return Ok(sorted);
        }
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let input = self.expansion_input(&call.arguments.args[0], element)?;
        if call.arguments.keywords.is_empty()
            && let Iteration::Container {
                iterable,
                items: false,
                start: None,
            } = &input
            && self.types.set_element(iterable.ty) == Some(Type::Str)
        {
            let mut dictionaries = Vec::new();
            if local_key_union(iterable, &mut dictionaries) && dictionaries.len() >= 2 {
                return Ok(Expr {
                    ty: self.types.list(Type::Str),
                    kind: ExprKind::Builtin(Builtin::SortedStringKeys, dictionaries),
                });
            }
        }
        let input = self.capture_consumer_input(input, &mut body, &mut clear);
        let mut input = self.prepare_iteration(input, call, false)?;
        self.hint_empty_iteration(&input, Some(element.unwrap_or(Type::Int)));
        let element = self.locals[input.value];
        body.append(&mut input.evaluate);
        clear.extend(&input.clear);

        let (callback, reverse) = self.sort_options(call, element, &mut body, &mut clear)?;
        let result_type = self.types.list(element);
        let result = self.hidden(result_type);
        clear.push(result);
        body.push(Statement::Assign(
            result,
            Expr {
                ty: result_type,
                kind: ExprKind::List(Vec::new()),
            },
        ));
        body.extend(
            self.expand_items(Iteration::Prepared(input), call, |checker, value| {
                Ok(vec![Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Append, vec![checker.local(result), value]),
                })])
            })?,
        );

        self.sort_values(call, result, callback, reverse, &mut body, &mut clear)?;
        Ok(Expr {
            ty: result_type,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear,
            },
        })
    }

    /// Bind reverse on an instance or as a descriptor. The receiver and
    /// every argument are evaluated before a rejected call raises TypeError.
    pub(super) fn sequence_reverse(
        &mut self,
        sequence: ReverseSequence,
        receiver: Option<Expr>,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = &call.arguments;
        let skip = usize::from(receiver.is_none());
        let mut receiver = if let Some(receiver) = receiver {
            Some(receiver)
        } else {
            args.args
                .first()
                .map(|arg| self.evaluated_argument(arg))
                .transpose()?
        };
        let (owner, descriptor, builtin) = match sequence {
            ReverseSequence::List => {
                if let Some(value) = receiver.take() {
                    receiver = Some(self.json_mutation_receiver(value, call)?);
                }
                ("list", "list", Builtin::ListReverse)
            }
            ReverseSequence::Deque => ("deque", "collections.deque", Builtin::DequeReverse),
        };
        let accepts = |ty| match sequence {
            ReverseSequence::List => self.types.element(ty).is_some(),
            ReverseSequence::Deque => self.types.deque_element(ty).is_some(),
        };
        let extra = args.args.len().saturating_sub(skip);
        let message = match receiver.as_ref() {
            None => {
                self.graph
                    .checked_missing_argument(self.module, call.range());
                format!("unbound method {owner}.reverse() needs an argument")
            }
            Some(value) if !accepts(value.ty) => {
                let name = match value.ty {
                    Type::None => "NoneType".to_owned(),
                    Type::PlainObject => "object".to_owned(),
                    Type::Deque(_) => "collections.deque".to_owned(),
                    Type::ListInt | Type::List(_) => "list".to_owned(),
                    Type::StrLiteral(_) => "str".to_owned(),
                    Type::Bool | Type::Int | Type::Float | Type::Str | Type::Bytes => {
                        self.type_name(value.ty)
                    }
                    Type::Set(_) => "set".to_owned(),
                    Type::Tuple(_) if self.types.namedtuple_definition(value.ty).is_none() => {
                        "tuple".to_owned()
                    }
                    Type::Dict(_)
                        if !self.types.is_counter(value.ty)
                            && !self.types.is_default_list_dict(value.ty) =>
                    {
                        "dict".to_owned()
                    }
                    _ => {
                        return Err(error(
                            self.source,
                            call,
                            &format!(
                                "{owner}.reverse requires an exact native {owner} or known descriptor receiver"
                            ),
                        ));
                    }
                };
                format!(
                    "descriptor 'reverse' for '{descriptor}' objects doesn't apply to a '{name}' object"
                )
            }
            Some(_) if !args.keywords.is_empty() => {
                format!("{owner}.reverse() takes no keyword arguments")
            }
            Some(_) if extra != 0 => {
                format!("{owner}.reverse() takes no arguments ({extra} given)")
            }
            Some(_) => {
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(builtin, vec![receiver.unwrap()]),
                });
            }
        };
        self.graph.checked_overload(self.module, call.range());
        self.graph
            .checked_keywords(self.module, args.keywords.iter().map(Ranged::range));
        for arg in args.args.iter().skip(skip) {
            self.graph.checked_excess_argument(self.module, arg.range());
        }
        let mut values: Vec<_> = receiver.into_iter().collect();
        for arg in args
            .args
            .iter()
            .skip(skip)
            .chain(args.keywords.iter().map(|kw| &kw.value))
        {
            values.push(self.evaluated_argument(arg)?);
        }
        Ok(calls::type_error(Type::None, values, message))
    }

    /// Keeps the receiver identity and argument order. A key can observe or mutate
    /// the temporarily empty list, so restoration must run on every completion.
    pub(super) fn list_sort(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if !call.arguments.args.is_empty() {
            return Err(error(
                self.source,
                call,
                "list.sort accepts only keyword arguments",
            ));
        }
        let ty = receiver.ty;
        let element = self.types.element(ty).unwrap();
        let target = self.hidden(ty);
        let mut body = vec![Statement::Assign(target, receiver)];
        let mut clear = vec![target];
        let (callback, reverse) = self.sort_options(call, element, &mut body, &mut clear)?;
        if callback.is_some() {
            let original = self.hidden(ty);
            let completed = self.hidden(Type::Bool);
            clear.extend([original, completed]);
            body.push(Statement::Assign(
                original,
                Expr {
                    ty,
                    kind: ExprKind::Builtin(Builtin::SortDetach, vec![self.local(target)]),
                },
            ));
            body.push(Statement::Assign(
                completed,
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(false),
                },
            ));
            let mut sorting = Vec::new();
            self.sort_values(call, original, callback, reverse, &mut sorting, &mut clear)?;
            sorting.push(Statement::Assign(
                completed,
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(true),
                },
            ));
            body.push(Statement::Try {
                body: sorting,
                handlers: Vec::new(),
                orelse: Vec::new(),
                finalbody: vec![Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(
                        Builtin::SortRestore,
                        vec![
                            self.local(target),
                            self.local(original),
                            self.local(completed),
                        ],
                    ),
                })],
            });
        } else {
            self.sort_values(call, target, callback, reverse, &mut body, &mut clear)?;
        }
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body,
                value: Box::new(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }),
                clear,
            },
        })
    }

    fn sort_options(
        &mut self,
        call: &ast::ExprCall,
        element: Type,
        body: &mut Vec<Statement>,
        clear: &mut Vec<usize>,
    ) -> Result<(Option<usize>, Expr), String> {
        let mut keywords = HashSet::new();
        for keyword in &call.arguments.keywords {
            let name = keyword.arg.as_ref().map(|name| name.as_str());
            if !matches!(name, Some("key" | "reverse")) || !keywords.insert(name) {
                return Err(error(
                    self.source,
                    keyword,
                    "sort accepts key and reverse once each",
                ));
            }
        }
        let mut callback = None;
        let mut reverse = Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(false),
        };
        for keyword in &call.arguments.keywords {
            let value = match keyword.arg.as_ref().unwrap().as_str() {
                "key" => self.sort_callback(&keyword.value, element)?,
                "reverse" => self.expr_expected(&keyword.value, Type::Bool)?,
                _ => unreachable!("keyword names were checked"),
            };
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
            if keyword.arg.as_ref().unwrap() == "reverse" {
                reverse = self.local(slot);
            } else if self.locals[slot] != Type::None {
                callback = Some(slot);
            }
        }
        Ok((callback, reverse))
    }

    /// Collects keys in forward order before applying one stable permutation.
    fn sort_values(
        &mut self,
        call: &ast::ExprCall,
        result: usize,
        callback: Option<usize>,
        reverse: Expr,
        body: &mut Vec<Statement>,
        clear: &mut Vec<usize>,
    ) -> Result<(), String> {
        let element = self.types.element(self.locals[result]).unwrap();
        if let Some(slot) = callback
            && let Type::CmpKey(frame, _) = self.locals[slot]
        {
            body.push(Statement::Expr(self.sort_cmp_key(
                self.local(result),
                self.local(slot),
                frame,
                element,
                reverse,
                call,
            )?));
            return Ok(());
        }
        let keys = if let Some(callback) = callback {
            let (parameters, key) = self.types.signature(self.locals[callback]).unwrap();
            let [parameter] = parameters.as_slice() else {
                return Err(error(
                    self.source,
                    call,
                    "sorted key requires one positional parameter",
                ));
            };
            self.sortable_key(key, call)?;
            let key_list = self.types.list(key);
            let keys = self.hidden(key_list);
            clear.push(keys);
            body.push(Statement::Assign(
                keys,
                Expr {
                    ty: key_list,
                    kind: ExprKind::List(Vec::new()),
                },
            ));
            body.extend(self.expand_items(
                Iteration::Container {
                    iterable: self.local(result),
                    items: false,
                    start: None,
                },
                call,
                |checker, value| {
                    let key = Expr {
                        ty: key,
                        kind: ExprKind::Invoke {
                            callable: Box::new(checker.local(callback)),
                            args: ir::CallArgs::positional(vec![
                                checker.coerce(value, *parameter, call)?,
                            ]),
                        },
                    };
                    Ok(vec![Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::Append, vec![checker.local(keys), key]),
                    })])
                },
            )?);
            keys
        } else {
            self.sortable_key(element, call)?;
            result
        };
        body.push(Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(
                Builtin::SortByKeys,
                vec![self.local(result), self.local(keys), reverse],
            ),
        }));
        Ok(())
    }

    /// Learns one callback input from the already checked iterable. Scalar key
    /// results have a sealed total order, so sorting never invokes user comparison.
    fn sort_callback(&mut self, expression: &ast::Expr, input: Type) -> Result<Expr, String> {
        let value = if let ast::Expr::Lambda(lambda) = expression {
            self.lambda_value(lambda, Some(vec![input]), None)?
        } else if let Some(Binding::Definition(definition)) = self.resolve(expression)
            && let Some(signature) = self.functions.get(&definition).cloned()
        {
            if signature.params.len() == 1
                && let Some(inference) = self.inference
            {
                inference.learn_parameter(signature.id, 0, input);
            }
            self.callable_value(&signature, None, None, expression)?
        } else {
            self.expr(expression)?
        };
        if !matches!(value.ty, Type::None | Type::CmpKey(..))
            && self.types.signature(value.ty).is_none()
        {
            return Err(error(
                self.source,
                expression,
                "sorted key requires an exact Callable or None",
            ));
        }
        Ok(value)
    }

    fn sortable_key(&self, ty: Type, node: &impl Ranged) -> Result<(), String> {
        if self.types.ordered_key(ty) {
            Ok(())
        } else {
            Err(error(
                self.source,
                node,
                "sorted keys require int, bool, str, bytes, or fixed tuples of ordered scalar fields",
            ))
        }
    }

    /// Joins only after exhausting the iterable; callbacks run while both the
    /// separator and all previously yielded string owners remain rooted.
    pub(super) fn join_call(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [source] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "str.join requires one string iterable",
            ));
        };
        if self.joined_lines.finishes_buffer(call.range()) {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::JoinBufferFinish, vec![self.expr(source)?]),
            });
        }
        self.join_parts(receiver, source, call, JoinStyle::Plain)
    }

    /// Quote or convert each yielded item before asking for the next one.
    pub(super) fn join_parts(
        &mut self,
        receiver: Expr,
        source: &ast::Expr,
        call: &ast::ExprCall,
        style: JoinStyle,
    ) -> Result<Expr, String> {
        let input = if style == JoinStyle::WindowsCommand
            && let ast::Expr::List(display) = source
            && !display
                .elts
                .iter()
                .any(|item| matches!(item, ast::Expr::Starred(_)))
        {
            Iteration::Container {
                iterable: self.process_argument_list(display)?,
                items: false,
                start: None,
            }
        } else {
            self.expansion_input(source, (style == JoinStyle::Plain).then_some(Type::Str))?
        };
        if style == JoinStyle::WindowsCommand
            && let Iteration::Container { iterable, .. } = &input
            && let Some(name) = match iterable.ty {
                Type::None => Some("NoneType"),
                Type::Int => Some("int"),
                Type::Bool => Some("bool"),
                Type::Float => Some("float"),
                Type::Path => Some("PosixPath"),
                Type::PurePath(ir::PathFlavor::Posix) => Some("PurePosixPath"),
                Type::PurePath(ir::PathFlavor::Windows) => Some("PureWindowsPath"),
                _ => None,
            }
        {
            return self.sys_type_error(
                call,
                Type::Str,
                format!("'{name}' object is not iterable"),
            );
        }
        if let Iteration::Container {
            iterable,
            items: false,
            start: None,
        } = &input
            && style == JoinStyle::Plain
            && matches!(
                self.types.element(iterable.ty),
                Some(Type::Str | Type::StrLiteral(_))
            )
        {
            // Exact lists have no iteration callbacks; the runtime can borrow
            // their complete string buffer after both call arguments finish.
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::StringJoin, vec![receiver, iterable.clone()]),
            });
        }
        let separator = self.hidden(Type::Str);
        let list = self.types.list(Type::Str);
        let parts = self.hidden(list);
        let mut body = vec![
            Statement::Assign(separator, receiver),
            Statement::Assign(
                parts,
                Expr {
                    ty: list,
                    kind: ExprKind::List(Vec::new()),
                },
            ),
        ];
        body.extend(self.expand_items(input, call, |checker, value| {
            let value = match style {
                JoinStyle::Plain => checker.coerce(value, Type::Str, call)?,
                JoinStyle::Shell => checker.shlex_quote_value(value, call)?,
                JoinStyle::WindowsCommand => {
                    let path = Self::string_value(checker.fspath(value, source)?);
                    Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(
                            Builtin::Process(ir::ProcessOperation::QuoteCommandArgument),
                            vec![path],
                        ),
                    }
                }
            };
            Ok(vec![Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::Append, vec![checker.local(parts), value]),
            })])
        })?);
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Scope {
                body,
                value: Box::new(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(
                        Builtin::StringJoin,
                        vec![self.local(separator), self.local(parts)],
                    ),
                }),
                clear: vec![separator, parts],
            },
        })
    }

    /// The receiver and keyword values are evaluated once in call order, even
    /// when the template skips or repeats a name. Only exact scalar text enters
    /// the formatter, so looking up a field never invokes Python code.
    pub(super) fn named_format_call(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if !call.arguments.args.is_empty() {
            return Err(error(
                self.source,
                call,
                "str.format requires named arguments",
            ));
        }
        let mut args = vec![receiver];
        let mut names = HashSet::new();
        for keyword in &call.arguments.keywords {
            let name = keyword.arg.as_ref().expect("unpacking was rejected");
            if !names.insert(name.as_str()) {
                return Err(error(self.source, keyword, "repeated str.format keyword"));
            }
            let value = Self::string_value(self.expr(&keyword.value)?);
            if !matches!(value.ty, Type::Str | Type::Int) {
                return Err(error(
                    self.source,
                    keyword,
                    "str.format fields require str or int",
                ));
            }
            args.push(Expr {
                ty: Type::Str,
                kind: ExprKind::Str(name.to_string()),
            });
            args.push(value);
        }
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::StringFormat, args),
        })
    }

    pub(super) fn replace_call(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [old, new] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "str.replace requires old and new strings",
            ));
        };
        let old = self.expr_expected(old, Type::Str)?;
        let new = self.expr_expected(new, Type::Str)?;
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::StringReplace, vec![receiver, old, new]),
        })
    }

    pub(super) fn abs_call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [value] = call.arguments.args.as_ref() else {
            return Err(error(self.source, call, "abs requires one int or float"));
        };
        let mut value = self.expr(value)?;
        if value.ty == Type::Bool {
            value = Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Int, vec![value]),
            };
        }
        if !matches!(value.ty, Type::Int | Type::Float) {
            return Err(error(self.source, call, "abs requires one int or float"));
        }
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Builtin(Builtin::Abs, vec![value]),
        })
    }

    /// Decimal integer fields keep their full precision. Fixed-point and
    /// percentage fields use Python's checked integer-to-float conversion.
    pub(super) fn formatted_scalar(
        &mut self,
        field: &ast::InterpolatedElement,
    ) -> Result<Expr, String> {
        if field.debug_text.is_some() {
            return Err(error(
                self.source,
                field,
                "debug f-string fields are unsupported",
            ));
        }
        let mut specification = String::new();
        if let Some(spec) = &field.format_spec {
            for part in &spec.elements {
                let ast::InterpolatedStringElement::Literal(literal) = part else {
                    return Err(error(
                        self.source,
                        field,
                        "f-string format specifications must be literal",
                    ));
                };
                specification.push_str(&literal.value);
            }
        }
        if field.conversion == ast::ConversionFlag::Ascii {
            if !specification.is_empty() {
                return Err(error(
                    self.source,
                    field,
                    "formatted strings support !a with an empty format specification",
                ));
            }
            return self.ascii_expr(&field.expression);
        }
        if specification.is_empty()
            && let Some(text) = self.unittest_failure_repr(&field.expression)?
        {
            return Ok(text);
        }
        let mut value = self.expr(&field.expression)?;
        if field.conversion != ast::ConversionFlag::Repr {
            value = if matches!(value.ty, Type::Enum(_)) {
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::EnumString, vec![value]),
                }
            } else {
                Self::enum_text(value)
            };
        }
        if !specification.is_empty() {
            if let Some(digits) = specification.strip_suffix(['x', 'X'])
                && digits.bytes().all(|b| b.is_ascii_digit())
                && let Some(width) = if digits.is_empty() {
                    Some(0)
                } else {
                    digits.parse::<u32>().ok()
                }
                && width <= 1000
            {
                if field.conversion != ast::ConversionFlag::None
                    || !matches!(value.ty, Type::Int | Type::Bool)
                {
                    return Err(error(
                        self.source,
                        field,
                        "hexadecimal formats require an unconverted int or bool",
                    ));
                }
                if value.ty == Type::Bool {
                    value = Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                    };
                }
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(
                        Builtin::HexFormat {
                            width,
                            zero: digits.starts_with('0'),
                            upper: specification.ends_with('X'),
                        },
                        vec![value],
                    ),
                });
            }
            let sign = specification.strip_suffix('d').unwrap_or(&specification);
            if matches!(sign, "" | "-" | "+" | " ") {
                if field.conversion != ast::ConversionFlag::None
                    || !matches!(value.ty, Type::Int | Type::Bool)
                {
                    return Err(error(
                        self.source,
                        field,
                        "decimal integer formats require an unconverted int or bool",
                    ));
                }
                return Ok(self.formatted_integer(value, sign));
            }
            let (positive_sign, unsigned) = match specification.as_bytes().first() {
                Some(&sign @ (b'+' | b' ')) => (sign, &specification[1..]),
                Some(b'-') => (0, &specification[1..]),
                _ => (0, specification.as_str()),
            };
            let (precision, percent) = numeric_format(unsigned).ok_or_else(|| {
                error(
                    self.source,
                    field,
                    "numeric formats support signed decimal integers or f and % with literal precision from 0 through 1000",
                )
            })?;
            if field.conversion != ast::ConversionFlag::None
                || !matches!(value.ty, Type::Int | Type::Float | Type::Bool)
            {
                return Err(error(
                    self.source,
                    field,
                    "numeric format specifications require an unconverted int, float, or bool",
                ));
            }
            if value.ty != Type::Float {
                value = Expr {
                    ty: Type::Float,
                    kind: ExprKind::Builtin(Builtin::Float, vec![value]),
                };
            }
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(
                    Builtin::FloatFormat {
                        precision,
                        percent,
                        positive_sign,
                    },
                    vec![value],
                ),
            });
        }
        if matches!(
            value.ty,
            Type::Exception | Type::ImportException | Type::NamedException(_)
        ) {
            let op = if field.conversion == ast::ConversionFlag::Repr {
                Builtin::ExceptionRepr
            } else {
                Builtin::ExceptionMessage
            };
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(op, vec![value]),
            });
        }
        if matches!(value.ty, Type::Calendar(_)) {
            let name = if field.conversion == ast::ConversionFlag::Repr {
                "repr"
            } else {
                "str"
            };
            return Ok(calendar::format_builtin(name, vec![value]));
        }
        if field.conversion != ast::ConversionFlag::Repr
            && matches!(value.ty, Type::Class(_) | Type::CompletedProcess(..))
        {
            return self.record_text(value, field);
        }
        if field.conversion == ast::ConversionFlag::Repr && matches!(value.ty, Type::Class(_)) {
            return self.record_repr(value, field);
        }
        if field.conversion == ast::ConversionFlag::Repr && value.ty == Type::ObjectValue {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::ExceptionFieldRepr, vec![value]),
            });
        }
        if field.conversion == ast::ConversionFlag::Repr && matches!(value.ty, Type::Document(_)) {
            return Ok(documents::op(ir::DocumentOp::Repr, Type::Str, vec![value]));
        }
        if field.conversion == ast::ConversionFlag::Repr
            || self.types.element(value.ty).is_some()
            || matches!(value.ty, Type::Set(_))
        {
            if !dataclass_values::can_format(value.ty, self.types, self.classes) {
                return Err(error(
                    self.source,
                    field,
                    "repr does not support resource or callable values",
                ));
            }
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Repr, vec![value]),
            });
        }
        if value.ty.is_path() {
            return Ok(paths::operation(
                ir::PathOperation::Text,
                Type::Str,
                vec![value],
            ));
        }
        if self.types.payload(value.ty) == Some(Type::Str) {
            let slot = self.hidden(value.ty);
            let local = self.local(slot);
            let absent = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::IsNone, vec![local.clone()]),
            };
            let present = Expr {
                ty: Type::Str,
                kind: ExprKind::Unwrap(Box::new(local)),
            };
            let none = Expr {
                ty: Type::Str,
                kind: ExprKind::Str("None".into()),
            };
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    value: Box::new(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Conditional(
                            Box::new(absent),
                            Box::new(none),
                            Box::new(present),
                        ),
                    }),
                    clear: vec![slot],
                },
            });
        }
        if !matches!(
            value.ty,
            Type::Int
                | Type::Float
                | Type::Bool
                | Type::Str
                | Type::Bytes
                | Type::None
                | Type::Exception
                | Type::ObjectValue
                | Type::ImportException
                | Type::TimeoutExpired
        ) {
            return Err(error(
                self.source,
                field,
                "unsupported value type for default f-string conversion",
            ));
        }
        Ok(if value.ty == Type::Str {
            value
        } else {
            Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Str, vec![value]),
            }
        })
    }

    fn formatted_integer(&mut self, mut value: Expr, sign: &str) -> Expr {
        if value.ty == Type::Bool {
            value = Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Int, vec![value]),
            };
        }
        let digits = |value| Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::Str, vec![value]),
        };
        if matches!(sign, "" | "-") {
            return digits(value);
        }
        // Evaluate the field once even when deciding whether to add a sign.
        let slot = self.hidden(Type::Int);
        let local = self.local(slot);
        let nonnegative =
            pipelines::compare(Compare::GreaterEqual, local.clone(), pipelines::integer(0));
        let digits = digits(local);
        let prefixed = unpacking::concat(unpacking::string(sign), digits.clone());
        Expr {
            ty: Type::Str,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, value)],
                value: Box::new(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Conditional(
                        Box::new(nonnegative),
                        Box::new(prefixed),
                        Box::new(digits),
                    ),
                }),
                clear: vec![slot],
            },
        }
    }
}

fn numeric_format(specification: &str) -> Option<(u32, bool)> {
    let (prefix, percent) = if let Some(prefix) = specification.strip_suffix('f') {
        (prefix, false)
    } else {
        (specification.strip_suffix('%')?, true)
    };
    let precision = if prefix.is_empty() {
        6
    } else {
        let digits = prefix.strip_prefix('.')?;
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        digits.parse::<u32>().ok()?
    };
    (precision <= 1000).then_some((precision, percent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_formats_are_literal_and_bounded() {
        for (text, expected) in [
            ("f", (6, false)),
            ("%", (6, true)),
            (".0f", (0, false)),
            (".2%", (2, true)),
            (".1000f", (1000, false)),
        ] {
            assert_eq!(numeric_format(text), Some(expected));
        }
        for text in [
            ".1001f",
            ".f",
            ".-1f",
            ".٢f",
            "2f",
            ".2g",
            ".999999999999999999999f",
        ] {
            assert_eq!(numeric_format(text), None);
        }
    }
}
