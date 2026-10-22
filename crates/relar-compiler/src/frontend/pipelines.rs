//! Typed iterator pipelines retain their state in private local slots. Creation,
//! iterator acquisition, and each pull stay separate so composition preserves
//! Python's lazy evaluation and mutation checks without escaping iterator values.

use super::*;

/// `evaluate` evaluates an iterable expression; `acquire` obtains its iterator.
/// Constructor expressions fold their own acquisition into `evaluate`. Every
/// successful `next` owns `value`, and its consumer clears that transient owner.
pub(super) struct Prepared {
    pub evaluate: Vec<Statement>,
    acquire: Vec<Statement>,
    next: Vec<Statement>,
    pub(super) value: usize,
    ready: usize,
    /// An empty tuple has no element type or executable item assignment.
    elementless: bool,
    /// Enclosing slots a later pull may replace, after constructor arguments.
    writes: HashSet<usize>,
    pub clear: Vec<usize>,
}

/// These constructors have observable acquisition timing and cannot be passed
/// into an eager comprehension helper as ordinary captured container values.
pub(super) fn is_pipeline_expression(expression: &ast::Expr) -> bool {
    matches!(expression, ast::Expr::Generator(_))
        || matches!(expression, ast::Expr::Call(call)
            if matches!(&*call.func, ast::Expr::Name(name)
                if matches!(name.id.as_str(), "zip" | "reversed" | "map" | "filter" | "enumerate"))
            || matches!(&*call.func, ast::Expr::Attribute(attribute)
                if matches!(attribute.attr.as_str(), "iterdir" | "glob")))
}

pub(super) fn integer(value: i64) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(value.to_string()),
    }
}

fn boolean(value: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(value),
    }
}

fn binary(operation: Binary, left: Expr, right: Expr) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Binary(operation, Box::new(left), Box::new(right)),
    }
}

pub(super) fn compare(operation: Compare, left: Expr, right: Expr) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Compare(vec![left, right], vec![operation]),
    }
}

fn not(value: Expr) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Not(Box::new(value)),
    }
}

fn truth(value: Expr) -> Expr {
    if value.ty == Type::Bool {
        value
    } else {
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::Bool, vec![value]),
        }
    }
}

fn raise_value_error(message: String) -> Statement {
    Statement::Raise(Some(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("ValueError").unwrap(),
            message: Some(Box::new(Expr {
                ty: Type::Str,
                kind: ExprKind::Str(message),
            })),
        },
    }))
}

/// Collects private binding names without treating stores through attributes or
/// subscripts as new locals.
fn target_names(target: &ast::Expr, names: &mut HashSet<String>) {
    if let ast::Expr::Name(name) = target {
        names.insert(name.id.to_string());
    } else if let ast::Expr::Starred(starred) = target {
        target_names(&starred.value, names);
    } else if let Some(elements) = pattern_elements(target) {
        for element in elements {
            target_names(element, names);
        }
    }
}

impl Checker<'_> {
    /// Recognizes only sealed constructor syntax at a direct consumer boundary.
    pub(super) fn pipeline_input(
        &mut self,
        expression: &ast::Expr,
        element: Option<Type>,
    ) -> Result<Option<Iteration>, String> {
        if let ast::Expr::Generator(generator) = expression {
            return Ok(Some(Iteration::Prepared(
                self.generator_input(generator, element)?,
            )));
        }
        let ast::Expr::Call(call) = expression else {
            return Ok(None);
        };
        if self.resolve(&call.func).is_some() {
            return Ok(None);
        }
        if let ast::Expr::Attribute(attribute) = &*call.func
            && matches!(attribute.attr.as_str(), "iterdir" | "glob")
            && self.class_reference(&attribute.value).is_none()
        {
            let receiver = self.expr(&attribute.value)?;
            if receiver.ty == Type::Path {
                let iterator =
                    self.path_iterator(receiver, call, attribute.attr.as_str() == "glob")?;
                return Ok(Some(Iteration::Prepared(
                    self.prepare_path_iterator(iterator),
                )));
            }
            if let Type::Class(class) = receiver.ty {
                // A first-party method with the same spelling returns an ordinary
                // value. Preserve its length hint and list.extend's alias snapshot.
                return Ok(Some(Iteration::Container {
                    iterable: self.class_method_call(call, attribute, class, Some(receiver))?,
                    items: false,
                    start: None,
                }));
            }
            return Err(error(
                self.source,
                call,
                "directory iteration requires pathlib.Path",
            ));
        }
        let ast::Expr::Name(name) = &*call.func else {
            return Ok(None);
        };
        let input = match name.id.as_str() {
            "zip" => self.zip_input(call, element)?,
            "reversed" => {
                if !call.arguments.keywords.is_empty() || call.arguments.args.len() != 1 {
                    return Err(error(
                        self.source,
                        call,
                        "reversed requires one positional sequence",
                    ));
                }
                let input = self.expansion_input(&call.arguments.args[0], element)?;
                let mut input = self.prepare_iteration(input, call, true)?;
                input.evaluate.append(&mut input.acquire);
                input
            }
            "map" => {
                if !matches!(call.arguments.args.as_ref(), [ast::Expr::Name(name), _] if name.id == "int")
                {
                    return Ok(Some(Iteration::Prepared(
                        self.callback_input(call, element, false)?,
                    )));
                }
                if !call.arguments.keywords.is_empty()
                    || !matches!(call.arguments.args.as_ref(), [ast::Expr::Name(name), _] if name.id == "int")
                {
                    return Err(error(
                        self.source,
                        call,
                        "map requires the builtin int converter and one concrete iterable",
                    ));
                }
                let source = self.expansion_input(&call.arguments.args[1], None)?;
                let mut input = self.prepare_iteration(source, call, false)?;
                let value = self.hidden(Type::Int);
                let converted = self.consumer_value(self.local(input.value), true, call)?;
                input.next.push(Statement::If(
                    self.local(input.ready),
                    vec![
                        Statement::Assign(value, converted),
                        Statement::Clear(input.value),
                    ],
                    Vec::new(),
                ));
                input.next.insert(0, Statement::Clear(value));
                input.value = value;
                input.elementless = false;
                input.clear.push(value);
                input.evaluate.append(&mut input.acquire);
                input
            }
            "filter" => self.callback_input(call, element, true)?,
            "enumerate" => {
                if !call.arguments.keywords.is_empty()
                    || !(1..=2).contains(&call.arguments.args.len())
                {
                    return Err(error(
                        self.source,
                        call,
                        "enumerate requires an iterable and an optional positional integer start",
                    ));
                }
                let fields =
                    element.and_then(|ty| self.types.fields(self.types.payload(ty).unwrap_or(ty)));
                let element = fields.as_deref().and_then(|fields| match fields {
                    [Type::Int, element] => Some(*element),
                    _ => None,
                });
                let source = self.expansion_input(&call.arguments.args[0], element)?;
                let mut input = self.prepare_iteration(source, call, false)?;
                self.hint_empty_iteration(&input, element);
                let start = if let Some(start) = call.arguments.args.get(1) {
                    self.expr_expected(start, Type::Int)?
                } else {
                    integer(0)
                };
                let counter = self.hidden(Type::Int);
                input.evaluate.push(Statement::Assign(counter, start));
                input.evaluate.append(&mut input.acquire);
                let ty = self.types.tuple(vec![Type::Int, self.locals[input.value]]);
                let value = self.hidden(ty);
                let cache = self.iterator_tuple_cache(ty, &mut input.evaluate, &mut input.clear);
                let mut result = if let Some(cache) = cache {
                    vec![
                        Statement::Assign(
                            value,
                            Expr {
                                ty,
                                kind: ExprKind::Builtin(
                                    Builtin::TupleCacheResult,
                                    vec![self.local(cache)],
                                ),
                            },
                        ),
                        Statement::SetField(self.local(value), 0, self.local(counter)),
                        Statement::SetField(self.local(value), 1, self.local(input.value)),
                    ]
                } else {
                    vec![Statement::Assign(
                        value,
                        Expr {
                            ty,
                            kind: ExprKind::Tuple(vec![
                                self.local(counter),
                                self.local(input.value),
                            ]),
                        },
                    )]
                };
                result.extend([
                    Statement::Clear(input.value),
                    Statement::Assign(
                        counter,
                        binary(Binary::Add, self.local(counter), integer(1)),
                    ),
                ]);
                input
                    .next
                    .push(Statement::If(self.local(input.ready), result, Vec::new()));
                input.next.insert(0, Statement::Clear(value));
                input.value = value;
                input.elementless = false;
                input.clear.extend([counter, value]);
                input
            }
            _ => return Ok(None),
        };
        Ok(Some(Iteration::Prepared(input)))
    }

    /// Method evaluation creates the iterator at Python's observable point.
    /// Runtime iterdir snapshots eagerly; glob waits for the first pull. Private
    /// nullable results own every yielded path until the consumer acquires it.
    fn prepare_path_iterator(&mut self, expression: Expr) -> Prepared {
        let iterator = self.hidden(Type::Iterator);
        let optional = self.hidden(self.types.optional(Type::Path));
        let value = self.hidden(Type::Path);
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let pull = vec![
            Statement::Assign(
                optional,
                paths::operation(
                    ir::PathOperation::Next,
                    self.locals[optional],
                    vec![self.local(iterator)],
                ),
            ),
            Statement::Assign(
                ready,
                not(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::IsNone, vec![self.local(optional)]),
                }),
            ),
            Statement::If(
                self.local(ready),
                vec![Statement::Assign(
                    value,
                    Expr {
                        ty: Type::Path,
                        kind: ExprKind::Unwrap(Box::new(self.local(optional))),
                    },
                )],
                vec![
                    Statement::Assign(done, boolean(true)),
                    Statement::Clear(iterator),
                ],
            ),
            Statement::Clear(optional),
        ];
        Prepared {
            evaluate: vec![
                Statement::Assign(iterator, expression),
                Statement::Assign(done, boolean(false)),
            ],
            acquire: Vec::new(),
            next: vec![
                Statement::Clear(value),
                Statement::Assign(ready, boolean(false)),
                Statement::If(not(self.local(done)), pull, Vec::new()),
            ],
            value,
            ready,
            elementless: false,
            writes: HashSet::new(),
            clear: vec![iterator, optional, value, ready, done],
        }
    }

    /// Only file resources have observable final drops in the sealed language.
    /// Their transitive owners retain CPython's iterator tuple cache, including
    /// shared-result behavior; other exact tuples can be scalar-replaced.
    fn iterator_tuple_cache(
        &mut self,
        ty: Type,
        evaluate: &mut Vec<Statement>,
        clear: &mut Vec<usize>,
    ) -> Option<usize> {
        fn owns_file(checker: &Checker<'_>, ty: Type) -> bool {
            match ty {
                Type::TextIO | Type::BinaryIO => true,
                Type::Class(class) => checker.classes.fields[class]
                    .iter()
                    .any(|ty| owns_file(checker, *ty)),
                _ => checker
                    .types
                    .children(ty)
                    .into_iter()
                    .any(|ty| owns_file(checker, ty)),
            }
        }
        if !owns_file(self, ty) {
            return None;
        }
        let cache = self.hidden(ty);
        evaluate.push(Statement::Assign(
            cache,
            Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::TupleCacheNew, Vec::new()),
            },
        ));
        clear.push(cache);
        Some(cache)
    }

    /// Adapts existing concrete iteration without introducing a public iterator
    /// type. Reverse construction supports sequences whose indexing is sealed.
    pub(super) fn prepare_iteration(
        &mut self,
        input: Iteration,
        node: &impl Ranged,
        reverse: bool,
    ) -> Result<Prepared, String> {
        match input {
            Iteration::Prepared(input) if !reverse => Ok(input),
            Iteration::Prepared(_) => Err(error(
                self.source,
                node,
                "reversed requires a list, tuple, string, bytes, or range",
            )),
            Iteration::Range(args) => Ok(self.prepare_range(args, reverse)),
            Iteration::Container {
                iterable,
                items,
                start: None,
            } => self.prepare_container(iterable, items, node, reverse),
            Iteration::Container { start: Some(_), .. } => {
                unreachable!("enumerate constructors use the shared pipeline path")
            }
        }
    }

    /// A finite range keeps arbitrary-precision cursor arithmetic. Constructing
    /// it checks the step before later call arguments; reversing never uses len.
    fn prepare_range(&mut self, args: Vec<Expr>, reverse: bool) -> Prepared {
        let mut evaluate = Vec::new();
        let mut arguments = Vec::new();
        for argument in args {
            let slot = self.hidden(Type::Int);
            evaluate.push(Statement::Assign(slot, argument));
            arguments.push(slot);
        }
        let (start, stop, step) = match arguments.as_slice() {
            [stop] => (integer(0), self.local(*stop), integer(1)),
            [start, stop] => (self.local(*start), self.local(*stop), integer(1)),
            [start, stop, step] => (self.local(*start), self.local(*stop), self.local(*step)),
            _ => unreachable!("range arity was checked"),
        };
        if arguments.len() == 3 {
            evaluate.push(Statement::If(
                compare(Compare::Equal, step.clone(), integer(0)),
                vec![raise_value_error("range() arg 3 must not be zero".into())],
                Vec::new(),
            ));
        }
        let cursor = self.hidden(Type::Int);
        let end = self.hidden(Type::Int);
        let stride = self.hidden(Type::Int);
        let value = self.hidden(Type::Int);
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let positive = compare(Compare::Greater, step.clone(), integer(0));
        let nonempty = Expr {
            ty: Type::Bool,
            kind: ExprKind::Conditional(
                Box::new(positive.clone()),
                Box::new(compare(Compare::Less, start.clone(), stop.clone())),
                Box::new(compare(Compare::Greater, start.clone(), stop.clone())),
            ),
        };
        let mut acquire = vec![Statement::Assign(done, boolean(false))];
        if reverse {
            let sign = Expr {
                ty: Type::Int,
                kind: ExprKind::Conditional(
                    Box::new(positive),
                    Box::new(integer(1)),
                    Box::new(integer(-1)),
                ),
            };
            let last = binary(
                Binary::Add,
                start.clone(),
                binary(
                    Binary::Multiply,
                    binary(
                        Binary::FloorDivide,
                        binary(
                            Binary::Subtract,
                            binary(Binary::Subtract, stop, start.clone()),
                            sign,
                        ),
                        step.clone(),
                    ),
                    step.clone(),
                ),
            );
            acquire.push(Statement::If(
                nonempty,
                vec![
                    Statement::Assign(cursor, last),
                    Statement::Assign(end, binary(Binary::Subtract, start, step.clone())),
                    Statement::Assign(
                        stride,
                        Expr {
                            ty: Type::Int,
                            kind: ExprKind::Negate(Box::new(step)),
                        },
                    ),
                ],
                vec![
                    Statement::Assign(cursor, integer(0)),
                    Statement::Assign(end, integer(0)),
                    Statement::Assign(stride, integer(1)),
                ],
            ));
        } else {
            acquire.extend([
                Statement::Assign(cursor, start),
                Statement::Assign(end, stop),
                Statement::Assign(stride, step),
            ]);
        }
        let available = Expr {
            ty: Type::Bool,
            kind: ExprKind::Conditional(
                Box::new(compare(Compare::Greater, self.local(stride), integer(0))),
                Box::new(compare(Compare::Less, self.local(cursor), self.local(end))),
                Box::new(compare(
                    Compare::Greater,
                    self.local(cursor),
                    self.local(end),
                )),
            ),
        };
        let next = vec![
            Statement::Clear(value),
            Statement::Assign(ready, boolean(false)),
            Statement::If(
                not(self.local(done)),
                vec![Statement::If(
                    available,
                    vec![
                        Statement::Assign(value, self.local(cursor)),
                        Statement::Assign(
                            cursor,
                            binary(Binary::Add, self.local(cursor), self.local(stride)),
                        ),
                        Statement::Assign(ready, boolean(true)),
                    ],
                    vec![Statement::Assign(done, boolean(true))],
                )],
                Vec::new(),
            ),
        ];
        arguments.extend([cursor, end, stride, value, ready, done]);
        Prepared {
            evaluate,
            acquire,
            next,
            value,
            ready,
            elementless: false,
            writes: HashSet::new(),
            clear: arguments,
        }
    }

    /// A file is its own iterator. Acquiring it checks for closure, and pulls
    /// share its position with every reader, including a nested loop on it.
    pub(super) fn prepare_file_lines(&mut self, file: Expr) -> Prepared {
        let line_type = if file.ty == Type::BinaryIO {
            Type::Bytes
        } else {
            Type::Str
        };
        let source = self.hidden(file.ty);
        let value = self.hidden(line_type);
        let ready = self.hidden(Type::Bool);
        Prepared {
            evaluate: vec![Statement::Assign(source, file)],
            acquire: vec![Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::TextEnter, vec![self.local(source)]),
            })],
            next: vec![
                Statement::Assign(
                    value,
                    Expr {
                        ty: line_type,
                        kind: ExprKind::Builtin(Builtin::FileReadline, vec![self.local(source)]),
                    },
                ),
                Statement::Assign(ready, truth(self.local(value))),
            ],
            value,
            ready,
            elementless: false,
            writes: HashSet::new(),
            clear: vec![source, value, ready],
        }
    }

    /// List cursors re-read the live length; reverse cursors capture only their
    /// starting index. Dict/set/string cursors use the rooted runtime protocol.
    fn prepare_container(
        &mut self,
        iterable: Expr,
        items: bool,
        node: &impl Ranged,
        reverse: bool,
    ) -> Result<Prepared, String> {
        if !reverse && matches!(iterable.ty, Type::TextIO | Type::BinaryIO) {
            return Ok(self.prepare_file_lines(iterable));
        }
        let mapping = self.types.mapping(iterable.ty);
        let set_element = self.types.set_element(iterable.ty);
        let fields = self.types.fields(iterable.ty);
        if fields.as_ref().is_some_and(Vec::is_empty) {
            let source = self.hidden(iterable.ty);
            let value = self.hidden(Type::Int);
            let ready = self.hidden(Type::Bool);
            return Ok(Prepared {
                evaluate: vec![Statement::Assign(source, iterable)],
                acquire: Vec::new(),
                next: vec![
                    Statement::Clear(value),
                    Statement::Assign(ready, boolean(false)),
                ],
                value,
                ready,
                elementless: true,
                writes: HashSet::new(),
                clear: vec![source, value, ready],
            });
        }
        let tuple_element = fields.as_ref().and_then(|fields| {
            let first = fields.first().copied().unwrap_or(Type::None);
            fields.iter().all(|ty| *ty == first).then_some(first)
        });
        let element = self.types.element(iterable.ty)
            .or(set_element)
            .or((iterable.ty == Type::Str).then_some(Type::Str))
            .or((iterable.ty == Type::Bytes).then_some(Type::Int))
            .or(tuple_element)
            .or_else(|| mapping.map(|(key, value)| if items { self.types.tuple(vec![key, value]) } else { key }))
            .ok_or_else(|| error(self.source, node, "iteration requires a concrete container, homogeneous tuple, string, bytes, or range"))?;
        if reverse && (mapping.is_some() || set_element.is_some() || items) {
            return Err(error(
                self.source,
                node,
                "reversed requires a list, tuple, string, bytes, or range",
            ));
        }
        let ty = iterable.ty;
        let source = self.hidden(ty);
        let value = self.hidden(element);
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let evaluate = vec![Statement::Assign(source, iterable)];
        let mut acquire = vec![Statement::Assign(done, boolean(false))];
        let mut clear = vec![source, value, ready, done];
        let mut pull = Vec::new();
        let indexed =
            self.types.element(ty).is_some() || fields.is_some() || (reverse && ty == Type::Bytes);
        if indexed {
            let cursor = self.hidden(Type::Int);
            clear.push(cursor);
            let length = || Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(
                    Builtin::Len,
                    vec![Expr {
                        ty,
                        kind: ExprKind::Local(source),
                    }],
                ),
            };
            acquire.push(Statement::Assign(
                cursor,
                if reverse {
                    binary(Binary::Subtract, length(), integer(1))
                } else {
                    integer(0)
                },
            ));
            let in_bounds = compare(Compare::Less, self.local(cursor), length());
            let available = if reverse {
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Boolean(
                        true,
                        vec![
                            compare(Compare::GreaterEqual, self.local(cursor), integer(0)),
                            in_bounds,
                        ],
                    ),
                }
            } else {
                in_bounds
            };
            let item = if let Some(fields) = fields {
                let mut item = Expr {
                    ty: element,
                    kind: ExprKind::None,
                };
                // An empty tuple never reaches this expression. For a nonempty
                // homogeneous tuple every dynamic arm has one concrete layout.
                for index in (0..fields.len()).rev() {
                    let field = Expr {
                        ty: element,
                        kind: ExprKind::Field(Box::new(self.local(source)), index),
                    };
                    item = if index + 1 == fields.len() {
                        field
                    } else {
                        Expr {
                            ty: element,
                            kind: ExprKind::Conditional(
                                Box::new(compare(
                                    Compare::Equal,
                                    self.local(cursor),
                                    integer(index as i64),
                                )),
                                Box::new(field),
                                Box::new(item),
                            ),
                        }
                    };
                }
                item
            } else {
                Expr {
                    ty: element,
                    kind: ExprKind::Builtin(
                        if ty == Type::Bytes {
                            Builtin::BytesGet
                        } else if matches!(ty, Type::List(_)) {
                            Builtin::SequenceGetInBounds
                        } else {
                            Builtin::ListGet
                        },
                        vec![self.local(source), self.local(cursor)],
                    ),
                }
            };
            pull.push(Statement::If(
                available,
                vec![
                    Statement::Assign(value, item),
                    Statement::Assign(
                        cursor,
                        binary(
                            if reverse {
                                Binary::Subtract
                            } else {
                                Binary::Add
                            },
                            self.local(cursor),
                            integer(1),
                        ),
                    ),
                    Statement::Assign(ready, boolean(true)),
                ],
                vec![
                    Statement::Assign(done, boolean(true)),
                    Statement::Clear(source),
                ],
            ));
        } else {
            let iterator = self.hidden(Type::Iterator);
            clear.push(iterator);
            let builtin = if ty == Type::Str {
                if reverse {
                    Builtin::StringReverseIter
                } else {
                    Builtin::StringIter
                }
            } else if ty == Type::Bytes {
                Builtin::BytesIter
            } else if set_element.is_some() {
                Builtin::SetIter
            } else {
                Builtin::DictIter(items)
            };
            acquire.push(Statement::Assign(
                iterator,
                Expr {
                    ty: Type::Iterator,
                    kind: ExprKind::Builtin(builtin, vec![self.local(source)]),
                },
            ));
            acquire.push(Statement::Clear(source));
            let key = if items {
                self.hidden(mapping.unwrap().0)
            } else {
                value
            };
            let second = items.then(|| self.hidden(mapping.unwrap().1));
            if items {
                clear.push(key);
            }
            clear.extend(second);
            pull.push(Statement::IterNext {
                iterator,
                key,
                value: second,
                ready,
            });
            if let Some(second) = second {
                pull.push(Statement::If(
                    self.local(ready),
                    vec![
                        Statement::Assign(
                            value,
                            Expr {
                                ty: element,
                                kind: ExprKind::Tuple(vec![self.local(key), self.local(second)]),
                            },
                        ),
                        Statement::Clear(key),
                        Statement::Clear(second),
                    ],
                    Vec::new(),
                ));
            }
            pull.push(Statement::If(
                not(self.local(ready)),
                vec![
                    Statement::Assign(done, boolean(true)),
                    Statement::Clear(iterator),
                ],
                Vec::new(),
            ));
        }
        let next = vec![
            Statement::Clear(value),
            Statement::Assign(ready, boolean(false)),
            Statement::If(not(self.local(done)), pull, Vec::new()),
        ];
        Ok(Prepared {
            evaluate,
            acquire,
            next,
            value,
            ready,
            elementless: false,
            writes: HashSet::new(),
            clear,
        })
    }

    /// Obtains one element from each source in order. Strict mismatch checking
    /// retains successful earlier pulls and performs only CPython's extra probes.
    ///
    /// ```python
    /// any(zip([1], [2, 3], strict=True))  # True: no second row is requested.
    /// ```
    fn zip_input(
        &mut self,
        call: &ast::ExprCall,
        element: Option<Type>,
    ) -> Result<Prepared, String> {
        if call
            .arguments
            .args
            .iter()
            .any(|arg| matches!(arg, ast::Expr::Starred(_)))
            || call.arguments.keywords.len() > 1
            || call
                .arguments
                .keywords
                .iter()
                .any(|keyword| keyword.arg.as_ref().map(|arg| arg.as_str()) != Some("strict"))
        {
            return Err(error(
                self.source,
                call,
                "zip accepts positional iterables and optional strict=",
            ));
        }
        let mut inputs = Vec::new();
        let fields = element.and_then(|ty| self.types.fields(self.types.payload(ty).unwrap_or(ty)));
        for (index, expression) in call.arguments.args.iter().enumerate() {
            let hint = fields
                .as_ref()
                .and_then(|fields| fields.get(index))
                .copied();
            let input = self.expansion_input(expression, hint)?;
            let input = self.prepare_iteration(input, expression, false)?;
            self.hint_empty_iteration(&input, hint);
            inputs.push(input);
        }
        let strict_expression = match call.arguments.keywords.first() {
            Some(keyword) => self.condition(&keyword.value)?,
            None => boolean(false),
        };
        let strict = self.hidden(Type::Bool);
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let ty = self.types.tuple(
            inputs
                .iter()
                .map(|input| self.locals[input.value])
                .collect(),
        );
        let value = self.hidden(ty);
        let mut evaluate = Vec::new();
        let mut clear = vec![strict, ready, done, value];
        let writes = inputs
            .iter()
            .flat_map(|input| input.writes.iter().copied())
            .collect();
        for input in &mut inputs {
            evaluate.append(&mut input.evaluate);
            clear.extend(&input.clear);
        }
        evaluate.push(Statement::Assign(strict, strict_expression));
        for input in &mut inputs {
            evaluate.append(&mut input.acquire);
        }
        let cache = self.iterator_tuple_cache(ty, &mut evaluate, &mut clear);
        evaluate.push(Statement::Assign(done, boolean(inputs.is_empty())));
        let mut success = vec![Statement::Assign(ready, boolean(true))];
        if cache.is_none() {
            success.insert(
                0,
                Statement::Assign(
                    value,
                    Expr {
                        ty,
                        kind: ExprKind::Tuple(
                            inputs.iter().map(|input| self.local(input.value)).collect(),
                        ),
                    },
                ),
            );
        }
        success.extend(inputs.iter().map(|input| Statement::Clear(input.value)));
        for index in (0..inputs.len()).rev() {
            let mut exhausted = vec![
                Statement::Assign(done, boolean(true)),
                Statement::Clear(value),
            ];
            let strict_checks = if index == 0 {
                let mut probes = Vec::new();
                for (other_index, input) in inputs.iter().enumerate().skip(1) {
                    probes.extend(input.next.clone());
                    let previous = if other_index == 1 {
                        "argument 1".into()
                    } else {
                        format!("arguments 1-{other_index}")
                    };
                    probes.push(Statement::If(
                        self.local(input.ready),
                        vec![raise_value_error(format!(
                            "zip() argument {} is longer than {previous}",
                            other_index + 1
                        ))],
                        Vec::new(),
                    ));
                }
                probes
            } else {
                let previous = if index == 1 {
                    "argument 1".into()
                } else {
                    format!("arguments 1-{index}")
                };
                vec![raise_value_error(format!(
                    "zip() argument {} is shorter than {previous}",
                    index + 1
                ))]
            };
            exhausted.push(Statement::If(self.local(strict), strict_checks, Vec::new()));
            exhausted.extend(
                inputs
                    .iter()
                    .flat_map(|input| input.clear.iter().copied())
                    .map(Statement::Clear),
            );
            let mut step = inputs[index].next.clone();
            if cache.is_some() {
                success.insert(0, Statement::Clear(inputs[index].value));
                success.insert(
                    0,
                    Statement::SetField(self.local(value), index, self.local(inputs[index].value)),
                );
            }
            step.push(Statement::If(
                self.local(inputs[index].ready),
                success,
                exhausted,
            ));
            success = step;
        }
        if let Some(cache) = cache {
            success.insert(
                0,
                Statement::Assign(
                    value,
                    Expr {
                        ty,
                        kind: ExprKind::Builtin(Builtin::TupleCacheResult, vec![self.local(cache)]),
                    },
                ),
            );
        }
        let next = vec![
            Statement::Clear(value),
            Statement::Assign(ready, boolean(false)),
            Statement::If(not(self.local(done)), success, Vec::new()),
        ];
        Ok(Prepared {
            evaluate,
            acquire: Vec::new(),
            next,
            value,
            ready,
            elementless: false,
            writes,
            clear,
        })
    }

    /// Resumes private comprehension targets while all captured enclosing names
    /// continue to refer to their live slots. Only the leftmost iterator is
    /// acquired at creation; every inner source and predicate remains lazy.
    ///
    /// ```python
    /// factor = 1
    /// sum((factor * x for x in [1, 2]), (factor := 2))  # 8
    /// ```
    fn generator_input(
        &mut self,
        generator: &ast::ExprGenerator,
        element: Option<Type>,
    ) -> Result<Prepared, String> {
        if generator.generators.iter().any(|clause| clause.is_async) {
            return Err(error(
                self.source,
                generator,
                "asynchronous generator expressions are unsupported",
            ));
        }
        if generator
            .generators
            .iter()
            .any(|clause| !narrowing::expression_assignments(&clause.iter).is_empty())
        {
            return Err(error(
                self.source,
                generator,
                "assignment expressions are forbidden in comprehension iterables",
            ));
        }
        if generator
            .generators
            .iter()
            .any(|clause| !narrowing::expression_assignments(&clause.target).is_empty())
        {
            return Err(error(
                self.source,
                generator,
                "assignment expressions are forbidden in comprehension targets",
            ));
        }
        let first = self.expansion_input(&generator.generators[0].iter, None)?;
        let mut first = self.prepare_iteration(first, generator, false)?;
        let before = self.local_state();
        let names = self.names.clone();
        let class_receiver = self.class_receiver.clone();
        let targets = self.comprehension_targets.clone();
        let first_private = self.locals.len();
        let mut private_targets = HashSet::new();
        for clause in &generator.generators {
            target_names(&clause.target, &mut private_targets);
        }
        for name in &private_targets {
            self.names.remove(name);
        }
        if self
            .class_receiver
            .as_ref()
            .is_some_and(|name| private_targets.contains(name))
        {
            self.class_receiver = None;
        }
        self.comprehension_targets.extend(private_targets);
        self.comprehension_depth += 1;
        // Suspended generators can observe later enclosing writes. Facts about
        // those mutable slots must be proved again inside the lazy body.
        self.narrowed.clear();
        let mut inputs = Vec::new();
        let mut bindings = Vec::new();
        let mut filters = Vec::new();
        first.evaluate.append(&mut first.acquire);
        let mut first = Some(first);
        for (index, clause) in generator.generators.iter().enumerate() {
            let input = if index == 0 {
                first.take().unwrap()
            } else {
                // An inner source is constructed immediately under the outer
                // clause's successful filters, before the first suspension.
                let input = self.expansion_input(&clause.iter, None)?;
                self.prepare_iteration(input, clause, false)?
            };
            if index != 0 {
                // Later pulls resume this clause without rechecking its outer
                // filters. Only private target slots cannot be rebound by a
                // consumer or a sibling iterator while the generator pauses.
                let stable = self
                    .comprehension_targets
                    .iter()
                    .filter_map(|name| self.names.get(name).copied())
                    .collect::<HashSet<_>>();
                self.narrowed.retain(|slot, _| stable.contains(slot));
            }
            let repeated_writes = generator.generators[index..]
                .iter()
                .flat_map(|clause| std::iter::once(&clause.target).chain(clause.ifs.iter()))
                .chain([&*generator.elt])
                .flat_map(narrowing::expression_assignments)
                .collect::<HashSet<_>>();
            for name in repeated_writes {
                if let Some(slot) = self.names.get(&name) {
                    self.narrowed.remove(slot);
                }
            }
            bindings.push(self.bind_iterator_pattern(&clause.target, input.value)?);
            let mut conditions = Vec::new();
            for condition in &clause.ifs {
                conditions.push(self.condition(condition)?);
                self.apply_facts(condition, true);
            }
            filters.push(conditions);
            inputs.push(input);
        }
        let yielded = if let Some(element) = element {
            self.expr_expected(&generator.elt, element)?
        } else {
            self.expr(&generator.elt)?
        };
        let value = self.hidden(yielded.ty);
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let depth = self.hidden(Type::Int);
        self.comprehension_depth -= 1;
        self.comprehension_targets = targets;
        self.names = names;
        self.class_receiver = class_receiver;
        for (name, &slot) in &self.comprehension_exports {
            if !self.comprehension_targets.contains(name) {
                self.names.insert(name.clone(), slot);
            }
        }
        self.restore_state(before);
        let mut writes = inputs
            .iter()
            .flat_map(|input| input.writes.iter().copied())
            .collect::<HashSet<_>>();
        for expression in generator
            .generators
            .iter()
            .flat_map(|clause| clause.ifs.iter())
            .chain([&*generator.elt])
        {
            for name in narrowing::expression_assignments(expression) {
                if let Some(slot) = self.names.get(&name) {
                    self.narrowed.remove(slot);
                    writes.insert(*slot);
                }
            }
        }
        let exported = self
            .comprehension_exports
            .values()
            .copied()
            .collect::<HashSet<_>>();
        let mut clear = inputs[0].clear.clone();
        clear.extend((first_private..self.locals.len()).filter(|slot| !exported.contains(slot)));
        let mut evaluate = std::mem::take(&mut inputs[0].evaluate);
        evaluate.extend([
            Statement::Assign(depth, integer(0)),
            Statement::Assign(done, boolean(false)),
        ]);
        let mut cases = Vec::new();
        for index in (0..inputs.len()).rev() {
            let mut accepted = if index + 1 == inputs.len() {
                vec![
                    Statement::Assign(value, yielded.clone()),
                    Statement::Assign(ready, boolean(true)),
                ]
            } else {
                let mut initialize = inputs[index + 1].evaluate.clone();
                initialize.extend(inputs[index + 1].acquire.clone());
                initialize.push(Statement::Assign(depth, integer((index + 1) as i64)));
                initialize
            };
            for condition in filters[index].iter().rev() {
                accepted = vec![Statement::If(condition.clone(), accepted, Vec::new())];
            }
            let mut available = bindings[index].clone();
            available.push(Statement::Clear(inputs[index].value));
            available.extend(accepted);
            // Exhausting the outermost clause destroys the suspended frame
            // before a surrounding zip can perform its remaining strict probes.
            let exhausted_roots = if index == 0 {
                &clear
            } else {
                &inputs[index].clear
            };
            let mut exhausted = exhausted_roots
                .iter()
                .copied()
                .map(Statement::Clear)
                .collect::<Vec<_>>();
            exhausted.push(if index == 0 {
                Statement::Assign(done, boolean(true))
            } else {
                Statement::Assign(depth, integer((index - 1) as i64))
            });
            let mut step = inputs[index].next.clone();
            step.push(Statement::If(
                self.local(inputs[index].ready),
                available,
                exhausted,
            ));
            cases = vec![Statement::If(
                compare(Compare::Equal, self.local(depth), integer(index as i64)),
                step,
                cases,
            )];
        }
        let next = vec![
            Statement::Clear(value),
            Statement::Assign(ready, boolean(false)),
            Statement::While(
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Boolean(
                        true,
                        vec![not(self.local(ready)), not(self.local(done))],
                    ),
                },
                cases,
                Vec::new(),
            ),
        ];
        Ok(Prepared {
            evaluate,
            acquire: Vec::new(),
            next,
            value,
            ready,
            elementless: false,
            writes,
            clear,
        })
    }

    /// Captures the callable before its source, then invokes it only after each
    /// pull has independently rooted an item and released runtime storage access.
    fn callback_input(
        &mut self,
        call: &ast::ExprCall,
        element: Option<Type>,
        filter: bool,
    ) -> Result<Prepared, String> {
        let [callback, source] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "map/filter require a callback and one concrete iterable",
            ));
        };
        if !call.arguments.keywords.is_empty() {
            return Err(error(
                self.source,
                call,
                "map/filter do not accept keyword arguments",
            ));
        }
        let definition = match self.resolve(callback) {
            Some(Binding::Definition(definition)) => self.functions.get(&definition).cloned(),
            _ => None,
        };
        // Fixed function names and noncapturing lambdas have no enclosing-slot
        // effects. Their input type can therefore be learned from the source;
        // emitted creation still precedes that source's evaluation.
        let checked = if definition.is_some() || matches!(callback, ast::Expr::Lambda(_)) {
            None
        } else {
            Some(self.expr(callback)?)
        };
        let callback_input = checked
            .as_ref()
            .and_then(|value| self.types.signature(value.ty))
            .and_then(|(params, _)| (params.len() == 1).then(|| params[0]))
            .or_else(|| {
                definition.as_ref().and_then(|signature| {
                    if signature.params.len() != 1 {
                        return None;
                    }
                    self.inference
                        .map_or(Some(signature.params[0].ty), |inference| {
                            inference.parameter(signature.id, 0)
                        })
                })
            });
        let input_hint = callback_input.or(if filter { element } else { None });
        let source = self.expansion_input(source, input_hint)?;
        let mut input = self.prepare_iteration(source, call, false)?;
        let input_type = self.locals[input.value];
        let callback = if let ast::Expr::Lambda(lambda) = callback {
            self.lambda_value(
                lambda,
                Some(vec![input_type]),
                if filter { None } else { element },
            )?
        } else if let Some(signature) = definition {
            if signature.params.len() == 1
                && let Some(inference) = self.inference
            {
                inference.learn_parameter(signature.id, 0, input_type);
            }
            self.callable_value(&signature, None, None, callback)?
        } else {
            checked.expect("ordinary callback was checked before its source")
        };
        let callback_slot = self.hidden(callback.ty);
        let operation = if filter && callback.ty == Type::None {
            self.local(input.value)
        } else {
            let (params, returns) = self.types.signature(callback.ty).ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "map/filter require an exact Callable; only filter also accepts None",
                )
            })?;
            let [parameter] = params.as_slice() else {
                return Err(error(
                    self.source,
                    call,
                    "map/filter callbacks require one positional parameter",
                ));
            };
            Expr {
                ty: returns,
                kind: ExprKind::Invoke {
                    callable: Box::new(self.local(callback_slot)),
                    args: ir::CallArgs::positional(vec![self.coerce(
                        self.local(input.value),
                        *parameter,
                        call,
                    )?]),
                },
            }
        };
        input
            .evaluate
            .insert(0, Statement::Assign(callback_slot, callback));
        input.evaluate.append(&mut input.acquire);
        input.clear.push(callback_slot);
        if filter {
            let ready = self.hidden(Type::Bool);
            let done = self.hidden(Type::Bool);
            let mut step = std::mem::take(&mut input.next);
            step.push(Statement::If(
                self.local(input.ready),
                vec![Statement::If(
                    truth(operation),
                    vec![Statement::Assign(ready, boolean(true))],
                    vec![Statement::Clear(input.value)],
                )],
                vec![Statement::Assign(done, boolean(true))],
            ));
            input.evaluate.push(Statement::Assign(done, boolean(false)));
            input.next = vec![
                Statement::Clear(input.value),
                Statement::Assign(ready, boolean(false)),
                Statement::While(
                    Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Boolean(
                            true,
                            vec![not(self.local(ready)), not(self.local(done))],
                        ),
                    },
                    step,
                    Vec::new(),
                ),
            ];
            input.ready = ready;
            input.clear.extend([ready, done]);
        } else {
            let value = self.hidden(operation.ty);
            input.next.push(Statement::If(
                self.local(input.ready),
                vec![
                    Statement::Assign(value, operation),
                    Statement::Clear(input.value),
                ],
                Vec::new(),
            ));
            input.next.insert(0, Statement::Clear(value));
            input.value = value;
            input.elementless = false;
            input.clear.push(value);
        }
        Ok(input)
    }

    /// A source loop owns its complete pipeline across break, return, and error.
    /// Normal exhaustion drops the iterator before the user-visible else suite.
    pub(super) fn lower_prepared_iteration(
        &mut self,
        target: &ast::Expr,
        mut input: Prepared,
        orelse: &[ast::Stmt],
        before_iteration: Vec<Statement>,
        lower_body: impl FnOnce(&mut Self) -> Result<Vec<Statement>, String>,
    ) -> Result<(Vec<Statement>, bool), String> {
        // A later consumer argument or zip constructor argument may have
        // established new facts since this lazy body was checked.
        for slot in &input.writes {
            self.narrowed.remove(slot);
        }
        let before = self.assigned.clone();
        let mut facts = self.narrowed.clone();
        let binding = self.bind_iterator_pattern(target, input.value)?;
        let mut rebound = narrowing::expression_assignments(target);
        target_names(target, &mut rebound);
        for name in rebound {
            if let Some(slot) = self.names.get(&name) {
                facts.remove(slot);
            }
        }
        let mut body = input.next;
        body.push(Statement::If(
            not(self.local(input.ready)),
            vec![Statement::Exhaust],
            Vec::new(),
        ));
        body.extend(binding);
        body.push(Statement::Clear(input.value));
        self.loop_breaks.push(Vec::new());
        body.extend(lower_body(self)?);
        let breaks = self.loop_breaks.pop().unwrap();
        self.assigned = before;
        self.narrowed = facts;
        let (mut orelse, falls) = self.loop_else(orelse, breaks)?;
        if !orelse.is_empty() {
            orelse = input
                .clear
                .iter()
                .copied()
                .map(Statement::Clear)
                .chain(orelse)
                .collect();
        }
        input.evaluate.append(&mut input.acquire);
        input.evaluate.extend(before_iteration);
        input
            .evaluate
            .push(Statement::While(boolean(true), body, orelse));
        Ok((
            vec![Statement::Scope {
                body: input.evaluate,
                clear: input.clear,
            }],
            falls,
        ))
    }

    /// A statically empty tuple can adopt its consumer's element layout because
    /// it has no item-producing branch that could violate that representation.
    pub(super) fn hint_empty_iteration(&mut self, input: &Prepared, element: Option<Type>) {
        if input.elementless
            && let Some(element) = element
        {
            self.locals[input.value] = element;
        }
    }

    /// Quantifiers consume truth values immediately and never pull after their
    /// deciding element. Fixed tuples preserve eager field evaluation first.
    pub(super) fn quantifier_call(
        &mut self,
        call: &ast::ExprCall,
        all: bool,
    ) -> Result<Expr, String> {
        if !call.arguments.keywords.is_empty() || call.arguments.args.len() != 1 {
            return Err(error(
                self.source,
                call,
                "any and all require one positional iterable",
            ));
        }
        let source = &call.arguments.args[0];
        let input = self.expansion_input(source, None)?;
        let result = self.hidden(Type::Bool);
        let mut body = vec![Statement::Assign(result, boolean(all))];
        let mut clear = vec![result];
        if let Iteration::Container {
            iterable,
            items: false,
            start: None,
        } = &input
            && let Some(fields) = self.types.fields(iterable.ty)
        {
            let captured = self.hidden(iterable.ty);
            clear.push(captured);
            body.push(Statement::Assign(captured, iterable.clone()));
            for (index, ty) in fields.into_iter().enumerate() {
                let value = truth(Expr {
                    ty,
                    kind: ExprKind::Field(Box::new(self.local(captured)), index),
                });
                body.push(Statement::If(
                    if all {
                        self.local(result)
                    } else {
                        not(self.local(result))
                    },
                    vec![Statement::Assign(result, value)],
                    Vec::new(),
                ));
            }
        } else {
            body.extend(self.expand_items(input, call, |_checker, value| {
                let deciding = if all { not(truth(value)) } else { truth(value) };
                Ok(vec![Statement::If(
                    deciding,
                    vec![Statement::Assign(result, boolean(!all)), Statement::Break],
                    Vec::new(),
                )])
            })?);
        }
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear,
            },
        })
    }
}
