//! First-class itertools objects share the ordinary iteration and ownership ABI.

use super::*;

mod accumulate;
mod batched;
mod predicate;

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

    fn cursor_source_value(&mut self, node: &ast::Expr) -> Result<Expr, String> {
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
            return Ok(Expr {
                ty: self.types.cursor(Type::Int),
                kind: ExprKind::Builtin(Builtin::Progression { bounded: true }, args),
            });
        }
        self.expr(node)
    }

    fn iterable_element(&self, ty: Type) -> Option<Type> {
        match ty {
            Type::VarTuple(_) => self.types.var_tuple_element(ty),
            Type::Str | Type::TextIO | Type::StringIO => Some(Type::Str),
            Type::CsvReader => Some(self.types.list(Type::Str)),
            Type::Bytes => Some(Type::Int),
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

    fn cursor_source(&mut self, node: &ast::Expr) -> Result<(Expr, Type), String> {
        let source = self.cursor_source_value(node)?;
        let element = self.iterable_element(source.ty).ok_or_else(|| {
            error(
                self.source,
                node,
                "iterator source requires one concrete element type",
            )
        })?;
        Ok((source, element))
    }

    pub(super) fn is_chain_from_iterable(&self, call: &ast::ExprCall) -> bool {
        matches!(&*call.func, ast::Expr::Attribute(attr)
            if attr.attr.as_str() == "from_iterable"
                && self.resolve(&attr.value) == Some(Binding::Intrinsic(Intrinsic::Chain)))
    }

    pub(super) fn is_expanded_chain(&self, call: &ast::ExprCall) -> bool {
        self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::Chain))
            && call
                .arguments
                .args
                .iter()
                .any(|arg| matches!(arg, ast::Expr::Starred(_)))
    }

    pub(super) fn chain_cursor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let (op, input, element) =
            if self.is_chain_from_iterable(call) || self.is_expanded_chain(call) {
                let source = if self.is_chain_from_iterable(call) {
                    let [outer] = call.arguments.args.as_ref() else {
                        return Err(error(
                            self.source,
                            call,
                            "chain.from_iterable takes one iterable",
                        ));
                    };
                    self.cursor_source_value(outer)?
                } else {
                    // A starred call consumes its outer inputs at construction, but
                    // the resulting arguments still refer to their original objects.
                    self.expanded_sequence(&call.arguments.args, None, false, call)?
                };
                let inner = self.chain_element(source.ty, call)?.unwrap_or(Type::Int);
                (
                    Builtin::ChainFrom(inner),
                    source,
                    self.chain_element(inner, call)?,
                )
            } else {
                let mut values = Vec::new();
                let mut element = None;
                for arg in &call.arguments.args {
                    let value = self.cursor_source_value(arg)?;
                    if let Some(ty) = self.chain_element(value.ty, arg)? {
                        if let Some(previous) = element {
                            self.expect(previous, ty, arg)?;
                        }
                        element = Some(ty);
                    }
                    values.push(value);
                }
                let ty = self.types.tuple(values.iter().map(|e| e.ty).collect());
                (
                    Builtin::Chain,
                    Expr {
                        ty,
                        kind: ExprKind::Tuple(values),
                    },
                    element,
                )
            };
        Ok(Expr {
            ty: self.types.cursor(element.unwrap_or(Type::Int)),
            kind: ExprKind::Builtin(op, vec![input]),
        })
    }

    fn chain_element(&self, ty: Type, node: &impl Ranged) -> Result<Option<Type>, String> {
        // Iterating an exact scalar raises only when chain reaches that input.
        if matches!(ty, Type::None | Type::Int | Type::Bool | Type::Float)
            || matches!(ty, Type::Tuple(_) if self.types.fields(ty).unwrap().is_empty())
        {
            return Ok(None);
        }
        self.iterable_element(ty).map(Some).ok_or_else(|| {
            error(
                self.source,
                node,
                "chain input requires one concrete element type",
            )
        })
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

    pub(super) fn compress_cursor(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = ["data", "selectors"].map(|name| Parameter {
            name: name.to_owned(),
            ty: Type::None,
            kind: ParameterKind::PositionalOrKeyword,
            default: None,
            sign_only: false,
        });
        let mut items = [Type::Int; 2];
        let args = self.arguments_with(call, &params, None, |checker, i, node| {
            let source = checker.cursor_source_value(node)?;
            items[i] = checker
                .iterable_element(source.ty)
                .or_else(|| {
                    matches!(source.ty, Type::None | Type::Int | Type::Bool | Type::Float)
                        .then_some(Type::Int)
                })
                .ok_or_else(|| {
                    error(
                        checker.source,
                        node,
                        "compress source requires one concrete element type",
                    )
                })?;
            Ok(source)
        })?;
        let selector = self.types.payload(items[1]).unwrap_or(items[1]);
        if !matches!(
            selector,
            Type::None
                | Type::Bool
                | Type::Int
                | Type::Float
                | Type::Str
                | Type::StrLiteral(_)
                | Type::Bytes
                | Type::ListInt
                | Type::List(_)
                | Type::Tuple(_)
                | Type::VarTuple(_)
                | Type::Dict(_)
                | Type::Set(_)
                | Type::TextIO
                | Type::StringIO
                | Type::CsvReader
                | Type::Cursor(_)
                | Type::Union(_)
        ) {
            return Err(error(
                self.source,
                call,
                "compress selector needs a supported truth test",
            ));
        }
        Ok(self.ordered_builtin(Builtin::Compress, self.types.cursor(items[0]), args))
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
        } else if source.ty == Type::StringIO {
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
            if rest.is_empty() {
                return Ok(Expr {
                    ty: element,
                    kind: ExprKind::Builtin(Builtin::CursorNextRequired, vec![source]),
                });
            }
            self.prepare_cursor(source, element)
        };
        let item = prepared.value;
        let element = self.locals[item];
        let ready = prepared.ready;
        let mut body = prepared.evaluate;
        let mut clear = prepared.clear;
        // The default is evaluated before pulling, including on success.
        let [default] = rest else {
            unreachable!("next without a default is lowered above")
        };
        let default = self.expr(default)?;
        let slot = self.hidden(default.ty);
        body.push(Statement::Assign(slot, default));
        clear.push(slot);
        body.extend(prepared.acquire);
        body.extend(prepared.next);
        let default = self.local(slot);
        let ty = self.types.join(element, default.ty).ok_or_else(|| {
            error(
                self.source,
                call,
                "next item and default need a common type",
            )
        })?;
        let value = Expr {
            ty,
            kind: ExprKind::Conditional(
                Box::new(self.local(ready)),
                Box::new(self.coerce(self.local(item), ty, call)?),
                Box::new(self.coerce(default, ty, call)?),
            ),
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
