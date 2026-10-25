//! Native argument binding and context cleanup for unittest.mock.patch.dict.

use super::*;

impl Checker<'_> {
    /// Bind values and keyword updates in caller order. A literal iterable of
    /// pairs becomes an ordinary dictionary with normal last-key-wins behavior.
    pub(super) fn mock_dictionary_call(
        &mut self,
        call: &ast::ExprCall,
        class: usize,
        receiver: Expr,
    ) -> Result<Expr, String> {
        let sig = self.classes.methods[class]["_dictionary"].clone();
        let dict = sig.params[1].ty;
        let optional = sig.params[2].ty;
        let root = self.hidden(receiver.ty);
        let mut body = vec![Statement::Assign(root, receiver)];
        let mut clear = vec![root];
        let mut slots = [None; 3];
        let mut extras = Vec::new();
        if call.arguments.args.len() > 3 {
            return Err(error(
                self.source,
                call,
                "patch.dict takes at most three positional arguments",
            ));
        }
        let mut arguments = call
            .arguments
            .args
            .iter()
            .enumerate()
            .map(|(index, arg)| (Some(index), None, arg))
            .collect::<Vec<_>>();
        for kw in &call.arguments.keywords {
            let Some(name) = kw.arg.as_deref() else {
                return Err(error(
                    self.source,
                    kw,
                    "patch.dict requires named keyword updates",
                ));
            };
            let index = match name {
                "in_dict" => Some(0),
                "values" => Some(1),
                "clear" => Some(2),
                _ => None,
            };
            arguments.push((index, Some(name), &kw.value));
        }
        for (index, name, arg) in arguments {
            let value = match index {
                Some(0) => self.expr_expected(arg, dict)?,
                Some(1) if matches!(arg, ast::Expr::List(_) | ast::Expr::Tuple(_)) => {
                    let items = match arg {
                        ast::Expr::List(list) => &list.elts,
                        ast::Expr::Tuple(tuple) => &tuple.elts,
                        _ => unreachable!(),
                    };
                    let mut entries = Vec::new();
                    for item in items {
                        let pair = match item {
                            ast::Expr::List(list) => &list.elts,
                            ast::Expr::Tuple(tuple) => &tuple.elts,
                            _ => {
                                return Err(error(
                                    self.source,
                                    item,
                                    "patch.dict needs a literal pair or a native mapping",
                                ));
                            }
                        };
                        let [key, value] = pair.as_slice() else {
                            return Err(error(
                                self.source,
                                item,
                                "patch.dict update requires pairs of length two",
                            ));
                        };
                        entries.push((
                            self.expr_expected(key, Type::Str)?,
                            self.expr_expected(value, Type::Str)?,
                        ));
                    }
                    self.coerce(
                        Expr {
                            ty: dict,
                            kind: ExprKind::Dict {
                                key: Type::Str,
                                value: Type::Str,
                                entries,
                            },
                        },
                        optional,
                        arg,
                    )?
                }
                Some(1) => self.expr_expected(arg, optional)?,
                Some(2) => self.expr_expected(arg, Type::Bool)?,
                None => self.expr_expected(arg, Type::Str)?,
                _ => unreachable!(),
            };
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
            if let Some(index) = index {
                if slots[index].replace(slot).is_some() {
                    return Err(error(self.source, arg, "duplicate patch.dict argument"));
                }
            } else {
                extras.push((unpacking::string(name.unwrap()), self.local(slot)));
            }
        }
        let target =
            slots[0].ok_or_else(|| error(self.source, call, "patch.dict requires a mapping"))?;
        let values = if let Some(slot) = slots[1] {
            self.local(slot)
        } else {
            self.coerce(
                Expr {
                    ty: dict,
                    kind: ExprKind::Dict {
                        key: Type::Str,
                        value: Type::Str,
                        entries: Vec::new(),
                    },
                },
                optional,
                call,
            )?
        };
        let reset = slots[2].map_or(
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(false),
            },
            |slot| self.local(slot),
        );
        let kwargs = Expr {
            ty: dict,
            kind: ExprKind::Dict {
                key: Type::Str,
                value: Type::Str,
                entries: extras,
            },
        };
        self.calls.insert(sig.id);
        let result = Expr {
            ty: sig.returns,
            kind: ExprKind::Call(
                sig.id,
                ir::CallArgs::positional(vec![
                    self.local(root),
                    self.local(target),
                    values,
                    reset,
                    kwargs,
                ]),
            ),
        };
        Ok(Expr {
            ty: result.ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(result),
                clear,
            },
        })
    }

    /// A dictionary patch returns the original mapping itself, and restores its
    /// entire snapshot, including keys inserted or deleted by the protected body.
    pub(super) fn mock_dictionary_scope(
        &mut self,
        item: &ast::WithItem,
        remaining: &[ast::WithItem],
        suite: &[ast::Stmt],
        class: usize,
        manager: Expr,
    ) -> Result<(Vec<Statement>, bool), String> {
        let saved = self.hidden(manager.ty);
        let (enter_id, enter_ty) = {
            let sig = &self.classes.methods[class]["_enter"];
            (sig.id, sig.returns)
        };
        let exit = self.classes.methods[class]["_exit"].id;
        self.calls.extend([enter_id, exit]);
        let call = |id, ty| Expr {
            ty,
            kind: ExprKind::Call(id, ir::CallArgs::positional(vec![self.local(saved)])),
        };
        let value = call(enter_id, enter_ty);
        let restore = Statement::Expr(call(exit, Type::Bool));
        let entered = self.hidden(enter_ty);
        let mut protected = Vec::new();
        if let Some(target) = &item.optional_vars {
            protected.push(self.assign(target, self.local(entered))?);
        }
        let (nested, falls) = self.with_items(remaining, suite)?;
        protected.extend(nested);
        let guard = Statement::Try {
            body: protected,
            handlers: Vec::new(),
            orelse: Vec::new(),
            finalbody: vec![restore],
        };
        Ok((
            vec![Statement::Scope {
                body: vec![
                    Statement::Assign(saved, manager),
                    Statement::Assign(entered, value),
                    guard,
                ],
                clear: vec![saved, entered],
            }],
            falls,
        ))
    }
}
