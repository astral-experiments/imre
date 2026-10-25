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
        let cursor = self.expr(&call.arguments.args[0])?;
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
        let prepared = if source.ty == Type::CsvReader {
            if rest.is_empty() {
                return Ok(Expr {
                    ty: self.types.list(Type::Str),
                    kind: ExprKind::Builtin(Builtin::CsvNext { required: true }, vec![source]),
                });
            }
            self.prepare_csv_reader(source)
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
