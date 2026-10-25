//! An integer-valued Counter keeps a concrete key representation. Construction
//! and update share the iterator consumer so each source item is applied in order.

use super::*;

impl Checker<'_> {
    pub(super) fn counter_math(
        &self,
        operator: ast::Operator,
        left: Expr,
        right: Expr,
        inplace: bool,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if !inplace || self.types.mapping(left.ty) != self.types.mapping(right.ty) {
            self.expect(left.ty, right.ty, node)?;
        }
        let op = match operator {
            ast::Operator::Add => ir::CounterOp::Add,
            ast::Operator::Sub => ir::CounterOp::Subtract,
            ast::Operator::BitOr => ir::CounterOp::Union,
            ast::Operator::BitAnd => ir::CounterOp::Intersection,
            _ => return Err(error(self.source, node, "unsupported Counter operator")),
        };
        Ok(Expr {
            ty: left.ty,
            kind: ExprKind::Builtin(Builtin::CounterMath { op, inplace }, vec![left, right]),
        })
    }

    fn counter_input(
        &mut self,
        source: &ast::Expr,
        hint: Option<Type>,
    ) -> Result<Iteration, String> {
        let empty_key = hint.unwrap_or(Type::Str);
        if matches!(source, ast::Expr::Dict(dict) if dict.items.is_empty()) {
            Ok(Iteration::Container {
                iterable: self.expr_expected(source, self.types.dict(empty_key, Type::Int))?,
                items: false,
                start: None,
            })
        } else {
            self.expansion_input(
                source,
                hint.or_else(|| {
                    matches!(source, ast::Expr::List(list) if list.elts.is_empty())
                        .then_some(empty_key)
                }),
            )
        }
    }

    fn prepare_counter_input(
        &mut self,
        mut input: Iteration,
        source: &ast::Expr,
        hint: Option<Type>,
    ) -> Result<(pipelines::Prepared, Type, bool), String> {
        let mut mapping_key = None;
        if let Iteration::Container {
            iterable, items, ..
        } = &mut input
        {
            if let Some((key, count)) = self.types.mapping(iterable.ty)
                && !*items
            {
                self.expect(Type::Int, count, source)?;
                mapping_key = Some(key);
                *items = true;
            }
            if matches!(iterable.ty, Type::StrLiteral(_)) {
                *iterable = Self::string_value(iterable.clone());
            }
        }
        let prepared = self.prepare_iteration(input, source, false)?;
        self.hint_empty_iteration(&prepared, Some(hint.unwrap_or(Type::Str)));
        let element = mapping_key.unwrap_or(self.locals[prepared.value]);
        if let Some(key) = hint {
            self.expect(key, element, source)?;
        }
        Ok((prepared, element, mapping_key.is_some()))
    }

    /// Evaluate the positional input and all keyword values before acquiring
    /// the iterator or mutating its destination. Generator inputs carry their
    /// own eager outer-iterator acquisition in the evaluation phase.
    pub(super) fn counter(
        &mut self,
        call: &ast::ExprCall,
        receiver: Option<Expr>,
        hint: Option<Type>,
    ) -> Result<Expr, String> {
        let hint = receiver
            .as_ref()
            .and_then(|v| self.types.mapping(v.ty))
            .map(|(key, _)| key)
            .or(hint);
        let (receiver, input) = match call.arguments.args.as_ref() {
            [source] => match (receiver, self.counter_input(source, hint)?) {
                (
                    Some(receiver),
                    Iteration::Container {
                        iterable,
                        items: false,
                        start: None,
                    },
                ) if call.arguments.keywords.is_empty()
                    && self.types.mapping(iterable.ty) == self.types.mapping(receiver.ty) =>
                {
                    return Ok(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::CounterUpdate, vec![receiver, iterable]),
                    });
                }
                (receiver, input) => (
                    receiver,
                    Some(self.prepare_counter_input(input, source, hint)?),
                ),
            },
            [] => (receiver, None),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "Counter takes at most one positional input",
                ));
            }
        };
        let key = input
            .as_ref()
            .map(|(_, element, _)| *element)
            .or(hint)
            .unwrap_or(Type::Str);
        if !matches!(key, Type::Str | Type::Int) {
            return Err(error(self.source, call, "Counter keys must be str or int"));
        }
        let ty = self.types.counter(key);
        let fresh = Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::CounterNew, Vec::new()),
        };
        let constructor = receiver.is_none();
        if constructor && input.is_none() && call.arguments.keywords.is_empty() {
            return Ok(fresh);
        }
        let target = self.hidden(ty);
        let mut clear = vec![target];
        let mut body = vec![Statement::Assign(target, receiver.unwrap_or(fresh))];
        let mut evaluate = Vec::new();
        let mut keywords = Vec::new();
        for keyword in &call.arguments.keywords {
            if key != Type::Str {
                return Err(error(
                    self.source,
                    keyword,
                    "Counter keyword counts require string keys",
                ));
            }
            let Some(name) = &keyword.arg else {
                return Err(error(
                    self.source,
                    keyword,
                    "Counter keyword unpacking is unsupported",
                ));
            };
            let slot = self.hidden(Type::Int);
            let count = self.expr_expected(&keyword.value, Type::Int)?;
            evaluate.push(Statement::Assign(slot, count));
            clear.push(slot);
            keywords.push((name.to_string(), slot));
        }
        if let Some((mut input, _, mapping)) = input {
            input.evaluate.extend(evaluate);
            body.extend(
                self.expand_items(Iteration::Prepared(input), call, |checker, item| {
                    let (element, delta) = if mapping {
                        let field = |index, ty| Expr {
                            ty,
                            kind: ExprKind::Field(Box::new(item.clone()), index),
                        };
                        (field(0, key), field(1, Type::Int))
                    } else {
                        (checker.coerce(item, key, call)?, pipelines::integer(1))
                    };
                    Ok(vec![Statement::AugmentItem(
                        checker.local(target),
                        element,
                        Binary::Add,
                        delta,
                    )])
                })?,
            );
        } else {
            body.extend(evaluate);
        }
        for (name, slot) in keywords {
            body.push(Statement::AugmentItem(
                self.local(target),
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str(name),
                },
                Binary::Add,
                self.local(slot),
            ));
        }
        let value = if constructor {
            self.local(target)
        } else {
            Expr {
                ty: Type::None,
                kind: ExprKind::None,
            }
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
