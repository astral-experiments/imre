//! Eager typed library operations reuse the existing iterator and callback IR.

use super::*;

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
        let mut keywords = HashSet::new();
        for keyword in &call.arguments.keywords {
            let name = keyword.arg.as_ref().map(|name| name.as_str());
            if !matches!(name, Some("key" | "reverse")) || !keywords.insert(name) {
                return Err(error(
                    self.source,
                    keyword,
                    "sorted accepts key and reverse once each",
                ));
            }
        }
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let input = self.expansion_input(&call.arguments.args[0], element)?;
        let input = self.capture_consumer_input(input, &mut body, &mut clear);
        let mut input = self.prepare_iteration(input, call, false)?;
        self.hint_empty_iteration(&input, Some(element.unwrap_or(Type::Int)));
        let element = self.locals[input.value];
        body.append(&mut input.evaluate);
        clear.extend(&input.clear);

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
        Ok(Expr {
            ty: result_type,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear,
            },
        })
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
        if value.ty != Type::None && self.types.signature(value.ty).is_none() {
            return Err(error(
                self.source,
                expression,
                "sorted key requires an exact Callable or None",
            ));
        }
        Ok(value)
    }

    fn sortable_key(&self, ty: Type, node: &impl Ranged) -> Result<(), String> {
        if matches!(ty, Type::Int | Type::Bool | Type::Str | Type::Bytes) {
            Ok(())
        } else {
            Err(error(
                self.source,
                node,
                "sorted keys require exactly int, bool, str, or bytes",
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
        let input = self.expansion_input(source, Some(Type::Str))?;
        if let Iteration::Container {
            iterable,
            items: false,
            start: None,
        } = &input
            && self.types.element(iterable.ty) == Some(Type::Str)
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
            checker.expect(Type::Str, value.ty, call)?;
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

    /// Accepts literal fixed-point/percentage precision, retaining Python's
    /// checked integer-to-float conversion before the formatting operation.
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
        if !matches!(
            field.conversion,
            ast::ConversionFlag::None | ast::ConversionFlag::Str | ast::ConversionFlag::Repr
        ) {
            return Err(error(
                self.source,
                field,
                "f-string conversions support only !s and !r",
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
        let mut value = self.expr(&field.expression)?;
        if !specification.is_empty() {
            let (precision, percent) = numeric_format(&specification).ok_or_else(|| {
                error(
                    self.source,
                    field,
                    "numeric formats support f or % with literal precision from 0 through 1000",
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
                kind: ExprKind::Builtin(Builtin::FloatFormat { precision, percent }, vec![value]),
            });
        }
        if field.conversion == ast::ConversionFlag::Repr || matches!(value.ty, Type::Class(_)) {
            if !dataclass_values::repr_supported(value.ty, self.types) {
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
        if value.ty == Type::Path {
            return Ok(paths::operation(
                ir::PathOperation::Text,
                Type::Str,
                vec![value],
            ));
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
        ) {
            return Err(error(
                self.source,
                field,
                "f-string fields require exactly int, float, bool, str, bytes, None, or Exception",
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
