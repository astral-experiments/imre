//! Keep construction, argument evaluation and iterator acquisition in Python order.

use super::*;

/// Allocate a ring, or clear the receiver and replace its bound before taking new items.
fn initialize(ty: Type, receiver: Option<Expr>, bound: Expr) -> Expr {
    match receiver {
        Some(receiver) => Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(Builtin::DequeReset, vec![receiver, bound]),
        },
        None => Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::DequeNew, vec![bound]),
        },
    }
}

impl Checker<'_> {
    /// A saved method owns the receiver that was evaluated at attribute access.
    /// Its adapter has the same typed arguments as the corresponding direct call.
    pub(super) fn deque_attribute(
        &mut self,
        attribute: &ast::ExprAttribute,
        receiver: Expr,
        element: Type,
    ) -> Result<Expr, String> {
        let name = attribute.attr.as_str();
        let (builtin, parameters, returns) = match name {
            "maxlen" => {
                return Ok(Expr {
                    ty: self.types.optional(Type::Int),
                    kind: ExprKind::Builtin(Builtin::DequeMaxlen, vec![receiver]),
                });
            }
            "append" | "appendleft" => (
                Builtin::DequeAppend {
                    left: name == "appendleft",
                    extending: false,
                },
                vec![element],
                Type::None,
            ),
            "pop" | "popleft" => (
                Builtin::DequePop {
                    left: name == "popleft",
                },
                vec![],
                element,
            ),
            "clear" => (Builtin::DequeClear, vec![], Type::None),
            "reverse" => (Builtin::DequeReverse, vec![], Type::None),
            "__getitem__" => (Builtin::DequeGet { method: true }, vec![Type::Int], element),
            "__setitem__" => (Builtin::DequeSet, vec![Type::Int, element], Type::None),
            "__delitem__" => (
                Builtin::DequeDelete { method: true },
                vec![Type::Int],
                Type::None,
            ),
            "copy" | "__copy__" => (Builtin::DequeCopy, vec![], receiver.ty),
            _ => return Err(error(self.source, attribute, "unsupported deque attribute")),
        };
        Ok(self.bound_builtin(attribute, receiver, builtin, parameters, returns))
    }

    pub(super) fn deque_new(
        &mut self,
        call: &ast::ExprCall,
        hint: Option<Type>,
    ) -> Result<Expr, String> {
        self.deque_initialize(call, hint, None)
    }

    fn deque_initialize(
        &mut self,
        call: &ast::ExprCall,
        hint: Option<Type>,
        receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let result = if receiver.is_some() {
            Type::None
        } else {
            self.types.deque(hint.unwrap_or(Type::Int))
        };
        let count = call.arguments.args.len() + call.arguments.keywords.len();
        if count > 2 {
            return self.deque_binding_error(
                call,
                receiver,
                result,
                format!("deque() takes at most 2 arguments ({count} given)"),
            );
        }
        let mut source = call.arguments.args.first();
        let mut limit = call.arguments.args.get(1);
        for keyword in &call.arguments.keywords {
            let destination = match keyword.arg.as_deref() {
                Some("iterable") => &mut source,
                Some("maxlen") => &mut limit,
                Some(name) => {
                    return self.deque_binding_error(
                        call,
                        receiver,
                        result,
                        format!("deque() got an unexpected keyword argument '{name}'"),
                    );
                }
                None => unreachable!("keyword unpacking was rejected"),
            };
            if destination.replace(&keyword.value).is_some() {
                let name = keyword.arg.as_deref().unwrap();
                let position = if name == "iterable" { 1 } else { 2 };
                return self.deque_binding_error(
                    call,
                    receiver,
                    result,
                    format!(
                        "argument for deque() given by name ('{name}') and position ({position})"
                    ),
                );
            }
        }
        let optional = self.types.optional(Type::Int);
        let bound = if let Some(limit) = limit {
            let mut value = self.evaluated_argument(limit)?;
            if value.ty == Type::Bool {
                value = Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                };
            }
            if !matches!(value.ty, Type::None | Type::Int) && value.ty != optional {
                if !matches!(
                    value.ty,
                    Type::Float
                        | Type::Str
                        | Type::StrLiteral(_)
                        | Type::Bytes
                        | Type::Tuple(_)
                        | Type::ListInt
                        | Type::List(_)
                        | Type::Set(_)
                        | Type::Dict(_)
                        | Type::Deque(_)
                        | Type::PlainObject
                ) {
                    return Err(error(
                        self.source,
                        limit,
                        "deque maxlen requires a known integer or None",
                    ));
                }
                return self.deque_binding_error(
                    call,
                    receiver,
                    result,
                    "an integer is required".to_owned(),
                );
            }
            self.coerce(value, optional, limit)?
        } else {
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            }
        };
        let Some(source) = source else {
            return Ok(initialize(
                self.types.deque(hint.unwrap_or(Type::Int)),
                receiver,
                bound,
            ));
        };
        let mut input = self.expansion_input(
            source,
            hint.or_else(|| {
                matches!(source, ast::Expr::List(list) if list.elts.is_empty()).then_some(Type::Int)
            }),
        )?;
        let limit_first = limit.is_some_and(|limit| limit.start() < source.start());
        if let Iteration::Container { iterable, .. } = &mut input {
            if matches!(
                iterable.ty,
                Type::None | Type::Bool | Type::Int | Type::Float | Type::PlainObject
            ) {
                let name = match iterable.ty {
                    Type::None => "NoneType".to_owned(),
                    Type::PlainObject => "object".to_owned(),
                    ty => self.type_name(ty),
                };
                let mut scope = calls::ArgumentScope::default();
                let receiver = receiver.map(|r| scope.hold(self, r));
                let bound = if limit_first {
                    let bound = scope.hold(self, bound);
                    scope.hold(self, iterable.clone());
                    bound
                } else {
                    scope.hold(self, iterable.clone());
                    scope.hold(self, bound)
                };
                let init = initialize(self.types.deque(hint.unwrap_or(Type::Int)), receiver, bound);
                return Ok(scope.finish(calls::type_error(
                    result,
                    vec![init],
                    format!("'{name}' object is not iterable"),
                )));
            }
            if matches!(iterable.ty, Type::StrLiteral(_)) {
                *iterable = Self::string_value(iterable.clone());
            }
        }
        let mut input = self.prepare_iteration(input, source, false)?;
        self.hint_empty_iteration(&input, Some(hint.unwrap_or(Type::Int)));
        let element = hint.unwrap_or(self.locals[input.value]);
        let ty = self.types.deque(element);
        let target = self.hidden(ty);
        let limit_slot = self.hidden(bound.ty);
        let bound = Statement::Assign(limit_slot, bound);
        if limit_first {
            input.evaluate.insert(0, bound);
        } else {
            input.evaluate.push(bound);
        }
        let bound = self.local(limit_slot);
        let result = if let Some(receiver) = receiver {
            input
                .evaluate
                .insert(0, Statement::Assign(target, receiver));
            input.evaluate.push(Statement::Expr(initialize(
                ty,
                Some(self.local(target)),
                bound,
            )));
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            }
        } else {
            input
                .evaluate
                .push(Statement::Assign(target, initialize(ty, None, bound)));
            self.local(target)
        };
        let body = self.deque_fill(target, element, Iteration::Prepared(input), false, call)?;
        Ok(Expr {
            ty: result.ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(result),
                clear: vec![target, limit_slot],
            },
        })
    }

    /// A rejection still evaluates the receiver and every argument before raising.
    fn deque_binding_error(
        &mut self,
        call: &ast::ExprCall,
        receiver: Option<Expr>,
        result: Type,
        message: String,
    ) -> Result<Expr, String> {
        self.graph.checked_overload(self.module, call.range());
        self.graph.checked_keywords(
            self.module,
            call.arguments.keywords.iter().map(Ranged::range),
        );
        for arg in call.arguments.args.iter().skip(2) {
            self.graph.checked_excess_argument(self.module, arg.range());
        }
        let mut values: Vec<_> = receiver.into_iter().collect();
        for arg in call
            .arguments
            .args
            .iter()
            .chain(call.arguments.keywords.iter().map(|k| &k.value))
        {
            values.push(self.evaluated_argument(arg)?);
        }
        Ok(calls::type_error(result, values, message))
    }

    fn deque_fill(
        &mut self,
        target: usize,
        element: Type,
        input: Iteration,
        left: bool,
        node: &impl Ranged,
    ) -> Result<Vec<Statement>, String> {
        self.expand_items(input, node, |checker, item| {
            let item = checker.coerce(item, element, node)?;
            Ok(vec![Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(
                    Builtin::DequeAppend {
                        left,
                        extending: true,
                    },
                    vec![checker.local(target), item],
                ),
            })])
        })
    }

    pub(super) fn deque_method(
        &mut self,
        call: &ast::ExprCall,
        attribute: &ast::ExprAttribute,
        receiver: Expr,
        element: Type,
    ) -> Result<Expr, String> {
        let name = attribute.attr.as_str();
        if name == "__init__" {
            return self.deque_initialize(call, Some(element), Some(receiver));
        }
        if name == "reverse" {
            return self.sequence_reverse(stdlib::ReverseSequence::Deque, Some(receiver), call);
        }
        self.reject_builtin_keywords(call)?;
        let mut args = vec![receiver];
        let (builtin, ty) = match (name, call.arguments.args.as_ref()) {
            ("append" | "appendleft", [value]) => {
                args.push(self.expr_expected(value, element)?);
                (
                    Builtin::DequeAppend {
                        left: name == "appendleft",
                        extending: false,
                    },
                    Type::None,
                )
            }
            ("pop" | "popleft", []) => (
                Builtin::DequePop {
                    left: name == "popleft",
                },
                element,
            ),
            ("clear", []) => (Builtin::DequeClear, Type::None),
            ("__getitem__" | "__delitem__", [index]) => {
                args.push(self.integer_index(index)?);
                if name == "__getitem__" {
                    (Builtin::DequeGet { method: true }, element)
                } else {
                    (Builtin::DequeDelete { method: true }, Type::None)
                }
            }
            ("__setitem__", [index, value]) => {
                args.push(self.integer_index(index)?);
                args.push(self.expr_expected(value, element)?);
                (Builtin::DequeSet, Type::None)
            }
            ("copy" | "__copy__", []) => (Builtin::DequeCopy, args[0].ty),
            ("extend" | "extendleft", [source]) => {
                let receiver = args.pop().unwrap();
                let target = self.hidden(receiver.ty);
                let mut input = self.expansion_input(source, Some(element))?;
                if let Iteration::Container {
                    iterable,
                    items: false,
                    start: None,
                } = &mut input
                {
                    if iterable.ty == receiver.ty {
                        *iterable = Expr {
                            ty: receiver.ty,
                            kind: ExprKind::Builtin(
                                Builtin::DequeExtendSource,
                                vec![self.local(target), iterable.clone()],
                            ),
                        };
                    } else if matches!(iterable.ty, Type::StrLiteral(_)) {
                        *iterable = Self::string_value(iterable.clone());
                    }
                }
                let mut body = vec![Statement::Assign(target, receiver)];
                body.extend(self.deque_fill(target, element, input, name == "extendleft", call)?);
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::Scope {
                        body,
                        value: Box::new(Expr {
                            ty: Type::None,
                            kind: ExprKind::None,
                        }),
                        clear: vec![target],
                    },
                });
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "unsupported deque method or arguments",
                ));
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(builtin, args),
        })
    }
}
