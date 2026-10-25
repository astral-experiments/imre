//! Eager display expansion keeps every copy before the following source entry.

use super::*;

impl Checker<'_> {
    /// Capture ordinary fields and expanded elements in source order. Keeping
    /// each reference also preserves identity if a later field mutates it.
    pub(super) fn expanded_tuple(
        &mut self,
        tuple: &ast::ExprTuple,
        expected: Option<&[Type]>,
    ) -> Result<Expr, String> {
        let mut scope = calls::ArgumentScope::default();
        let mut values = Vec::new();
        for entry in &tuple.elts {
            if let ast::Expr::Starred(star) = entry {
                let width = self
                    .graph
                    .session
                    .tuple_spread_width(self.module, star.start())
                    .ok_or_else(|| error(self.source, star, "tuple spread has no fixed width"))?;
                // A literal is private until the display finishes. Its fields
                // can be captured directly, even when their native types differ.
                if let Some(items) = match &*star.value {
                    ast::Expr::Tuple(t) => Some(&t.elts),
                    ast::Expr::List(l) => Some(&l.elts),
                    _ => None,
                } {
                    for item in items {
                        let field = match expected.and_then(|fields| fields.get(values.len())) {
                            Some(&ty) => self.expr_expected(item, ty)?,
                            None => self.expr(item)?,
                        };
                        values.push(scope.hold(self, field));
                    }
                    continue;
                }
                let source = self.expr(&star.value)?;
                let source = scope.hold(self, source);
                let fields = self.types.fields(source.ty);
                let element = self.types.element(source.ty);
                if fields.as_ref().is_some_and(|f| f.len() != width)
                    || fields.is_none() && element.is_none()
                {
                    return Err(error(
                        self.source,
                        star,
                        "tuple spread requires a fixed tuple or list",
                    ));
                }
                for i in 0..width {
                    let field = if let Some(fields) = &fields {
                        Expr {
                            ty: fields[i],
                            kind: ExprKind::Field(Box::new(source.clone()), i),
                        }
                    } else {
                        Expr {
                            ty: element.unwrap(),
                            kind: ExprKind::Builtin(
                                Builtin::ListGet,
                                vec![
                                    source.clone(),
                                    Expr {
                                        ty: Type::Int,
                                        kind: ExprKind::Int(i.to_string()),
                                    },
                                ],
                            ),
                        }
                    };
                    let field = match expected.and_then(|fields| fields.get(values.len())) {
                        Some(&ty) => self.coerce(field, ty, star)?,
                        None => field,
                    };
                    values.push(scope.hold(self, field));
                }
            } else {
                let value = match expected.and_then(|fields| fields.get(values.len())) {
                    Some(&ty) => self.expr_expected(entry, ty)?,
                    None => self.expr(entry)?,
                };
                values.push(scope.hold(self, value));
            }
        }
        if expected.is_some_and(|fields| fields.len() != values.len()) {
            return Err(error(
                self.source,
                tuple,
                "tuple literal has the wrong number of fields",
            ));
        }
        Ok(scope.finish(Expr {
            ty: self.types.tuple(values.iter().map(|v| v.ty).collect()),
            kind: ExprKind::Tuple(values),
        }))
    }

    /// The right operand may mutate or rebind the left before either list is copied.
    pub(super) fn concat_lists(
        &mut self,
        left: Expr,
        right: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let ty = left.ty;
        let left_slot = self.hidden(ty);
        let right_slot = self.hidden(ty);
        let result = self.hidden(ty);
        let mut body = vec![
            Statement::Assign(left_slot, left),
            Statement::Assign(right_slot, right),
            Statement::Assign(
                result,
                Expr {
                    ty,
                    kind: ExprKind::List(Vec::new()),
                },
            ),
        ];
        let mut element = self.types.element(ty);
        for slot in [left_slot, right_slot] {
            let input = Iteration::Container {
                iterable: self.local(slot),
                items: false,
                start: None,
            };
            body.extend(self.expand_items(input, node, |checker, value| {
                checker.expansion_insert(result, &mut element, value, false, node)
            })?);
        }
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear: vec![result, right_slot, left_slot],
            },
        })
    }

    /// Roots the receiver before its argument, and snapshots list aliases so
    /// extending a list by itself consumes only the original elements.
    pub(super) fn extend_list(
        &mut self,
        receiver: Expr,
        source: &ast::Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let element = self.types.element(receiver.ty).unwrap();
        let destination = self.hidden(receiver.ty);
        let mut input = self.expansion_input(source, Some(element))?;
        if let Iteration::Container {
            iterable,
            items: false,
            start: None,
        } = &mut input
            && iterable.ty == receiver.ty
        {
            *iterable = self.list_snapshot(iterable.clone());
        }
        let mut body = vec![Statement::Assign(destination, receiver)];
        // CPython reserves space on acquiring an iterator with no length hint,
        // even when its first pull stops or raises. A reentrant sort observes it.
        let unhinted = match &input {
            Iteration::Prepared(_) => !matches!(source, ast::Expr::Call(call)
                if matches!(&*call.func, ast::Expr::Name(name) if name.id == "reversed")),
            Iteration::Container { iterable, .. } => {
                matches!(
                    iterable.ty,
                    Type::TextIO | Type::StringIO | Type::BinaryIO | Type::CsvReader
                )
            }
            Iteration::Range(_) => false,
        };
        let before = if unhinted {
            vec![Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::ListExtendUnhinted, vec![self.local(destination)]),
            })]
        } else {
            Vec::new()
        };
        body.extend(
            self.expand_items_after(input, node, before, |checker, value| {
                Ok(vec![Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(
                        Builtin::Append,
                        vec![
                            checker.local(destination),
                            checker.coerce(value, element, node)?,
                        ],
                    ),
                })])
            })?,
        );
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body,
                value: Box::new(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }),
                clear: vec![destination],
            },
        })
    }

    /// Uses destination context for newly constructed sources, without coercing
    /// an existing mutable container to a different element layout.
    fn expansion_hint(&self, expression: &ast::Expr, element: Type) -> Option<Type> {
        match expression {
            ast::Expr::List(_) | ast::Expr::ListComp(_) => Some(self.types.list(element)),
            ast::Expr::Tuple(tuple)
                if !tuple
                    .elts
                    .iter()
                    .any(|value| matches!(value, ast::Expr::Starred(_))) =>
            {
                Some(self.types.tuple(vec![element; tuple.elts.len()]))
            }
            ast::Expr::Set(_) | ast::Expr::SetComp(_) => {
                let element = self.types.payload(element).unwrap_or(element);
                self.types
                    .hashable_key(element)
                    .then(|| self.types.set(element))
            }
            ast::Expr::Dict(dict) if dict.items.is_empty() => {
                let element = self.types.payload(element).unwrap_or(element);
                self.types
                    .hashable_key(element)
                    .then(|| self.types.dict(element, Type::None))
            }
            ast::Expr::Call(call)
                if call.arguments.args.is_empty() && call.arguments.keywords.is_empty() =>
            {
                match &*call.func {
                    ast::Expr::Name(name) if name.id == "list" => Some(self.types.list(element)),
                    ast::Expr::Name(name) if name.id == "set" => {
                        let element = self.types.payload(element).unwrap_or(element);
                        self.types
                            .hashable_key(element)
                            .then(|| self.types.set(element))
                    }
                    ast::Expr::Name(name) if name.id == "dict" => {
                        let element = self.types.payload(element).unwrap_or(element);
                        self.types
                            .hashable_key(element)
                            .then(|| self.types.dict(element, Type::None))
                    }
                    _ => None,
                }
            }
            ast::Expr::Named(value) => self.expansion_hint(&value.value, element),
            ast::Expr::If(value) => self
                .expansion_hint(&value.body, element)
                .or_else(|| self.expansion_hint(&value.orelse, element)),
            ast::Expr::BoolOp(value) => value
                .values
                .iter()
                .find_map(|value| self.expansion_hint(value, element)),
            _ => None,
        }
    }

    /// Existing range/enumerate/items syntax remains confined to an eager
    /// consumer; no public iterator layout is introduced by display expansion.
    pub(super) fn expansion_input(
        &mut self,
        expression: &ast::Expr,
        element: Option<Type>,
    ) -> Result<Iteration, String> {
        if let Some(input) = self.pipeline_input(expression, element)? {
            if let Iteration::Prepared(prepared) = &input {
                self.hint_empty_iteration(prepared, element);
            }
            return Ok(input);
        }
        if let ast::Expr::Call(call) = expression {
            if self.resolve(&call.func).is_some() {
                return Ok(Iteration::Container {
                    iterable: self.expr(expression)?,
                    items: false,
                    start: None,
                });
            }
            if matches!(&*call.func, ast::Expr::Name(name) if name.id == "range") {
                return self.iteration_input(expression);
            }
            if let ast::Expr::Attribute(attribute) = &*call.func
                && attribute.attr.as_str() == "items"
                && self.class_reference(&attribute.value).is_none()
            {
                let fields =
                    element.and_then(|ty| self.types.fields(self.types.payload(ty).unwrap_or(ty)));
                let hint = fields.as_deref().and_then(|fields| match fields {
                    [key, value] if self.types.hashable_key(*key) => {
                        Some(self.types.dict(*key, *value))
                    }
                    _ => None,
                });
                let iterable = self.boolean_operand(&attribute.value, hint)?;
                if let Type::Class(class) = iterable.ty {
                    let iterable = self.iteration_method_call(call, attribute, class, iterable)?;
                    return self.iteration_value(iterable, false, call);
                }
                let iterable = if matches!(iterable.ty, Type::Document(_)) {
                    self.document_map(iterable)
                } else {
                    iterable
                };
                if self.types.mapping(iterable.ty).is_none() {
                    return Err(error(
                        self.source,
                        call,
                        "items() requires a concrete dictionary",
                    ));
                }
                if !call.arguments.args.is_empty() || !call.arguments.keywords.is_empty() {
                    return Err(error(self.source, call, "dict.items() takes no arguments"));
                }
                return Ok(Iteration::Container {
                    iterable,
                    items: true,
                    start: None,
                });
            }
        }
        let hint = element.and_then(|element| self.expansion_hint(expression, element));
        let iterable = self.iteration_expression(expression, hint)?;
        self.iteration_value(iterable, false, expression)
    }

    /// Fixed tuples copy directly from rooted fields. Other sources share the
    /// normal iteration lowering with one private target that cannot escape.
    pub(super) fn expand_items(
        &mut self,
        input: Iteration,
        node: &impl Ranged,
        consume: impl FnMut(&mut Self, Expr) -> Result<Vec<Statement>, String>,
    ) -> Result<Vec<Statement>, String> {
        self.expand_items_after(input, node, Vec::new(), consume)
    }

    /// Remaining consumer arguments run after iterator acquisition, even for
    /// an empty source. Fixed tuples hold their original fields through effects.
    pub(super) fn expand_items_after(
        &mut self,
        input: Iteration,
        node: &impl Ranged,
        before_iteration: Vec<Statement>,
        mut consume: impl FnMut(&mut Self, Expr) -> Result<Vec<Statement>, String>,
    ) -> Result<Vec<Statement>, String> {
        if let Iteration::Container {
            iterable,
            items: false,
            start: None,
        } = &input
            && let Some(fields) = self.types.fields(iterable.ty)
        {
            let captured = self.hidden(iterable.ty);
            let mut body = vec![Statement::Assign(captured, iterable.clone())];
            body.extend(before_iteration);
            for (index, ty) in fields.into_iter().enumerate() {
                body.extend(consume(
                    self,
                    Expr {
                        ty,
                        kind: ExprKind::Field(Box::new(self.local(captured)), index),
                    },
                )?);
            }
            return Ok(vec![Statement::Scope {
                body,
                clear: vec![captured],
            }]);
        }
        // The spelling is not a Python identifier, so no source binding can
        // collide with this target even when expansion is nested.
        let name = format!("@expansion{}", self.locals.len());
        let target = ast::Expr::Name(ast::ExprName {
            node_index: ast::AtomicNodeIndex::NONE,
            range: node.range(),
            id: name.clone().into(),
            ctx: ast::ExprContext::Store,
        });
        let (body, _) =
            self.lower_iteration_after(&target, input, &[], before_iteration, |checker| {
                let slot = checker.names[&name];
                let mut body = consume(checker, checker.local(slot))?;
                // Builtin consumers release each yielded item before requesting
                // the next one. Keeping this private target alive could alter
                // file finalization or make an iterator's tuple cache shared.
                body.push(Statement::Clear(slot));
                Ok(body)
            })?;
        let target = self.names.remove(&name).unwrap();
        Ok(vec![Statement::Scope {
            body,
            clear: vec![target],
        }])
    }

    /// Promotes copied values into the destination layout at each insertion.
    pub(super) fn expansion_insert(
        &mut self,
        result: usize,
        element: &mut Option<Type>,
        value: Expr,
        is_set: bool,
        node: &impl Ranged,
    ) -> Result<Vec<Statement>, String> {
        let ty = *element.get_or_insert(value.ty);
        if is_set && !self.types.hashable_key(ty) {
            return Err(error(
                self.source,
                node,
                "set elements must be int, str, bytes, Path, or immutable scalar tuples",
            ));
        }
        let value = self.coerce(value, ty, node)?;
        self.locals[result] = if is_set {
            self.types.set(ty)
        } else {
            self.types.list(ty)
        };
        Ok(vec![Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(
                if is_set {
                    Builtin::SetAdd
                } else {
                    Builtin::Append
                },
                vec![self.local(result), value],
            ),
        })])
    }

    /// A display owns its destination across all source evaluation and iterator
    /// failures, while source-level walrus bindings retain their usual lifetime.
    pub(super) fn expanded_sequence(
        &mut self,
        values: &[ast::Expr],
        mut element: Option<Type>,
        is_set: bool,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let result = self.hidden(Type::None);
        let mut body = Vec::new();
        for expression in values {
            if let ast::Expr::Starred(starred) = expression {
                let input = self.expansion_input(&starred.value, element)?;
                body.extend(self.expand_items(input, expression, |checker, value| {
                    checker.expansion_insert(result, &mut element, value, is_set, expression)
                })?);
            } else {
                let value = if let Some(element) = element {
                    self.expr_expected(expression, element)?
                } else {
                    self.expr(expression)?
                };
                body.extend(self.expansion_insert(
                    result,
                    &mut element,
                    value,
                    is_set,
                    expression,
                )?);
            }
        }
        let element = element.or((!is_set).then_some(Type::Int)).ok_or_else(|| {
            error(
                self.source,
                node,
                "an empty set requires a concrete set[T] context",
            )
        })?;
        if is_set && !self.types.hashable_key(element) {
            return Err(error(
                self.source,
                node,
                "set elements must be int, str, bytes, Path, or immutable scalar tuples",
            ));
        }
        let (ty, empty) = if is_set {
            (
                self.types.set(element),
                ExprKind::Set {
                    element,
                    values: Vec::new(),
                },
            )
        } else {
            (self.types.list(element), ExprKind::List(Vec::new()))
        };
        self.locals[result] = ty;
        body.insert(0, Statement::Assign(result, Expr { ty, kind: empty }));
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear: vec![result],
            },
        })
    }

    /// Copies mapping entries with the existing ordered iterator, without
    /// allocating an intermediate tuple for each key/value pair.
    fn expand_mapping(
        &mut self,
        result: usize,
        source: Expr,
        destination: (Type, Type),
        node: &impl Ranged,
    ) -> Result<Statement, String> {
        let (key_type, value_type) = self.types.mapping(source.ty).ok_or_else(|| {
            error(
                self.source,
                node,
                "dictionary expansion requires a concrete dictionary",
            )
        })?;
        self.expect(destination.0, key_type, node)?;
        let source_slot = self.hidden(source.ty);
        let iterator = self.hidden(Type::Iterator);
        let key = self.hidden(key_type);
        let value = self.hidden(value_type);
        let ready = self.hidden(Type::Bool);
        let value_expr = self.coerce(self.local(value), destination.1, node)?;
        let body = vec![
            Statement::IterNext {
                iterator,
                key,
                value: Some(value),
                ready,
            },
            Statement::If(
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Not(Box::new(self.local(ready))),
                },
                vec![Statement::Exhaust],
                Vec::new(),
            ),
            Statement::SetItem(self.local(result), self.local(key), value_expr),
            Statement::Clear(key),
            Statement::Clear(value),
        ];
        Ok(Statement::Scope {
            body: vec![
                Statement::Assign(source_slot, source),
                Statement::Assign(
                    iterator,
                    Expr {
                        ty: Type::Iterator,
                        kind: ExprKind::Builtin(
                            Builtin::DictIter(true),
                            vec![self.local(source_slot)],
                        ),
                    },
                ),
                Statement::While(
                    Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Bool(true),
                    },
                    body,
                    Vec::new(),
                ),
            ],
            clear: vec![source_slot, iterator, key, value],
        })
    }

    /// Each ordinary key is evaluated before its value, and each expansion is
    /// exhausted before the next entry. Existing keys keep their first position.
    pub(super) fn expanded_dictionary(
        &mut self,
        dict: &ast::ExprDict,
        mut types: Option<(Type, Type)>,
    ) -> Result<Expr, String> {
        let result = self.hidden(Type::None);
        let mut body = Vec::new();
        for item in &dict.items {
            if let Some(key) = &item.key {
                let key_expr = if let Some((key_type, _)) = types {
                    self.expr_expected(key, key_type)?
                } else {
                    self.expr(key)?
                };
                if !self.types.hashable_key(key_expr.ty) {
                    return Err(error(
                        self.source,
                        key,
                        "dictionary keys must be int, str, bytes, Path, or immutable scalar tuples",
                    ));
                }
                let captured = self.hidden(key_expr.ty);
                let value = if let Some((_, value_type)) = types {
                    self.expr_expected(&item.value, value_type)?
                } else {
                    self.expr(&item.value)?
                };
                let (key_type, value_type) = *types.get_or_insert((key_expr.ty, value.ty));
                self.locals[result] = self.types.dict(key_type, value_type);
                body.push(Statement::Scope {
                    body: vec![
                        Statement::Assign(captured, key_expr),
                        Statement::SetItem(self.local(result), self.local(captured), value),
                    ],
                    clear: vec![captured],
                });
            } else {
                let hint = types.map(|(key, value)| self.types.dict(key, value));
                let value = self.boolean_operand(&item.value, hint)?;
                let mapping = self.types.mapping(value.ty).ok_or_else(|| {
                    error(
                        self.source,
                        &item.value,
                        "dictionary expansion requires a concrete dictionary",
                    )
                })?;
                let destination = *types.get_or_insert(mapping);
                self.locals[result] = self.types.dict(destination.0, destination.1);
                body.push(self.expand_mapping(result, value, destination, &item.value)?);
            }
        }
        let (key, value) = types.expect("expanded dictionary has at least one entry");
        let ty = self.types.dict(key, value);
        body.insert(
            0,
            Statement::Assign(
                result,
                Expr {
                    ty,
                    kind: ExprKind::Dict {
                        key,
                        value,
                        entries: Vec::new(),
                    },
                },
            ),
        );
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear: vec![result],
            },
        })
    }
}
