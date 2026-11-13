//! Source-ordered comparison links, with an early exit for each false middle result.
use super::super::{Compare, Expr, Type, Value, read_only_field_operand};
use super::{CheckedBody, CheckedValue, Entry, ValueJoin};
use crate::llvm::registered::{permanent, permanent_handle};
use ruff_python_ast::CmpOp;

impl CheckedValue {
    pub(crate) fn into_condition(self) -> String {
        let operand = self.into_native();
        debug_assert_eq!(operand.value.ty, Type::Bool);
        operand.value.operand
    }
    pub(crate) fn condition(bit: String) -> Self {
        Self::owned(Value {
            ty: Type::Bool,
            operand: bit,
            small_integer: false,
        })
    }
}

impl<'f, 'a, 'p, 'm> CheckedBody<'f, 'a, 'p, 'm> {
    pub(crate) fn comparisons(
        &mut self,
        values: &[Expr],
        ops: &[CmpOp],
        truth: bool,
    ) -> CheckedValue {
        let immediate = match &mut self.0 {
            Entry::Native(code) => {
                if let Some(result) = code.specialized_comparison(values, ops) {
                    return CheckedValue::owned(result);
                }
                ops.len() == 1
                    && matches!(
                        values[0].ty,
                        Type::Str | Type::StrLiteral(_) | Type::Tuple(_)
                    )
                    && values.iter().all(read_only_field_operand)
            }
            Entry::Registered(_) => false,
        };
        let integer_pair = matches!(values, [a, b]
            if a.checked_operand_type() == Type::Int && b.checked_operand_type() == Type::Int);
        let root = if matches!(self.0, Entry::Registered(_))
            && (ops.len() > 1
                || matches!(ops, [CmpOp::Is | CmpOp::IsNot])
                || matches!(ops, [op] if truth && Compare::from_operator(*op).is_some()))
        {
            Some(self.python().compare_scope())
        } else {
            None
        };
        let done = self.body().name("compare_end");
        let mut join = ValueJoin::default();
        let operand = |code: &mut Self, expr: &Expr| {
            if immediate {
                CheckedValue::native(code.fixed().immediate_receiver(expr))
            } else {
                code.rooted_operand(expr)
            }
        };
        debug_assert_eq!(values.len(), ops.len() + 1);
        let mut left = operand(self, &values[0]);
        for (index, (&op, expr)) in ops.iter().zip(&values[1..]).enumerate() {
            if ops.len() == 1
                && matches!(op, CmpOp::In | CmpOp::NotIn)
                && let CheckedValue::Handle { id, .. } = &left
                && let Some(result) =
                    self.python()
                        .literal_sequence_membership(id, expr, op == CmpOp::NotIn)
            {
                // Membership has already tested each rich comparison and returned a Python bool.
                return if truth {
                    let bit = self
                        .python()
                        .value(&format!("icmp eq i64 {result}, {}", permanent::TRUE));
                    CheckedValue::condition(bit)
                } else {
                    CheckedValue::handle(result)
                };
            }
            // An identity test cannot run Python, and a permanent right operand has no root to release.
            let right_scope = (root.is_some()
                && !(matches!(op, CmpOp::Is | CmpOp::IsNot) && permanent_handle(expr).is_some()))
            .then(|| self.python().compare_scope());
            let right = operand(self, expr);
            let value = self.compare_pair(op, &left, &right, integer_pair);
            let handle = match &value {
                CheckedValue::Handle { id, .. } => Some(id.as_str()),
                _ => None,
            };
            self.release_comparison(&left);
            let right_mark = right_scope.map(|(mark, edge)| {
                self.python().close_compare_scope(edge, &mark);
                mark
            });
            if index + 1 == ops.len() {
                if let Some((mark, _)) = &root {
                    // Release right before left; a rich comparison result may alias either.
                    for boundary in right_mark.iter().chain([mark]) {
                        if let Some(id) = handle {
                            self.python().keep_operands(boundary, id, "0");
                        } else {
                            self.python().discard_truth(boundary);
                        }
                    }
                } else {
                    self.release_comparison(&right);
                }
                let tested_handle = truth && handle.is_some();
                let value = self.comparison_result(value, truth);
                if tested_handle && let Some((mark, _)) = &root {
                    // A branch keeps the truth bit. Release the rich result before the next operand.
                    self.python().discard_truth(mark);
                }
                let result = if index != 0 {
                    join.jump(self.body(), &done, value);
                    self.body().start(&done);
                    join.finish(self.body()).expect("comparison has a result")
                } else {
                    value
                };
                if let Some((mark, edge)) = root {
                    self.python().close_compare_scope(edge, &mark);
                }
                return result;
            }
            let middle = if let (Some((mark, _)), CheckedValue::Handle { id, .. }) = (&root, &right)
            {
                Some(self.python().keep_operands(mark, id, handle.unwrap_or("0")))
            } else {
                None
            };
            let bit = match handle {
                Some(id) => self.python().comparison_truth(id, middle.as_deref()),
                None => value.fixed_value().operand.clone(),
            };
            if truth && handle.is_some() {
                self.python().discard_truth(middle.as_ref().unwrap());
            }
            let next = self.body().name("compare_next");
            let stop = self.body().name("compare_stop");
            self.body().branch(&bit, &next, &stop);
            self.body().start(&stop);
            let temporaries = if let CheckedValue::Native(v, None) = &right {
                let saved = self.fixed().temporaries.clone();
                self.fixed().release_operand(v);
                Some(saved)
            } else {
                let mark = &root.as_ref().expect("rooted chain").0;
                if let Some(id) = handle.filter(|_| !truth) {
                    self.python().keep_operands(mark, id, "0");
                } else {
                    self.python().discard_truth(mark);
                }
                None
            };
            join.jump(
                self.body(),
                &done,
                if truth {
                    CheckedValue::condition("false".into())
                } else {
                    CheckedValue::handle(
                        handle.map_or_else(|| permanent::FALSE.to_string(), str::to_owned),
                    )
                },
            );
            self.body().start(&next);
            if let Some(saved) = temporaries {
                self.fixed().temporaries = saved;
            }
            if handle.is_some() && !truth {
                self.python().discard_truth(middle.as_ref().unwrap());
            }
            left = right;
        }
        unreachable!("checked comparisons end with a result")
    }

    fn comparison_result(&mut self, value: CheckedValue, truth: bool) -> CheckedValue {
        match (&value, truth) {
            (CheckedValue::Native(_, None), false) => {
                self.python().boolean_result(&value.into_condition(), false)
            }
            (CheckedValue::Handle { id, .. }, true) => {
                CheckedValue::condition(self.python().comparison_truth(id, None))
            }
            _ => value,
        }
    }

    fn compare_pair(
        &mut self,
        op: CmpOp,
        left: &CheckedValue,
        right: &CheckedValue,
        integer_pair: bool,
    ) -> CheckedValue {
        match (left, right) {
            (CheckedValue::Native(a, None), CheckedValue::Native(b, None)) => {
                let op = Compare::from_operator(op).expect("checked native comparison");
                CheckedValue::owned(self.fixed().compare(op, &a.value, &b.value))
            }
            (CheckedValue::Handle { id: a, .. }, CheckedValue::Handle { id: b, .. }) => {
                if matches!(op, CmpOp::Is | CmpOp::IsNot) {
                    let relation = if op == CmpOp::Is { "eq" } else { "ne" };
                    CheckedValue::condition(
                        self.body()
                            .instruction(format!("icmp {relation} i64 {a}, {b}")),
                    )
                } else if integer_pair && let Some(order) = Compare::from_operator(op) {
                    let (x, _) = self.integer_operand(left);
                    let (y, _) = self.integer_operand(right);
                    self.integer_relation(order, &x, &y, |code| {
                        CheckedValue::handle(code.python().op(op.as_str(), &[a, b]))
                    })
                } else {
                    CheckedValue::handle(self.python().op(op.as_str(), &[a, b]))
                }
            }
            _ => unreachable!("comparison operands follow the selected representation"),
        }
    }

    /// The tagged words preserve signed integer order. A heap value takes the
    /// calling convention's original integer or Python comparison.
    pub(in crate::llvm) fn integer_relation(
        &mut self,
        order: Compare,
        x: &str,
        y: &str,
        fallback: impl FnOnce(&mut Self) -> CheckedValue,
    ) -> CheckedValue {
        let both = self.body().instruction(format!("and i64 {x}, {y}"));
        let tagged = self.body().instruction(format!("trunc i64 {both} to i1"));
        ValueJoin::select(
            self,
            Self::body,
            |code, yes, no| code.body().conditional(&tagged, yes, no, Some(true)),
            |code| {
                let predicate = order.signed_predicate();
                let bit = code
                    .body()
                    .instruction(format!("icmp {predicate} i64 {x}, {y}"));
                match &mut code.0 {
                    Entry::Native(_) => CheckedValue::condition(bit),
                    Entry::Registered(frame) => frame.boolean_result(&bit, false),
                }
            },
            fallback,
        )
        .expect("a comparison has a normal edge")
    }

    fn release_comparison(&mut self, value: &CheckedValue) {
        if let CheckedValue::Native(operand, None) = value {
            self.fixed().release_operand(operand);
        }
    }
}
