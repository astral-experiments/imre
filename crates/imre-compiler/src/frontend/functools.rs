//! Function tools, including left folds and the public decorator defaults.

use super::*;

/// Python's runtime type name for exact values that cannot implement a call.
pub(super) fn noncallable_type(ty: Type) -> Option<&'static str> {
    Some(match ty {
        Type::None => "NoneType",
        Type::Bool => "bool",
        Type::Int => "int",
        Type::Float => "float",
        Type::Str | Type::StrLiteral(_) => "str",
        Type::Bytes => "bytes",
        Type::ListInt | Type::List(_) => "list",
        Type::Tuple(_) => "tuple",
        Type::Dict(_) => "dict",
        Type::Set(_) => "set",
        _ => return None,
    })
}

pub(super) fn wrapper_attributes(fields: ir::WrapperAttributes, types: &Types) -> Expr {
    Expr {
        ty: types.tuple(vec![Type::Str; fields.names().len()]),
        kind: ExprKind::Builtin(Builtin::WrapperAttributes(fields), vec![]),
    }
}

fn type_error(message: &str) -> Statement {
    Statement::Raise(Some(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("TypeError").unwrap(),
            message: Some(Box::new(unpacking::string(message))),
        },
    }))
}

impl Checker<'_> {
    /// Check each argument in source order and hold it across the later calls.
    fn wrapper_arguments<const N: usize>(
        &mut self,
        call: &ast::ExprCall,
        names: [&str; N],
    ) -> Result<(calls::ArgumentScope, [Expr; N]), String> {
        let mut supplied = call.arguments.args.iter().enumerate().collect::<Vec<_>>();
        for keyword in &call.arguments.keywords {
            let index = names
                .iter()
                .position(|name| keyword.arg.as_deref() == Some(*name))
                .ok_or_else(|| error(self.source, keyword, "unsupported wrapper keyword"))?;
            supplied.push((index, &keyword.value));
        }
        let mut fields: [Option<Expr>; N] = std::array::from_fn(|_| None);
        let mut scope = calls::ArgumentScope::default();
        for (index, node) in supplied {
            let slot = fields
                .get_mut(index)
                .filter(|slot| slot.is_none())
                .ok_or_else(|| error(self.source, node, "invalid wrapper argument binding"))?;
            let value = self.expr(node)?;
            if index >= N - 2 && value.ty != self.types.tuple(Vec::new()) {
                return Err(error(
                    self.source,
                    node,
                    "native wrapper updates require empty assigned and updated tuples",
                ));
            }
            *slot = Some(scope.hold(self, value));
        }
        if fields.iter().any(Option::is_none) {
            return Err(error(
                self.source,
                call,
                "native wrapper updates require explicit assigned and updated tuples",
            ));
        }
        Ok((scope, fields.map(Option::unwrap)))
    }

    pub(super) fn attach_wrapped(
        &self,
        wrapper: Expr,
        wrapped: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if !matches!(wrapper.ty, Type::Callable(_)) || wrapper.ty != wrapped.ty {
            return Err(error(
                self.source,
                node,
                "native wrapper links require two functions with the same callable signature",
            ));
        }
        Ok(Expr {
            ty: wrapper.ty,
            kind: ExprKind::Builtin(Builtin::FunctionWrappedSet, vec![wrapper, wrapped]),
        })
    }

    pub(super) fn update_wrapper(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let (scope, [wrapper, wrapped, _, _]) =
            self.wrapper_arguments(call, ["wrapper", "wrapped", "assigned", "updated"])?;
        Ok(scope.finish(self.attach_wrapped(wrapper, wrapped, call)?))
    }

    pub(super) fn wraps_config(
        &mut self,
        config: &ast::ExprCall,
    ) -> Result<(calls::ArgumentScope, Expr), String> {
        let (scope, [wrapped, _, _]) =
            self.wrapper_arguments(config, ["wrapped", "assigned", "updated"])?;
        Ok((scope, wrapped))
    }

    /// An immediate decorator application can consume the partial without exposing it.
    pub(super) fn apply_wraps(
        &mut self,
        config: &ast::ExprCall,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let (scope, wrapped) = self.wraps_config(config)?;
        let node = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([node], []) => node,
            ([], [key]) if key.arg.as_deref() == Some("wrapper") => &key.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "an immediate wraps decorator needs a single wrapper",
                ));
            }
        };
        let wrapper = self.expr(node)?;
        let result = self.attach_wrapped(wrapper, wrapped, call)?;
        if let [keyword] = call.arguments.keywords.as_ref() {
            // The stub calls this parameter `f`; the actual partial calls
            // update_wrapper, whose first parameter is named `wrapper`.
            self.graph
                .checked_missing_argument(self.module, call.range());
            self.graph
                .checked_keywords(self.module, std::iter::once(keyword.range()));
        }
        Ok(scope.finish(result))
    }

    /// Evaluate the callable, iterable and optional seed before acquiring the iterator.
    /// Without a seed, the first item becomes the accumulator without a callback.
    pub(super) fn reduce_call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let args = &call.arguments;
        let [function, source, rest @ ..] = args.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "reduce requires a function and an iterable",
            ));
        };
        let initial = match (rest, args.keywords.as_ref()) {
            ([], []) => None,
            ([initial], []) => Some(initial),
            ([], [kw]) if kw.arg.as_deref() == Some("initial") => Some(&kw.value),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "reduce accepts one optional initial value",
                ));
            }
        };

        // Only a fixed function name or a noncapturing lambda can defer type
        // inference until its arguments are known. Other expressions can rebind
        // names used by the iterable or initial value.
        let definition = match self.resolve(function) {
            Some(Binding::Definition(id)) => self.functions.get(&id).cloned(),
            _ => None,
        };
        let checked = if definition.is_some() || matches!(function, ast::Expr::Lambda(_)) {
            None
        } else if let Some(binding) = self.local_function_binding(function) {
            Some(self.local_callback(binding, function)?)
        } else {
            Some(self.expr(function)?)
        };
        let parameter_hint = |i: usize| {
            checked
                .as_ref()
                .and_then(|f| self.types.signature(f.ty))
                .and_then(|(params, _)| params.get(i).copied())
                .or_else(|| {
                    definition.as_ref().and_then(|s| {
                        let p = s.params.get(i)?;
                        self.inference
                            .map_or(Some(p.ty), |inf| inf.parameter(s.id, i))
                    })
                })
        };
        let (acc_hint, item_hint) = (parameter_hint(0), parameter_hint(1));
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let input = self.expansion_input(source, item_hint)?;
        let mut input = self.prepare_iteration(input, source, false)?;
        self.hint_empty_iteration(&input, item_hint.or(acc_hint));
        let item = self.locals[input.value];
        body.append(&mut input.evaluate);
        clear.extend(&input.clear);
        let initial = initial
            .map(|initial| self.boolean_operand(initial, acc_hint.or(Some(item))))
            .transpose()?;
        let accumulator = initial.as_ref().map_or(item, |v| v.ty);
        let callback = if let Some(checked) = checked {
            checked
        } else if let ast::Expr::Lambda(lambda) = function {
            self.lambda_value(lambda, Some(vec![accumulator, item]), None)?
        } else {
            let signature = definition.unwrap();
            let expected = self.types.callable(vec![accumulator, item], accumulator);
            self.callable_value(&signature, None, Some(expected), function)?
        };
        let callback_slot = self.hidden(callback.ty);
        body.insert(0, Statement::Assign(callback_slot, callback));
        let result = self.hidden(accumulator);
        clear.extend([callback_slot, result]);
        let missing = if let Some(initial) = initial {
            body.push(Statement::Assign(result, initial));
            None
        } else {
            let slot = self.hidden(Type::Bool);
            body.push(Statement::Assign(
                slot,
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(true),
                },
            ));
            clear.push(slot);
            Some(slot)
        };
        body.extend(
            self.expand_items(Iteration::Prepared(input), call, |checker, item| {
                let step = if let Some((params, returns)) =
                    checker.types.signature(checker.locals[callback_slot])
                {
                    let [left, right] = params.as_slice() else {
                        return Err(error(
                            checker.source,
                            function,
                            "reduce requires a binary callable",
                        ));
                    };
                    let args = ir::CallArgs::positional(vec![
                        checker.coerce(checker.local(result), *left, function)?,
                        checker.coerce(item.clone(), *right, function)?,
                    ]);
                    let value = Expr {
                        ty: returns,
                        kind: ExprKind::Invoke {
                            callable: Box::new(checker.local(callback_slot)),
                            args,
                        },
                    };
                    Statement::Assign(result, checker.coerce(value, accumulator, function)?)
                } else {
                    // Exact noncallable values are valid until a step actually calls one.
                    let name =
                        noncallable_type(checker.locals[callback_slot]).ok_or_else(|| {
                            error(checker.source, function, "unsupported reduce callback type")
                        })?;
                    type_error(&format!("'{name}' object is not callable"))
                };
                Ok(vec![if let Some(slot) = missing {
                    Statement::If(
                        checker.local(slot),
                        vec![
                            Statement::Assign(result, item),
                            Statement::Assign(
                                slot,
                                Expr {
                                    ty: Type::Bool,
                                    kind: ExprKind::Bool(false),
                                },
                            ),
                        ],
                        vec![step],
                    )
                } else {
                    step
                }])
            })?,
        );
        if let Some(slot) = missing {
            body.push(Statement::If(
                self.local(slot),
                vec![type_error(
                    "reduce() of empty iterable with no initial value",
                )],
                vec![],
            ));
        }
        Ok(Expr {
            ty: accumulator,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear,
            },
        })
    }
}
