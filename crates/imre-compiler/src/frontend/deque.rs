//! Keep construction, argument evaluation and iterator acquisition in Python order.

use super::*;

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
            "copy" | "__copy__" => (Builtin::DequeCopy, vec![], receiver.ty),
            _ => return Err(error(self.source, attribute, "unsupported deque attribute")),
        };
        let mut locals = vec![receiver.ty];
        locals.extend(&parameters);
        let arguments = locals
            .iter()
            .enumerate()
            .map(|(slot, &ty)| Expr {
                ty,
                kind: ExprKind::Local(slot),
            })
            .collect();
        let id = self.helpers.reserve();
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                function: ir::Function {
                    params: (0..locals.len()).collect(),
                    returns,
                    locals,
                    body: vec![Statement::Return(Expr {
                        ty: returns,
                        kind: ExprKind::Builtin(builtin, arguments),
                    })],
                },
                calls: HashSet::new(),
                class_reads: HashSet::new(),
                source_frame: false,
                recursion_error: error(self.source, attribute, "recursive deque method adapter"),
            },
        );
        Ok(Expr {
            ty: self.types.callable(parameters, returns),
            kind: ExprKind::Callable {
                function: id,
                receiver: Some(Box::new(receiver)),
            },
        })
    }

    pub(super) fn deque_new(
        &mut self,
        call: &ast::ExprCall,
        hint: Option<Type>,
    ) -> Result<Expr, String> {
        if call.arguments.args.len() > 2 {
            return Err(error(
                self.source,
                call,
                "deque takes at most two arguments",
            ));
        }
        let mut source = call.arguments.args.first();
        let mut limit = call.arguments.args.get(1);
        for keyword in &call.arguments.keywords {
            let destination = match keyword.arg.as_deref() {
                Some("iterable") => &mut source,
                Some("maxlen") => &mut limit,
                _ => return Err(error(self.source, keyword, "expected iterable or maxlen")),
            };
            if destination.replace(&keyword.value).is_some() {
                return Err(error(self.source, keyword, "duplicate deque argument"));
            }
        }
        let optional = self.types.optional(Type::Int);
        let bound = if let Some(limit) = limit {
            self.expr_expected(limit, optional)?
        } else {
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            }
        };
        let Some(source) = source else {
            return Ok(Expr {
                ty: self.types.deque(hint.unwrap_or(Type::Int)),
                kind: ExprKind::Builtin(Builtin::DequeNew, vec![bound]),
            });
        };
        let mut input = self.expansion_input(
            source,
            hint.or_else(|| {
                matches!(source, ast::Expr::List(list) if list.elts.is_empty()).then_some(Type::Int)
            }),
        )?;
        if let Iteration::Container { iterable, .. } = &mut input
            && matches!(iterable.ty, Type::StrLiteral(_))
        {
            *iterable = Self::string_value(iterable.clone());
        }
        let mut input = self.prepare_iteration(input, source, false)?;
        self.hint_empty_iteration(&input, Some(hint.unwrap_or(Type::Int)));
        let element = hint.unwrap_or(self.locals[input.value]);
        let ty = self.types.deque(element);
        let target = self.hidden(ty);
        let limit_slot = self.hidden(bound.ty);
        let bound = Statement::Assign(limit_slot, bound);
        if limit.is_some_and(|limit| limit.start() < source.start()) {
            input.evaluate.insert(0, bound);
        } else {
            input.evaluate.push(bound);
        }
        input.evaluate.push(Statement::Assign(
            target,
            Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::DequeNew, vec![self.local(limit_slot)]),
            },
        ));
        let body = self.deque_fill(target, element, Iteration::Prepared(input), false, call)?;
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(target)),
                clear: vec![target, limit_slot],
            },
        })
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
        self.reject_builtin_keywords(call)?;
        let name = attribute.attr.as_str();
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
