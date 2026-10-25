//! An integer-valued Counter keeps a concrete key representation. Construction
//! and updates share the iterator consumer so each source item is applied in order.

use super::*;

/// Counter ordering compares counts for the union of keys even when the two
/// key types differ. Other operator families retain their ordinary type checks.
pub(super) fn comparison(
    types: &Types,
    left: Type,
    right: Type,
    op: ast::CmpOp,
) -> Option<Compare> {
    if !(types.is_counter(left) && types.is_counter(right)) {
        return None;
    }
    Some(match op {
        ast::CmpOp::Eq => Compare::Equal,
        ast::CmpOp::NotEq => Compare::NotEqual,
        ast::CmpOp::Lt => Compare::Less,
        ast::CmpOp::LtE => Compare::LessEqual,
        ast::CmpOp::Gt => Compare::Greater,
        ast::CmpOp::GtE => Compare::GreaterEqual,
        _ => return None,
    })
}

impl Checker<'_> {
    /// Capture the integer mapping now; repetition and count conversion belong
    /// to the returned iterator's pulls.
    pub(super) fn counter_elements(
        &mut self,
        call: &ast::ExprCall,
        receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let mut values: Vec<_> = receiver.into_iter().collect();
        let first_extra = 1 - values.len();
        for (index, arg) in call.arguments.args.iter().enumerate() {
            values.push(self.expr(arg)?);
            if index >= first_extra {
                self.graph.checked_excess_argument(self.module, arg.range());
            }
        }
        let positional = values.len();
        let mut keyword_self = None;
        let mut failure = None;
        for keyword in &call.arguments.keywords {
            let name = keyword.arg.as_ref().unwrap().as_str();
            if name != "self" {
                self.graph
                    .checked_keywords(self.module, std::iter::once(keyword.range()));
                failure
                    .get_or_insert_with(|| format!("got an unexpected keyword argument '{name}'"));
            } else if positional != 0 || keyword_self.replace(&keyword.value).is_some() {
                self.graph
                    .checked_repeated_keyword(self.module, keyword.range());
                failure.get_or_insert_with(|| "got multiple values for argument 'self'".to_owned());
            }
        }
        if positional == 0 && keyword_self.is_none() {
            self.graph
                .checked_missing_argument(self.module, call.range());
        }
        let failure = failure.or_else(|| match positional {
            0 if keyword_self.is_none() => {
                Some("missing 1 required positional argument: 'self'".into())
            }
            2.. => Some(format!(
                "takes 1 positional argument but {positional} were given"
            )),
            _ => None,
        });
        if let Some(message) = failure {
            return self.counter_call_error(call, values, format!("Counter.elements() {message}"));
        }
        let mapping = if let Some(keyword) = keyword_self {
            self.expr(keyword)?
        } else {
            values.pop().unwrap()
        };
        let Some((key, Type::Int)) = self.types.mapping(mapping.ty) else {
            return Err(error(
                self.source,
                call,
                "elements requires a mapping with integer counts",
            ));
        };
        if !self.types.is_counter(mapping.ty) {
            // CPython's unbound method calls self.items(), including on dicts.
            self.graph.checked_overload(self.module, call.range());
        }
        Ok(Expr {
            ty: self.types.cursor(key),
            kind: ExprKind::Builtin(Builtin::CounterElements, vec![mapping]),
        })
    }

    /// Bind both forms of the Python method in source order, including
    /// `Counter.most_common(n=..., self=...)`.
    pub(super) fn counter_most_common(
        &mut self,
        call: &ast::ExprCall,
        receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let mut values: Vec<_> = receiver.into_iter().collect();
        let offset = values.len();
        let mut slots = [(offset == 1).then_some(0), None];
        let keywords = call.arguments.keywords.iter().map(|kw| {
            (
                match kw.arg.as_ref().map(|name| name.as_str()) {
                    Some("self") => 0,
                    Some("n") => 1,
                    _ => 2,
                },
                &kw.value,
            )
        });
        for (index, argument) in (offset..).zip(&call.arguments.args).chain(keywords) {
            if index >= slots.len() || slots[index].replace(values.len()).is_some() {
                return Err(error(
                    self.source,
                    call,
                    "most_common accepts one optional count",
                ));
            }
            let value = self.expr(argument)?;
            values.push(if index == 1 && value.ty == Type::Bool {
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                }
            } else {
                value
            });
        }
        let owner = slots[0]
            .ok_or_else(|| error(self.source, call, "most_common requires a Counter receiver"))?;
        let ty = values[owner].ty;
        if !self.types.is_counter(ty) {
            return Err(error(
                self.source,
                call,
                "most_common requires a Counter receiver",
            ));
        }
        let limit = if let Some(limit) = slots[1] {
            let ty = values[limit].ty;
            if !matches!(ty, Type::None | Type::Int) && self.types.payload(ty) != Some(Type::Int) {
                return Err(error(
                    self.source,
                    call,
                    "most_common requires an integer count or None",
                ));
            }
            limit
        } else {
            values.push(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            });
            values.len() - 1
        };
        let (key, count) = self.types.mapping(ty).unwrap();
        let result = self.types.list(self.types.tuple(vec![key, count]));
        let mut args = ir::CallArgs::positional(values);
        args.order = vec![owner, limit];
        Ok(self.ordered_builtin(Builtin::CounterMostCommon, result, args))
    }

    /// Root all evaluated call arguments until the catchable argument error.
    fn counter_call_error(
        &mut self,
        call: &ast::ExprCall,
        mut values: Vec<Expr>,
        message: String,
    ) -> Result<Expr, String> {
        for keyword in &call.arguments.keywords {
            if keyword.arg.is_none() {
                return Err(error(
                    self.source,
                    keyword,
                    "Counter keyword unpacking is unsupported",
                ));
            }
            values.push(self.expr(&keyword.value)?);
        }
        values.push(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(message))),
            },
        });
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(Builtin::RaiseException, values),
        })
    }

    pub(super) fn counter_unbound_update(
        &mut self,
        call: &ast::ExprCall,
        operation: Binary,
        name: &str,
    ) -> Result<Expr, String> {
        let Some((receiver, arguments)) = call.arguments.args.split_first() else {
            self.graph.checked_overload(self.module, call.range());
            return self.counter_call_error(
                call,
                vec![],
                format!("Counter.{name}() missing 1 required positional argument: 'self'"),
            );
        };
        let receiver = self.expr(receiver)?;
        if !self.types.is_counter(receiver.ty) {
            return Err(error(
                self.source,
                call,
                "an unbound Counter method requires a typed Counter receiver",
            ));
        }
        let mut bound = call.clone();
        bound.arguments.args = arguments.to_vec().into();
        self.counter_counts(&bound, Some(receiver), None, operation)
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
        self.counter_counts(call, receiver, hint, Binary::Add)
    }

    pub(super) fn counter_counts(
        &mut self,
        call: &ast::ExprCall,
        receiver: Option<Expr>,
        hint: Option<Type>,
        operation: Binary,
    ) -> Result<Expr, String> {
        if receiver.is_some() {
            self.graph.checked_overload(self.module, call.range());
        }
        let hint = receiver
            .as_ref()
            .and_then(|v| self.types.mapping(v.ty))
            .map(|(key, _)| key)
            .or(hint);
        if call.arguments.args.len() > 1
            && let Some(receiver) = receiver.as_ref()
        {
            let mut values = vec![receiver.clone()];
            for argument in &call.arguments.args {
                let value = if matches!(argument, ast::Expr::Dict(dict) if dict.items.is_empty()) {
                    self.expr_expected(argument, self.types.dict(hint.unwrap(), Type::Int))?
                } else {
                    self.expr(argument)?
                };
                values.push(value);
            }
            let name = if matches!(operation, Binary::Subtract) {
                "subtract"
            } else {
                "update"
            };
            return self.counter_call_error(
                call,
                values,
                format!(
                    "Counter.{name}() takes from 1 to 2 positional arguments but {} were given",
                    call.arguments.args.len() + 1
                ),
            );
        }
        let (receiver, input) = match call.arguments.args.as_ref() {
            [ast::Expr::NoneLiteral(_)] => (receiver, None),
            [source] => match (receiver, self.counter_input(source, hint)?) {
                (Some(receiver), Iteration::Container { iterable, .. })
                    if matches!(iterable.ty, Type::Int | Type::Bool | Type::Float) =>
                {
                    let message =
                        format!("'{}' object is not iterable", self.type_name(iterable.ty));
                    return self.counter_call_error(call, vec![receiver, iterable], message);
                }
                (
                    Some(receiver),
                    Iteration::Container {
                        iterable,
                        items: false,
                        start: None,
                    },
                ) if matches!(operation, Binary::Add)
                    && call.arguments.keywords.is_empty()
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
                        operation,
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
                operation,
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
}
