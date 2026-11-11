//! Source-ordered comparison links, with an early exit for each false middle result.
use super::super::{Compare, Expr, Type, Value, read_only_field_operand};
use super::{CheckedBody, CheckedValue, Entry, ValueJoin};
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
        let mut chain = ComparisonChain::new(self, ops, truth);
        for expr in values {
            if let [op @ (CmpOp::In | CmpOp::NotIn)] = ops
                && let Some(CheckedValue::Handle { id, .. }) = &chain.left
                && let Some(result) =
                    self.python()
                        .literal_tuple_membership(id, expr, *op == CmpOp::NotIn)
            {
                return self.comparison_result(result, None, truth);
            }
            let value = if immediate {
                CheckedValue::native(self.fixed().immediate_receiver(expr))
            } else {
                self.store_operand(expr)
            };
            if let Some(result) = chain.push(self, value) {
                return result;
            }
        }
        unreachable!("checked comparisons end with a result")
    }

    fn comparison_result(
        &mut self,
        value: String,
        predicate: Option<String>,
        truth: bool,
    ) -> CheckedValue {
        if truth {
            CheckedValue::condition(
                predicate.unwrap_or_else(|| self.python().comparison_truth(&value, None)),
            )
        } else {
            CheckedValue::handle(value)
        }
    }

    fn compare_pair(
        &mut self,
        op: CmpOp,
        left: &CheckedValue,
        right: &CheckedValue,
    ) -> (String, Option<String>) {
        match (left, right) {
            (CheckedValue::Native(a, None), CheckedValue::Native(b, None)) => {
                let op = Compare::from_operator(op).expect("checked native comparison");
                let predicate = self.fixed().compare(op, &a.value, &b.value).operand;
                (predicate.clone(), Some(predicate))
            }
            (CheckedValue::Handle { id: a, .. }, CheckedValue::Handle { id: b, .. }) => {
                self.python().rich_comparison(op, a, b)
            }
            _ => unreachable!("comparison operands follow the selected representation"),
        }
    }

    fn release_comparison(&mut self, value: &CheckedValue) {
        if let CheckedValue::Native(operand, None) = value {
            self.fixed().release_operand(operand);
        }
    }
}

/// Owns the CFG and live middle operand between successive, source-ordered values.
struct ComparisonChain<'a> {
    ops: &'a [CmpOp],
    truth: bool,
    root: Option<(String, (String, String))>,
    right_scope: Option<(String, (String, String))>,
    left: Option<CheckedValue>,
    done: String,
    join: ValueJoin,
    branched: bool,
}

impl<'a> ComparisonChain<'a> {
    fn new(code: &mut CheckedBody<'_, '_, '_, '_>, ops: &'a [CmpOp], truth: bool) -> Self {
        let root = if matches!(code.0, Entry::Registered(_))
            && (ops.len() > 1 || matches!(ops, [CmpOp::Is | CmpOp::IsNot]))
        {
            Some(code.python().compare_scope())
        } else {
            None
        };
        Self {
            ops,
            root,
            truth,
            right_scope: None,
            left: None,
            done: code.body().name("compare_end"),
            join: ValueJoin::default(),
            branched: false,
        }
    }

    fn push(
        &mut self,
        code: &mut CheckedBody<'_, '_, '_, '_>,
        right: CheckedValue,
    ) -> Option<CheckedValue> {
        let Some(left) = self.left.take() else {
            self.prepare_right(code, right);
            return None;
        };
        let (&op, remaining) = self.ops.split_first().expect("checked comparison link");
        self.ops = remaining;
        let last = remaining.is_empty();
        let truth = self.truth;
        let root = &self.root;
        let done = &self.done;
        let join = &mut self.join;
        let right_scope = self.right_scope.take();
        let (value, predicate) = code.compare_pair(op, &left, &right);
        code.release_comparison(&left);
        let right_mark = right_scope.map(|(mark, edge)| {
            code.python().close_compare_scope(edge, &mark);
            mark
        });
        if last {
            if let Some((mark, _)) = root {
                // Release right before left; a rich comparison result may alias either.
                for boundary in [right_mark.as_ref().unwrap(), mark] {
                    if predicate.is_some() {
                        code.python().discard_truth(boundary);
                    } else {
                        code.python().keep_operands(boundary, &value, "0");
                    }
                }
            } else {
                code.release_comparison(&right);
            }
            let value = code.comparison_result(value, predicate, truth);
            let result = if self.branched {
                join.jump(code.body(), done, value);
                code.body().start(done);
                std::mem::take(join)
                    .finish(code.body())
                    .expect("comparison has a result")
            } else {
                value
            };
            if let Some((mark, edge)) = self.root.take() {
                code.python().close_compare_scope(edge, &mark);
            }
            return Some(result);
        }
        let rich = predicate.is_none();
        let middle = if let (Some((mark, _)), CheckedValue::Handle { id, .. }) = (root, &right) {
            Some(
                code.python()
                    .keep_operands(mark, id, if rich { &value } else { "0" }),
            )
        } else {
            None
        };
        let bit =
            predicate.unwrap_or_else(|| code.python().comparison_truth(&value, middle.as_deref()));
        if truth && rich {
            code.python().discard_truth(middle.as_ref().unwrap());
        }
        let next = code.body().name("compare_next");
        let stop = code.body().name("compare_stop");
        code.body().branch(&bit, &next, &stop);
        code.body().start(&stop);
        let temporaries = if let CheckedValue::Native(v, None) = &right {
            let saved = code.fixed().temporaries.clone();
            code.fixed().release_operand(v);
            Some(saved)
        } else {
            let mark = &root.as_ref().expect("rooted chain").0;
            if rich && !truth {
                code.python().keep_operands(mark, &value, "0");
            } else {
                code.python().discard_truth(mark);
            }
            None
        };
        join.jump(
            code.body(),
            done,
            if truth {
                CheckedValue::condition("false".into())
            } else {
                CheckedValue::handle(value)
            },
        );
        code.body().start(&next);
        if let Some(saved) = temporaries {
            code.fixed().temporaries = saved;
        }
        if rich && !truth {
            code.python().discard_truth(middle.as_ref().unwrap());
        }
        self.branched = true;
        self.prepare_right(code, right);
        None
    }

    fn prepare_right(&mut self, code: &mut CheckedBody<'_, '_, '_, '_>, left: CheckedValue) {
        self.left = Some(left);
        self.right_scope = self.root.as_ref().map(|_| code.python().compare_scope());
    }
}
