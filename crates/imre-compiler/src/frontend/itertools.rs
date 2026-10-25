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
                self.cursor_index(expression)?
            });
        }
        let object =
            positions[0].ok_or_else(|| error(self.source, call, "repeat requires an object"))?;
        let ty = self.types.cursor(args.values[object].ty);
        args.order = positions.into_iter().flatten().collect();
        Ok(self.ordered_builtin(Builtin::Repeat, ty, args))
    }

    fn cursor_source(&mut self, node: &ast::Expr) -> Result<(Expr, Type), String> {
        if let ast::Expr::Call(call) = node
            && matches!(&*call.func, ast::Expr::Name(name) if name.id == "range")
            && self.resolve(&call.func).is_none()
            && !self.names.contains_key("range")
        {
            self.reject_builtin_keywords(call)?;
            let args = call
                .arguments
                .args
                .iter()
                .map(|node| self.cursor_index(node))
                .collect::<Result<Vec<_>, _>>()?;
            let args = match args.as_slice() {
                [stop] => vec![pipelines::integer(0), stop.clone(), pipelines::integer(1)],
                [start, stop] => vec![start.clone(), stop.clone(), pipelines::integer(1)],
                [_, _, _] => args,
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "range takes one to three integers",
                    ));
                }
            };
            return Ok((
                Expr {
                    ty: self.types.cursor(Type::Int),
                    kind: ExprKind::Builtin(Builtin::Progression { bounded: true }, args),
                },
                Type::Int,
            ));
        }
        let source = self.expr(node)?;
        let element = match source.ty {
            Type::Str => Some(Type::Str),
            Type::Bytes => Some(Type::Int),
            Type::Tuple(_) => {
                let fields = self.types.fields(source.ty).unwrap();
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
        .ok_or_else(|| {
            error(
                self.source,
                node,
                "iterator source requires one concrete element type",
            )
        })?;
        Ok((source, element))
    }

    pub(super) fn count_cursor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = [("start", 0), ("step", 1)].map(|(name, value)| Parameter {
            name: name.to_owned(),
            ty: Type::Int,
            kind: ParameterKind::PositionalOrKeyword,
            default: Some(pipelines::integer(value)),
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
                        match value.ty {
                            Type::Int | Type::None => value,
                            Type::Bool => Expr {
                                ty: Type::Int,
                                kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                            },
                            Type::Str | Type::Bytes | Type::Float => Expr {
                                ty: Type::Int,
                                kind: ExprKind::Scope {
                                    body: vec![Statement::Expr(value)],
                                    value: Box::new(pipelines::integer(-1)),
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
                    None => Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    },
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
            self.cursor_index(default)?
        } else {
            pipelines::integer(0)
        };
        Ok(Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::CursorLengthHint, vec![source, default]),
        })
    }

    fn cursor_index(&mut self, node: &ast::Expr) -> Result<Expr, String> {
        let value = self.expr(node)?;
        if value.ty == Type::Bool {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Int, vec![value]),
            });
        }
        self.expect(Type::Int, value.ty, node)?;
        Ok(value)
    }

    pub(super) fn iter_cursor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let (cursor, _) = self.cursor_source(&call.arguments.args[0])?;
        if self.types.cursor_element(cursor.ty).is_none() {
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
        let element = self
            .types
            .cursor_element(source.ty)
            .ok_or_else(|| error(self.source, call, "next requires a supported iterator"))?;
        let iterator = self.hidden(source.ty);
        let item = self.hidden(element);
        let ready = self.hidden(Type::Bool);
        let mut body = vec![Statement::Assign(iterator, source)];
        let mut clear = vec![iterator, item, ready];
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
        body.push(Statement::IterNext {
            iterator,
            key: item,
            value: None,
            ready,
        });
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
                vec![Statement::Raise(Some(Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind("StopIteration").unwrap(),
                        message: None,
                    },
                }))],
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
