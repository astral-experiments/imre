//! First-class itertools objects share the ordinary iteration and ownership ABI.

use super::*;

impl Checker<'_> {
    pub(super) fn repeat_cursor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let mut args = ir::CallArgs::positional(Vec::new());
        let mut positions = [None; 2];
        let positional = call.arguments.args.iter().enumerate();
        let keywords = call.arguments.keywords.iter().map(|kw| {
            (
                match kw.arg.as_ref().map(|name| name.as_str()) {
                    Some("object") => 0,
                    Some("times") => 1,
                    _ => 2,
                },
                &kw.value,
            )
        });
        for (index, expression) in positional.chain(keywords) {
            if index >= positions.len() || positions[index].replace(args.values.len()).is_some() {
                return Err(error(
                    self.source,
                    call,
                    "repeat takes an object and optional times",
                ));
            }
            args.values.push(if index == 0 {
                self.expr(expression)?
            } else {
                self.integer_index(expression)?
            });
        }
        let object =
            positions[0].ok_or_else(|| error(self.source, call, "repeat requires an object"))?;
        let ty = self.types.cursor(args.values[object].ty);
        args.order = positions.into_iter().flatten().collect();
        Ok(self.ordered_builtin(Builtin::Repeat, ty, args))
    }

    /// Construct a stored range without materializing its elements.
    pub(super) fn range_value(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        // Keep the established error result accepted by iterator consumers. A binding
        // error raises before the constructed value could reach the outer operation.
        if let Some(failure) = self.range_argument_error(call, self.types.cursor(Type::Int))? {
            return Ok(failure);
        }
        let range = Expr {
            ty: Type::Range,
            kind: ExprKind::Builtin(Builtin::RecordFromFields, self.range_bounds(call)?),
        };
        if call.arguments.args.len() < 3 {
            return Ok(range);
        }
        let slot = self.hidden(Type::Range);
        Ok(Expr {
            ty: Type::Range,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(slot, range),
                    pipelines::check_range_step(self.range_field(slot, 2)),
                ],
                value: Box::new(self.local(slot)),
                clear: vec![slot],
            },
        })
    }

    fn range_field(&self, slot: usize, index: usize) -> Expr {
        Expr {
            ty: Type::Int,
            kind: ExprKind::Field(Box::new(self.local(slot)), index),
        }
    }

    fn range_bounds(&mut self, call: &ast::ExprCall) -> Result<Vec<Expr>, String> {
        self.reject_builtin_keywords(call)?;
        let args = call
            .arguments
            .args
            .iter()
            .map(|node| self.integer_index(node))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        match args.as_slice() {
            [stop] => Ok(vec![Expr::integer(0), stop.clone(), Expr::integer(1)]),
            [start, stop] => Ok(vec![start.clone(), stop.clone(), Expr::integer(1)]),
            [_, _, _] => Ok(args),
            _ => Err(error(
                self.source,
                call,
                "range takes one to three integers",
            )),
        }
    }

    /// Snapshot an iterable once before acquiring its independent, bounded cursor.
    pub(super) fn range_cursor(&mut self, value: Expr) -> Expr {
        let slot = self.hidden(Type::Range);
        let fields = (0..3).map(|i| self.range_field(slot, i)).collect();
        let ty = self.types.cursor(Type::Int);
        Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, value)],
                clear: vec![slot],
                value: Box::new(Expr {
                    ty,
                    kind: ExprKind::Builtin(Builtin::Progression { bounded: true }, fields),
                }),
            },
        }
    }

    pub(super) fn cursor_source_value(&mut self, node: &ast::Expr) -> Result<Expr, String> {
        if let ast::Expr::Call(call) = node
            && self.unshadowed_builtin(&call.func, "range")
        {
            if let Some(failure) = self.range_argument_error(call, self.types.cursor(Type::Int))? {
                return Ok(failure);
            }
            let args = self.range_bounds(call)?;
            return Ok(Expr {
                ty: self.types.cursor(Type::Int),
                kind: ExprKind::Builtin(Builtin::Progression { bounded: true }, args),
            });
        }
        let value = self.expr(node)?;
        Ok(if value.ty == Type::Range {
            self.range_cursor(value)
        } else {
            value
        })
    }

    fn cursor_source(&mut self, node: &ast::Expr) -> Result<(Expr, Type), String> {
        let source = self.cursor_source_value(node)?;
        let element = self.cursor_source_element(source.ty).ok_or_else(|| {
            error(
                self.source,
                node,
                "iterator source requires one concrete element type",
            )
        })?;
        Ok((source, element))
    }

    pub(super) fn cursor_source_element(&self, ty: Type) -> Option<Type> {
        match ty {
            Type::Str | Type::StrLiteral(_) => Some(Type::Str),
            Type::Bytes => Some(Type::Int),
            Type::VarTuple(_) => self.types.var_tuple_element(ty),
            Type::Tuple(_) => {
                let fields = self.types.fields(ty).unwrap();
                let first = fields.first().copied().unwrap_or(Type::Int);
                fields.iter().all(|&t| t == first).then_some(first)
            }
            ty => self
                .types
                .cursor_element(ty)
                .or_else(|| self.types.element(ty))
                .or_else(|| self.types.set_element(ty))
                .or_else(|| self.types.mapping(ty).map(|(key, _)| key)),
        }
    }

    /// Convert a concrete iterable, reusing exact tuples as required by tuple().
    pub(super) fn iterable_tuple(&self, source: &Expr) -> Option<Expr> {
        if matches!(source.ty, Type::Tuple(_) | Type::VarTuple(_)) {
            return self
                .types
                .tuple_origin(source.ty)
                .is_none()
                .then(|| source.clone());
        }
        let element = self.cursor_source_element(source.ty)?;
        Some(Expr {
            ty: self.types.var_tuple(element),
            kind: ExprKind::Builtin(Builtin::IterableTuple, vec![source.clone()]),
        })
    }

    /// Constructors consume each input once, but return an independently pullable cursor.
    pub(super) fn combinatorial_cursor(
        &mut self,
        call: &ast::ExprCall,
        op: ir::Combinatorial,
    ) -> Result<Expr, String> {
        use ir::Combinatorial::{Permutations, Product};

        let mut elements = Vec::new();
        let args = if op == Product {
            let mut values = Vec::new();
            for source in &call.arguments.args {
                let (value, element) = self.cursor_source(source)?;
                elements.push(element);
                values.push(value);
            }
            let repeat = match call.arguments.keywords.as_ref() {
                [] => Expr::integer(1),
                [kw] if kw
                    .arg
                    .as_ref()
                    .is_some_and(|name| name.as_str() == "repeat") =>
                {
                    self.integer_index(&kw.value)?
                }
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "product accepts only the repeat keyword",
                    ));
                }
            };
            values.push(repeat);
            ir::CallArgs::positional(values)
        } else {
            let none = Expr::none();
            let params = [
                ("iterable", None),
                ("r", (op == Permutations).then_some(none)),
            ]
            .map(|(name, default)| Parameter {
                name: name.into(),
                ty: Type::Int,
                kind: ParameterKind::PositionalOrKeyword,
                default,
                sign_only: false,
            });
            self.arguments_with(call, &params, None, |checker, index, node| {
                if index == 0 {
                    let (value, element) = checker.cursor_source(node)?;
                    elements.push(element);
                    Ok(value)
                } else {
                    let value = checker.expr(node)?;
                    if op == Permutations
                        && (value.ty == Type::None
                            || checker.types.payload(value.ty) == Some(Type::Int))
                    {
                        Ok(value)
                    } else {
                        checker.index_value(value, node)
                    }
                }
            })?
        };
        let parameter = &args.values[*args.order.last().unwrap()];
        // Retain fixed field types for destructuring. Large or runtime arities
        // use homogeneous tuple storage, without expanding their shape here.
        let size = match &parameter.kind {
            ExprKind::Int(value) => value.parse::<usize>().ok(),
            _ => None,
        };
        let row = if elements.is_empty() {
            self.types.tuple(vec![])
        } else if let Some(size) = size
            && size
                .checked_mul(elements.len())
                .is_some_and(|len| len <= 256)
        {
            self.types.tuple(elements.repeat(size))
        } else if elements.iter().all(|&element| element == elements[0]) {
            self.types.var_tuple(elements[0])
        } else {
            return Err(error(
                self.source,
                call,
                "product of different element types requires a fixed result shape",
            ));
        };
        Ok(self.ordered_builtin(Builtin::Combinatorial(op), self.types.cursor(row), args))
    }

    pub(super) fn count_cursor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = [("start", 0), ("step", 1)].map(|(name, value)| Parameter {
            name: name.to_owned(),
            ty: Type::Int,
            kind: ParameterKind::PositionalOrKeyword,
            default: Some(Expr::integer(value)),
            sign_only: false,
        });
        let args = self.arguments_with(call, &params, None, |checker, _, node| {
            let value = checker.expr(node)?;
            if matches!(value.ty, Type::Float | Type::Int) {
                Ok(value)
            } else {
                Err(error(
                    checker.source,
                    node,
                    "count requires exact int or float arguments",
                ))
            }
        })?;
        let start = args.values[args.order[0]].ty;
        let step = args.values[args.order[1]].ty;
        if start == Type::Int && step == Type::Float {
            return Err(error(
                self.source,
                call,
                "count(int, float) changes its yielded type after the first pull",
            ));
        }
        Ok(self.ordered_builtin(
            Builtin::Progression { bounded: false },
            self.types.cursor(start),
            args,
        ))
    }

    pub(super) fn tee_cursor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let (source, n) = match call.arguments.args.as_ref() {
            [source] => (source, 2),
            [source, n] => {
                let n = if let ast::Expr::NumberLiteral(number) = n
                    && let ast::Number::Int(n) = &number.value
                {
                    n.to_string().parse::<usize>().ok()
                } else {
                    None
                };
                (
                    source,
                    n.filter(|&n| n <= 256).ok_or_else(|| {
                        error(
                            self.source,
                            call,
                            "tee needs a nonnegative fixed tuple size of at most 256",
                        )
                    })?,
                )
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "tee takes an iterable and optional fork count",
                ));
            }
        };
        if n == 0 {
            // Do evaluate the supplied expression, but do not call iter on it.
            // range is represented by a private cursor even in this discarded position.
            let value = if matches!(source, ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Name(name) if name.id == "range")
                && self.resolve(&call.func).is_none() && !self.names.contains_key("range"))
            {
                self.cursor_source(source)?.0
            } else {
                self.expr(source)?
            };
            let ty = self.types.tuple(Vec::new());
            return Ok(Expr {
                ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(value)],
                    value: Box::new(Expr {
                        ty,
                        kind: ExprKind::Tuple(Vec::new()),
                    }),
                    clear: Vec::new(),
                },
            });
        }
        let (source, element) = self.cursor_source(source)?;
        let forks = self.types.tuple(vec![self.types.cursor(element); n]);
        Ok(Expr {
            ty: forks,
            kind: ExprKind::Builtin(Builtin::Tee, vec![source]),
        })
    }

    pub(super) fn source_adapter(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [source, bounds @ ..] = call.arguments.args.as_ref() else {
            return Err(error(self.source, call, "missing iterator source"));
        };
        let (source, element) = self.cursor_source(source)?;
        let mut args = vec![source];
        let op = if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::Cycle)) {
            if !bounds.is_empty() {
                return Err(error(self.source, call, "cycle takes one iterable"));
            }
            Builtin::Cycle
        } else {
            let (start, stop, step) = match bounds {
                [stop] => (None, Some(stop), None),
                [start, stop] => (Some(start), Some(stop), None),
                [start, stop, step] => (Some(start), Some(stop), Some(step)),
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "islice takes two to four arguments",
                    ));
                }
            };
            for bound in [start, stop, step] {
                let value = match bound {
                    Some(node) => {
                        let value = self.expr(node)?;
                        let value = self.optional_scalar_value(value, Type::Int, node)?;
                        match value.ty {
                            Type::Int | Type::None => value,
                            ty if self.types.payload(ty) == Some(Type::Int) => value,
                            Type::Str | Type::Bytes | Type::Float => Expr {
                                ty: Type::Int,
                                kind: ExprKind::Scope {
                                    body: vec![Statement::Expr(value)],
                                    value: Box::new(Expr::integer(-1)),
                                    clear: Vec::new(),
                                },
                            },
                            _ => {
                                return Err(error(
                                    self.source,
                                    node,
                                    "islice bounds require a supported index",
                                ));
                            }
                        }
                    }
                    None => Expr::none(),
                };
                args.push(value);
            }
            Builtin::Islice
        };
        Ok(Expr {
            ty: self.types.cursor(element),
            kind: ExprKind::Builtin(op, args),
        })
    }

    pub(super) fn cursor_length_hint(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let (source, default) = match call.arguments.args.as_ref() {
            [source] => (source, None),
            [source, default] => (source, Some(default)),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "length_hint requires an object and an optional integer default",
                ));
            }
        };
        let source = self.expr(source)?;
        if !matches!(source.ty, Type::Cursor(_)) {
            return Err(error(
                self.source,
                call,
                "length_hint requires a supported iterator",
            ));
        }
        let default = if let Some(default) = default {
            self.integer_index(default)?
        } else {
            Expr::integer(0)
        };
        Ok(Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::CursorLengthHint, vec![source, default]),
        })
    }

    pub(super) fn integer_index(&mut self, node: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(node)?;
        self.index_value(value, node)
    }

    pub(super) fn index_value(&self, value: Expr, node: &impl Ranged) -> Result<Expr, String> {
        let value = Self::enum_integer(value);
        if value.ty == Type::Bool {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Int(None), vec![value]),
            });
        }
        self.expect(Type::Int, value.ty, node)?;
        Ok(value)
    }

    pub(super) fn iter_cursor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let cursor = self.cursor_source_value(&call.arguments.args[0])?;
        if cursor.ty == Type::StringIO {
            return Ok(self.open_string_io(cursor));
        }
        if cursor.ty != Type::CsvReader && self.types.cursor_element(cursor.ty).is_none() {
            return Err(error(
                self.source,
                call,
                "iter requires a supported iterator",
            ));
        }
        Ok(cursor)
    }

    pub(super) fn next_cursor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [source, rest @ ..] = call.arguments.args.as_ref() else {
            return Err(error(self.source, call, "next requires an iterator"));
        };
        if rest.len() > 1 {
            return Err(error(self.source, call, "next takes at most one default"));
        }
        let source = self.expr(source)?;
        if source.ty == Type::PythonObject {
            let mut args = vec![source];
            if let [default] = rest {
                args.push(self.expr(default)?);
            }
            return Ok(self.registered_builtin_call(call, "next", args));
        }
        self.next_value(source, rest, call)
    }

    pub(super) fn next_value(
        &mut self,
        source: Expr,
        rest: &[ast::Expr],
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let prepared = if source.ty == Type::CsvReader {
            if rest.is_empty() {
                return Ok(Expr {
                    ty: self.types.list(Type::Str),
                    kind: ExprKind::Builtin(Builtin::CsvNext { required: true }, vec![source]),
                });
            }
            self.prepare_csv_reader(source)
        } else if matches!(
            source.ty,
            Type::TextIO | Type::StringIO | Type::BinaryIO | Type::BytesIO
        ) {
            let mut lines = self.prepare_file_lines(source);
            // next(stream) pulls directly. It does not invoke iter(stream)'s
            // separate closed-file check before readline.
            lines.acquire.clear();
            lines
        } else {
            let element = self
                .types
                .cursor_element(source.ty)
                .ok_or_else(|| error(self.source, call, "next requires a supported iterator"))?;
            self.prepare_cursor(source, element)
        };
        let item = prepared.value;
        let element = self.locals[item];
        let ready = prepared.ready;
        let mut body = prepared.evaluate;
        let mut clear = prepared.clear;
        // The default is evaluated before pulling, including on success.
        let default = if let [default] = rest {
            let default = self.expr(default)?;
            let slot = self.hidden(default.ty);
            body.push(Statement::Assign(slot, default));
            clear.push(slot);
            Some(self.local(slot))
        } else {
            None
        };
        body.extend(prepared.acquire);
        body.extend(prepared.next);
        let value = if let Some(default) = default {
            let ty = self.types.join(element, default.ty).ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "next item and default need a common type",
                )
            })?;
            Expr {
                ty,
                kind: ExprKind::Conditional(
                    Box::new(self.local(ready)),
                    Box::new(self.coerce(self.local(item), ty, call)?),
                    Box::new(self.coerce(default, ty, call)?),
                ),
            }
        } else {
            body.push(Statement::If(
                self.local(ready),
                Vec::new(),
                vec![Statement::raise(Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind("StopIteration").unwrap(),
                        message: None,
                    },
                })],
            ));
            self.local(item)
        };
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(value),
                clear,
            },
        })
    }
}
