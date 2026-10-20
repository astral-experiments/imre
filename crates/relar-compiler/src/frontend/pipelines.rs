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
    value: usize,
    ready: usize,
    /// An empty tuple has no element type or executable item assignment.
    elementless: bool,
    pub clear: Vec<usize>,
}

/// These constructors have observable acquisition timing and cannot be passed
/// into an eager comprehension helper as ordinary captured container values.
pub(super) fn is_pipeline_expression(expression: &ast::Expr) -> bool {
    matches!(expression, ast::Expr::Generator(_))
        || matches!(expression, ast::Expr::Call(call)
            if matches!(&*call.func, ast::Expr::Name(name)
                if matches!(name.id.as_str(), "zip" | "reversed" | "map" | "enumerate")))
}

fn integer(value: i64) -> Expr {
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

fn compare(operation: Compare, left: Expr, right: Expr) -> Expr {
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
    ) -> Result<Option<Prepared>, String> {
        if let ast::Expr::Generator(generator) = expression {
            return self.generator_input(generator).map(Some);
        }
        let ast::Expr::Call(call) = expression else {
            return Ok(None);
        };
        if self.graph.resolve(self.module, &call.func).is_some() {
            return Ok(None);
        }
        let ast::Expr::Name(name) = &*call.func else {
            return Ok(None);
        };
        match name.id.as_str() {
            "zip" => self.zip_input(call).map(Some),
            "reversed" => {
                if !call.arguments.keywords.is_empty() || call.arguments.args.len() != 1 {
                    return Err(error(
                        self.source,
                        call,
                        "reversed requires one positional sequence",
                    ));
                }
                let input = self.expansion_input(&call.arguments.args[0], None)?;
                let mut input = self.prepare_iteration(input, call, true)?;
                input.evaluate.append(&mut input.acquire);
                Ok(Some(input))
            }
            "map" => {
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
                Ok(Some(input))
            }
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
                let source = self.expansion_input(&call.arguments.args[0], None)?;
                let mut input = self.prepare_iteration(source, call, false)?;
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
                input.next.push(Statement::If(
                    self.local(input.ready),
                    vec![
                        Statement::Assign(
                            value,
                            Expr {
                                ty,
                                kind: ExprKind::Tuple(vec![
                                    self.local(counter),
                                    self.local(input.value),
                                ]),
                            },
                        ),
                        Statement::Clear(input.value),
                        Statement::Assign(
                            counter,
                            binary(Binary::Add, self.local(counter), integer(1)),
                        ),
                    ],
                    Vec::new(),
                ));
                input.next.insert(0, Statement::Clear(value));
                input.value = value;
                input.elementless = false;
                input.clear.extend([counter, value]);
                Ok(Some(input))
            }
            _ => Ok(None),
        }
    }

    /// Adapts existing concrete iteration without introducing a public iterator
    /// type. Reverse construction supports sequences whose indexing is sealed.
    fn prepare_iteration(
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
            clear: arguments,
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
            clear,
        })
    }

    /// Obtains one element from each source in order. Strict mismatch checking
    /// retains successful earlier pulls and performs only CPython's extra probes.
    fn zip_input(&mut self, call: &ast::ExprCall) -> Result<Prepared, String> {
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
        for expression in &call.arguments.args {
            let input = self.expansion_input(expression, None)?;
            inputs.push(self.prepare_iteration(input, expression, false)?);
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
        for input in &mut inputs {
            evaluate.append(&mut input.evaluate);
            clear.extend(&input.clear);
        }
        evaluate.push(Statement::Assign(strict, strict_expression));
        for input in &mut inputs {
            evaluate.append(&mut input.acquire);
        }
        evaluate.push(Statement::Assign(done, boolean(inputs.is_empty())));
        let mut success = vec![
            Statement::Assign(
                value,
                Expr {
                    ty,
                    kind: ExprKind::Tuple(
                        inputs.iter().map(|input| self.local(input.value)).collect(),
                    ),
                },
            ),
            Statement::Assign(ready, boolean(true)),
        ];
        success.extend(inputs.iter().map(|input| Statement::Clear(input.value)));
        for index in (0..inputs.len()).rev() {
            let mut exhausted = vec![Statement::Assign(done, boolean(true))];
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
            step.push(Statement::If(
                self.local(inputs[index].ready),
                success,
                exhausted,
            ));
            success = step;
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
            clear,
        })
    }

    /// Resumes private comprehension targets while all captured enclosing names
    /// continue to refer to their live slots. Only the leftmost iterator is
    /// acquired at creation; every inner source and predicate remains lazy.
    fn generator_input(&mut self, generator: &ast::ExprGenerator) -> Result<Prepared, String> {
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
        let first = self.expansion_input(&generator.generators[0].iter, None)?;
        let mut first = self.prepare_iteration(first, generator, false)?;
        let before = self.local_state();
        let names = self.names.clone();
        let targets = self.comprehension_targets.clone();
        let first_private = self.locals.len();
        let mut private_targets = HashSet::new();
        for clause in &generator.generators {
            target_names(&clause.target, &mut private_targets);
        }
        for name in &private_targets {
            self.names.remove(name);
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
            let input = if index == 0 {
                first.take().unwrap()
            } else {
                let input = self.expansion_input(&clause.iter, None)?;
                self.prepare_iteration(input, clause, false)?
            };
            bindings.push(self.bind_pattern(&clause.target, self.local(input.value))?);
            let mut conditions = Vec::new();
            for condition in &clause.ifs {
                conditions.push(self.condition(condition)?);
                self.apply_facts(condition, true);
            }
            filters.push(conditions);
            inputs.push(input);
        }
        let yielded = self.expr(&generator.elt)?;
        let value = self.hidden(yielded.ty);
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let depth = self.hidden(Type::Int);
        self.comprehension_depth -= 1;
        self.comprehension_targets = targets;
        self.names = names;
        for (name, &slot) in &self.comprehension_exports {
            if !self.comprehension_targets.contains(name) {
                self.names.insert(name.clone(), slot);
            }
        }
        self.restore_state(before);
        for expression in generator
            .generators
            .iter()
            .flat_map(|clause| clause.ifs.iter())
            .chain([&*generator.elt])
        {
            for name in narrowing::expression_assignments(expression) {
                if let Some(slot) = self.names.get(&name) {
                    self.narrowed.remove(slot);
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
            clear,
        })
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
        let before = self.assigned.clone();
        let mut facts = self.narrowed.clone();
        let binding = self.bind_pattern(target, self.local(input.value))?;
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
