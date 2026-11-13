//! Eager typed library operations reuse the existing iterator and callback IR.

use super::*;

/// These calls bind the original string method and its operands in the Python frame.
pub(super) fn registered_string_method(call: &ast::ExprCall) -> Option<&ast::ExprAttribute> {
    let method = call.func.as_attribute_expr()?;
    let args = &call.arguments;
    let supported = match method.attr.as_str() {
        "removeprefix" | "format_map" => {
            matches!(args.args.as_slice(), [arg] if !arg.is_starred_expr())
                && args.keywords.is_empty()
        }
        "format" => {
            !method.value.is_string_literal_expr()
                && !args.args.is_empty()
                && !args.args.iter().any(ast::Expr::is_starred_expr)
                && args.keywords.iter().all(|kw| kw.arg.is_some())
        }
        _ => false,
    };
    supported.then_some(method)
}

fn replace_arguments(call: &ast::ExprCall) -> Option<(&ast::Expr, &ast::Expr, Option<&ast::Expr>)> {
    match (&*call.arguments.args, &*call.arguments.keywords) {
        ([old, new], []) => Some((old, new, None)),
        ([old, new, count], []) => Some((old, new, Some(count))),
        ([old, new], [kw]) if kw.arg.as_deref() == Some("count") => {
            Some((old, new, Some(&kw.value)))
        }
        _ => None,
    }
}

/// The receiver fixes the method, and the completed operand has a builtin non-string class.
fn fixed_replace_operand(call: &ast::ExprCall, operand: &ast::Expr) -> bool {
    let Some(method) = call.func.as_attribute_expr() else {
        return false;
    };
    if method.attr.as_str() != "replace" || !method.value.is_string_literal_expr() {
        return false;
    }
    match operand {
        ast::Expr::List(_) | ast::Expr::Tuple(_) | ast::Expr::Set(_) | ast::Expr::Dict(_) => true,
        ast::Expr::UnaryOp(op) => {
            matches!(op.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub)
                && op.operand.is_number_literal_expr()
        }
        _ => operand.is_literal_expr() && !operand.is_string_literal_expr(),
    }
}

/// Select an existing ty overload diagnostic after the typed frontend stopped lowering.
pub(crate) fn literal_replace_mismatch(call: &ast::ExprCall) -> bool {
    !call.arguments.args.iter().any(ast::Expr::is_starred_expr)
        && replace_arguments(call).is_some_and(|(old, new, _)| {
            fixed_replace_operand(call, old) || fixed_replace_operand(call, new)
        })
}

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

/// Lower a unary math operation when the argument has a native numeric representation.
pub(super) fn unary_float_builtin(builtin: Builtin, value: Expr) -> Option<Expr> {
    let (builtin, conversion, ty) = match (builtin, value.ty) {
        // A Python int can have a finite logarithm even when float(int) overflows.
        (Builtin::FloatLog10, Type::Int | Type::Bool) => {
            (Builtin::IntLog10, Builtin::Int(None), Type::Int)
        }
        (op, Type::Float | Type::Int | Type::Bool) => (op, Builtin::Float(None), Type::Float),
        _ => return None,
    };
    let number = Expr {
        ty,
        kind: ExprKind::Builtin(conversion, vec![value]),
    };
    Some(Expr {
        ty: Type::Float,
        kind: ExprKind::Builtin(builtin, vec![number]),
    })
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

/// Rewrite only automatic, unadorned fields; the existing keyword formatter
/// will unescape literal braces and render the selected scalar values.
fn positional_template(mut rest: &str, arguments: usize) -> Option<String> {
    let mut template = String::new();
    let mut used = 0;
    let mut literal_chars = 0;
    while let Some(at) = rest.find(['{', '}']) {
        let (literal, field) = rest.split_at(at);
        template.push_str(literal);
        literal_chars += literal.chars().count();
        let (pair, tail) = field.split_at_checked(2)?;
        match pair {
            "{{" | "}}" => {
                template.push_str(pair);
                literal_chars += 1;
            }
            "{}" if used < arguments => {
                template.push_str(&format!("{{arg{used}}}"));
                used += 1;
            }
            _ => return None,
        }
        rest = tail;
    }
    // CPython can retain the template or a field owner and interns short results.
    // A field plus two literal characters always produces a fresh result.
    if used == 0 || literal_chars + rest.chars().count() < 2 {
        return None;
    }
    template.push_str(rest);
    Some(template)
}

impl Checker<'_> {
    pub(super) fn math_fmod(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = &call.arguments;
        if !args.keywords.is_empty() || args.args.len() != 2 {
            let message = if args.keywords.is_empty() {
                format!("fmod expected 2 arguments, got {}", args.args.len())
            } else {
                "math.fmod() takes no keyword arguments".to_owned()
            };
            return self.positional_argument_error(
                call,
                &["x", "y"],
                Type::Float,
                unpacking::string(message),
            );
        }
        let mut values = Vec::with_capacity(2);
        for arg in &args.args {
            let value = powers::integer(self.expr(arg)?);
            if !matches!(value.ty, Type::Int | Type::Float) {
                return Err(error(
                    self.source,
                    arg,
                    "math.fmod requires an integer or float",
                ));
            }
            values.push(value);
        }
        Ok(Expr {
            ty: Type::Float,
            kind: ExprKind::Builtin(Builtin::FloatFmod, values),
        })
    }

    /// Keep argument evaluation ahead of index conversion, including when k evaluates to None.
    pub(super) fn math_perm(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let args = &call.arguments;
        if !self
            .graph
            .session
            .is_stdlib_function(self.module, &call.func, "math", "perm")
        {
            return Err(error(
                self.source,
                call,
                "checked permutation requires a resolved math.perm call",
            ));
        }
        let message = if !args.keywords.is_empty() {
            Some("math.perm() takes no keyword arguments".into())
        } else {
            match args.args.len() {
                0 => Some("perm expected at least 1 argument, got 0".into()),
                1 | 2 => None,
                count => Some(format!("perm expected at most 2 arguments, got {count}")),
            }
        };
        self.graph.checked_overload(self.module, call.range());
        if let Some(message) = message {
            return self.positional_argument_error(
                call,
                &["n", "k"],
                Type::Int,
                unpacking::string(message),
            );
        }

        let mut scope = calls::ArgumentScope::default();
        let n_node = &args.args[0];
        let n = self.expr(n_node)?;
        let n = scope.hold(self, n);
        let k = if let Some(node) = args.args.get(1) {
            let value = self.expr(node)?;
            Some((node, scope.hold(self, value)))
        } else {
            None
        };
        let n = self.index_word(n, n_node, false)?;
        self.graph
            .checked_argument_type(self.module, n_node.range());
        let factorial = |n| Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::MathFactorial, vec![n]),
        };
        let perm = |n, k| Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::MathPerm, vec![n, k]),
        };
        let value = match k {
            None => factorial(n),
            Some((_, k)) if k.ty == Type::None => factorial(n),
            Some((node, k)) => {
                self.graph.checked_argument_type(self.module, node.range());
                if let Some(ty) = self.types.payload(k.ty) {
                    let n = scope.hold(self, n);
                    let absent = Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::IsNone, vec![k.clone()]),
                    };
                    let value = Expr {
                        ty,
                        kind: ExprKind::Unwrap(Box::new(k)),
                    };
                    let k = self.index_word(value, node, false)?;
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Conditional(
                            Box::new(absent),
                            Box::new(factorial(n.clone())),
                            Box::new(perm(n, k)),
                        ),
                    }
                } else {
                    let k = self.index_word(k, node, false)?;
                    perm(n, k)
                }
            }
        };
        Ok(scope.finish(value))
    }

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
        if callback.is_none() {
            // The surrounding check can continue after a refused sort.
            self.sortable_key(element, call)
                .inspect_err(|_| self.forget_pull_writes(&input))?;
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
            body.push(Statement::Assign(completed, Expr::boolean(false)));
            let mut sorting = Vec::new();
            self.sort_values(call, original, callback, reverse, &mut sorting, &mut clear)?;
            sorting.push(Statement::Assign(completed, Expr::boolean(true)));
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
            self.sortable_key(element, call)?;
            self.sort_values(call, target, callback, reverse, &mut body, &mut clear)?;
        }
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body,
                value: Box::new(Expr::none()),
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
        let mut reverse = Expr::boolean(false);
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
        } else if ty == Type::PythonObject || ty.is_path() {
            Err(self.scoped_builtin_error("comparing Python sort keys", node))
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
        let mut scope = calls::ArgumentScope::default();
        let (separator, parts) = match input {
            Iteration::Container {
                iterable,
                items: false,
                start: None,
            } if style == JoinStyle::Plain
                && matches!(
                    self.types.element(iterable.ty),
                    Some(Type::Str | Type::StrLiteral(_))
                ) =>
            {
                (receiver, iterable)
            }
            input => {
                // Pool roots live for the program. Hold any computed separator before pulling.
                let separator = if matches!(receiver.kind, ExprKind::Str(_)) {
                    receiver
                } else {
                    scope.hold(self, receiver)
                };
                let parts =
                    self.collect_list_with(input, Some(Type::Str), call, |checker, value| {
                        match style {
                            JoinStyle::Plain => Ok(value),
                            JoinStyle::Shell => checker.shlex_quote_value(value, call),
                            JoinStyle::WindowsCommand => {
                                let path = Self::string_value(checker.fspath(value, source)?);
                                Ok(Expr {
                                    ty: Type::Str,
                                    kind: ExprKind::Builtin(
                                        Builtin::Process(
                                            ir::ProcessOperation::QuoteCommandArgument,
                                        ),
                                        vec![path],
                                    ),
                                })
                            }
                        }
                    })?;
                (separator, parts)
            }
        };
        Ok(scope.finish(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::StringJoin, vec![separator, parts]),
        }))
    }

    /// Evaluate every argument before rendering any field, including unused
    /// arguments. Integer conversion stays in the runtime formatter.
    pub(super) fn string_format_call(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let mut args = vec![receiver];
        if !call.arguments.args.is_empty() {
            let ast::Expr::Attribute(method) = &*call.func else {
                unreachable!()
            };
            let template = match &*method.value {
                ast::Expr::StringLiteral(s) if call.arguments.keywords.is_empty() => {
                    positional_template(s.value.to_str(), call.arguments.args.len())
                }
                _ => None,
            };
            let Some(template) = template else {
                return Err(error(
                    self.source,
                    call,
                    "str.format requires named arguments",
                ));
            };
            args[0] = unpacking::string(&template);
            for (index, arg) in call.arguments.args.iter().enumerate() {
                let value = self.expr(arg)?;
                if !matches!(value.ty, Type::Int | Type::Str | Type::StrLiteral(_)) {
                    return Err(error(
                        self.source,
                        arg,
                        "positional str.format requires exact str or int",
                    ));
                }
                args.push(unpacking::string(format!("arg{index}")));
                args.push(Self::string_value(value));
            }
        }
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
        let Some((old, new, count)) = replace_arguments(call) else {
            return Err(self.object_builtin_error("str.replace", call));
        };
        let mut string_argument = |node: &ast::Expr| {
            if self.graph.session.explicit_check && fixed_replace_operand(call, node) {
                let value = self
                    .expr(node)
                    .map_err(|_| self.object_builtin_error("str.replace", call))?;
                self.coerce(value, Type::Str, node)
            } else {
                self.expr_expected(node, Type::Str)
                    .map_err(|_| self.object_builtin_error("str.replace", call))
            }
        };
        let old = string_argument(old)?;
        let new = string_argument(new)?;
        let mut args = vec![receiver, old, new];
        if let Some(count) = count {
            let count = powers::integer(self.expr(count)?);
            if count.ty != Type::Int {
                return Err(self.object_builtin_error("str.replace", call));
            }
            args.push(count);
        }
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::StringReplace, args),
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
                kind: ExprKind::Builtin(Builtin::Int(None), vec![value]),
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

    /// Evaluate the prompt before selecting stdout, then run its Python string conversion.
    pub(super) fn input_prompt(
        &mut self,
        prompt: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let mut scope = calls::ArgumentScope::default();
        let prompt = scope.hold(self, prompt);
        let output = scope.hold(
            self,
            Expr {
                ty: Type::TextIO,
                kind: ExprKind::Builtin(Builtin::InputSelectStdout, vec![]),
            },
        );
        let prompt = if matches!(prompt.ty, Type::StrLiteral(_)) {
            literals::view(prompt, Type::Str)
        } else {
            prompt
        };
        let text = self.checked_string(prompt, call)?;
        Ok(scope.finish(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::InputFinish, vec![output, text]),
        }))
    }

    /// Render one checked value with builtin str, independent of a local binding named str.
    pub(super) fn checked_string(&mut self, value: Expr, at: &impl Ranged) -> Result<Expr, String> {
        let ty = value.ty;
        if ty == Type::PythonObject {
            self.require_registered_operation(at)?;
            return Ok(super::fstrings::object_conversion(value, 's'));
        }
        if matches!(ty, Type::Calendar(_)) {
            return Ok(calendar::format_builtin("str", vec![value]));
        }
        if let Some(text) = self.patch_builtin("str", &value) {
            return Ok(text);
        }
        if self.types.payload(ty) == Some(Type::Str) {
            return Ok(self.optional_text(value));
        }
        if ty.is_path() || ty == Type::PathInfo {
            let op = if ty == Type::PathInfo {
                ir::PathOperation::InfoRepr
            } else {
                ir::PathOperation::Text
            };
            return Ok(paths::operation(op, Type::Str, vec![value]));
        }
        if ty == Type::LogRecord {
            return Ok(logging_records::op(
                ir::LogRecordOperation::Repr,
                Type::Str,
                vec![value],
            ));
        }
        if let Some(text) = self.mock_call_repr(&value, at)? {
            return Ok(text);
        }
        let op = match ty {
            Type::Enum(_) => Builtin::EnumString,
            Type::StrEnum(_) | Type::IntEnum(_) => Builtin::StrEnumValue,
            Type::BuiltinClass
            | Type::PlainObject
            | Type::StrDescriptor(_)
            | Type::Placeholder(_) => Builtin::Repr,
            Type::Int
            | Type::Float
            | Type::Bool
            | Type::Str
            | Type::Bytes
            | Type::None
            | Type::ObjectValue
            | Type::Exception
            | Type::NamedException(_)
            | Type::ImportException
            | Type::TimeoutExpired => Builtin::Str,
            _ => {
                return self.structured_display("str", &value, at)?.ok_or_else(|| {
                    error(
                        self.source,
                        at,
                        "unknown function or unsupported builtin arguments",
                    )
                });
            }
        };
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(op, vec![value]),
        })
    }

    /// Preserve the checked representation and field checks of lists, tuples and records.
    pub(super) fn structured_display(
        &mut self,
        name: &str,
        value: &Expr,
        at: &impl Ranged,
    ) -> Result<Option<Expr>, String> {
        let ty = value.ty;
        if self.types.element(ty) == Some(Type::ObjectValue) {
            return Ok(Some(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Repr, vec![value.clone()]),
            }));
        }
        if !dataclass_values::repr_supported(ty, self.types) {
            return Ok(None);
        }
        let list = self.types.element(ty).is_some();
        if name == "repr" || (name == "str" && list) {
            if !dataclass_values::can_format(ty, self.types, self.classes) {
                return Err(error(
                    self.source,
                    at,
                    "generated repr cannot format resource or callable fields",
                ));
            }
            return Ok(Some(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Repr, vec![value.clone()]),
            }));
        }
        if matches!(
            ty,
            Type::Class(_)
                | Type::Hash
                | Type::SysImplementation
                | Type::CompletedProcess(..)
                | Type::Context
                | Type::ContextMissing
                | Type::ContextVar(_)
                | Type::ContextToken(_)
                | Type::ForwardRef(_)
                | Type::ModuleSpec(_)
                | Type::SimpleNamespace(_)
                | Type::Dict(_)
                | Type::Tuple(_)
                | Type::VarTuple(_)
                | Type::StringIterable
        ) || self.types.payload(ty) == Some(Type::Int)
            || (name == "print" && list)
        {
            return self.record_text(value.clone(), at).map(Some);
        }
        Ok(None)
    }

    /// Decimal integer fields keep their full precision. Fixed-point and
    /// percentage fields use Python's checked integer-to-float conversion.
    pub(super) fn prepare_field(
        &mut self,
        field: &ast::InterpolatedElement,
        conversion: ast::ConversionFlag,
    ) -> Result<fstrings::FieldValue, String> {
        use fstrings::FieldValue::{Formatted, Operand};
        let operand = |checker: &mut Self| checker.expr(&field.expression).map(Operand);
        let mut specification = String::new();
        if let Some(spec) = &field.format_spec {
            for part in &spec.elements {
                let ast::InterpolatedStringElement::Literal(literal) = part else {
                    return operand(self);
                };
                specification.push_str(&literal.value);
            }
        }
        if conversion == ast::ConversionFlag::Ascii {
            return if specification.is_empty() {
                self.ascii_expr(&field.expression, None).map(Formatted)
            } else {
                operand(self)
            };
        }
        let format = if specification.is_empty() {
            None
        } else if let Some(numeric) = NumericField::parse(&specification) {
            Some(numeric)
        } else {
            return operand(self);
        };
        if format.is_none()
            && let Some(text) = self.unittest_failure_repr(&field.expression)?
        {
            return Ok(Formatted(text));
        }
        let value = self.expr(&field.expression)?;
        if value.ty == Type::Str
            && conversion == ast::ConversionFlag::None
            && field.format_spec.is_none()
        {
            let formatted = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::FormatField(Default::default()), vec![value]),
            };
            if let Some(sites) = &mut self.fixed_sites {
                sites.checked_expression(field.range(), &formatted);
                self.frame_arguments = FrameArguments::Python;
            }
            return Ok(Formatted(formatted));
        }
        if value.ty == Type::PythonObject {
            self.require_registered_operation(field)?;
            return Ok(Operand(value));
        }
        self.scalar_field(field, conversion, format, value)
            .map(Formatted)
    }

    fn scalar_field(
        &mut self,
        field: &ast::InterpolatedElement,
        conversion: ast::ConversionFlag,
        format: Option<NumericField<'_>>,
        mut value: Expr,
    ) -> Result<Expr, String> {
        if conversion != ast::ConversionFlag::Repr {
            value = if matches!(value.ty, Type::Enum(_)) {
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::EnumString, vec![value]),
                }
            } else {
                Self::enum_text(value)
            };
        }
        if let Some(format) = format {
            match &format {
                NumericField::Decimal(_) | NumericField::Hex { .. }
                    if conversion != ast::ConversionFlag::None
                        || !matches!(value.ty, Type::Int | Type::Bool) =>
                {
                    let reason = match format {
                        NumericField::Decimal(_) => {
                            "decimal integer formats require an unconverted int or bool"
                        }
                        _ => "hexadecimal formats require an unconverted int or bool",
                    };
                    return Err(error(self.source, field, reason));
                }
                NumericField::Float { .. }
                    if conversion != ast::ConversionFlag::None
                        || !matches!(value.ty, Type::Int | Type::Float | Type::Bool) =>
                {
                    return Err(error(
                        self.source,
                        field,
                        "numeric format specifications require an unconverted int, float, or bool",
                    ));
                }
                _ => {}
            }
            let op = match format {
                NumericField::Decimal(sign) => return Ok(self.formatted_integer(value, sign)),
                NumericField::Hex { width, zero, upper } => {
                    if value.ty == Type::Bool {
                        value = Expr {
                            ty: Type::Int,
                            kind: ExprKind::Builtin(Builtin::Int(None), vec![value]),
                        };
                    }
                    Builtin::HexFormat { width, zero, upper }
                }
                NumericField::Float {
                    precision,
                    percent,
                    positive_sign,
                } => {
                    if value.ty != Type::Float {
                        value = Expr {
                            ty: Type::Float,
                            kind: ExprKind::Builtin(Builtin::Float(None), vec![value]),
                        };
                    }
                    Builtin::FloatFormat {
                        precision,
                        percent,
                        positive_sign,
                    }
                }
            };
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(op, vec![value]),
            });
        }
        if matches!(
            value.ty,
            Type::Exception | Type::ImportException | Type::NamedException(_)
        ) {
            let op = if conversion == ast::ConversionFlag::Repr {
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
            let name = if conversion == ast::ConversionFlag::Repr {
                "repr"
            } else {
                "str"
            };
            return Ok(calendar::format_builtin(name, vec![value]));
        }
        if conversion != ast::ConversionFlag::Repr
            && matches!(value.ty, Type::Class(_) | Type::CompletedProcess(..))
        {
            return self.record_text(value, field);
        }
        if conversion == ast::ConversionFlag::Repr && matches!(value.ty, Type::Class(_)) {
            return self.record_repr(value, field);
        }
        if conversion == ast::ConversionFlag::Repr && value.ty == Type::ObjectValue {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::ExceptionFieldRepr, vec![value]),
            });
        }
        if conversion == ast::ConversionFlag::Repr && matches!(value.ty, Type::Document(_)) {
            return Ok(documents::op(ir::DocumentOp::Repr, Type::Str, vec![value]));
        }
        if conversion == ast::ConversionFlag::Repr
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
                kind: ExprKind::Builtin(Builtin::Int(None), vec![value]),
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
            pipelines::compare(Compare::GreaterEqual, local.clone(), Expr::integer(0));
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

enum NumericField<'a> {
    Decimal(&'a str),
    Hex {
        width: u32,
        zero: bool,
        upper: bool,
    },
    Float {
        precision: u32,
        percent: bool,
        positive_sign: u8,
    },
}

impl<'a> NumericField<'a> {
    fn parse(specification: &'a str) -> Option<Self> {
        if let Some(digits) = specification.strip_suffix(['x', 'X'])
            && digits.bytes().all(|b| b.is_ascii_digit())
            && let Some(width) = if digits.is_empty() {
                Some(0)
            } else {
                digits.parse().ok()
            }
            && width <= 1000
        {
            return Some(Self::Hex {
                width,
                zero: digits.starts_with('0'),
                upper: specification.ends_with('X'),
            });
        }
        let sign = specification.strip_suffix('d').unwrap_or(specification);
        if matches!(sign, "" | "-" | "+" | " ") {
            return Some(Self::Decimal(sign));
        }
        let (positive_sign, unsigned) = match specification.as_bytes().first() {
            Some(&sign @ (b'+' | b' ')) => (sign, &specification[1..]),
            Some(b'-') => (0, &specification[1..]),
            _ => (0, specification),
        };
        let (precision, percent) = numeric_format(unsigned)?;
        Some(Self::Float {
            precision,
            percent,
            positive_sign,
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
