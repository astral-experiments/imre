//! Function tools, including left folds and the public decorator defaults.

use super::*;

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
                    let name = match checker.locals[callback_slot] {
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
                        _ => {
                            return Err(error(
                                checker.source,
                                function,
                                "unsupported reduce callback type",
                            ));
                        }
                    };
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
