//! Source-ordered comparison links, with an early exit for each false middle result.
use super::super::{Compare, Emitter, Expr, Operand, Type, Value};
use super::{CheckedBody, CheckedValue, ValueJoin};
use crate::llvm::registered::{Frame, RegisteredFrame, sites::CheckedSites};
use ruff_python_ast::CmpOp;

enum Relation {
    Fixed(Compare),
    Python(CmpOp),
}

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
    pub(in crate::llvm) fn native_comparisons(
        &mut self,
        values: &[Expr],
        ops: &[Compare],
        operand: fn(&mut Emitter<'a, 'm>, &Expr) -> Operand,
    ) -> CheckedValue {
        let mut chain = ComparisonChain::new(self, Relations::Fixed(ops), true);
        values
            .iter()
            .find_map(|expr| {
                let value = CheckedValue::native(operand(self.fixed(), expr));
                chain.push(self, value)
            })
            .expect("complete fixed comparison")
    }

    pub(crate) fn registered_comparisons(
        &mut self,
        values: &[Expr],
        ops: &[CmpOp],
        truth: bool,
    ) -> CheckedValue {
        ComparisonChain::emit_registered(
            self,
            ops,
            truth,
            |body| body.python(),
            values,
            |body, expr| {
                let value = body.evaluate(expr);
                body.python().materialize(&value)
            },
        )
    }

    fn compare_pair(
        &mut self,
        relation: Relation,
        left: &CheckedValue,
        right: &CheckedValue,
    ) -> (String, Option<String>) {
        match (relation, left, right) {
            (Relation::Fixed(op), CheckedValue::Native(a, None), CheckedValue::Native(b, None)) => {
                let predicate = self.fixed().compare(op, &a.value, &b.value).operand;
                (predicate.clone(), Some(predicate))
            }
            (
                Relation::Python(op),
                CheckedValue::Handle { id: a, .. },
                CheckedValue::Handle { id: b, .. },
            ) => self.python().rich_comparison(op, a, b),
            _ => unreachable!("comparison operands follow the selected representation"),
        }
    }

    fn release_comparison(&mut self, value: &CheckedValue) {
        if let CheckedValue::Native(operand, None) = value {
            self.fixed().release_operand(operand);
        }
    }
}

enum Relations<'a> {
    Fixed(&'a [Compare]),
    Python(&'a [CmpOp]),
}

impl Relations<'_> {
    fn next(&mut self) -> (Relation, bool) {
        match self {
            Self::Fixed(ops) => {
                let (first, rest) = ops.split_first().expect("missing fixed comparison");
                *ops = rest;
                (Relation::Fixed(*first), rest.is_empty())
            }
            Self::Python(ops) => {
                let (first, rest) = ops.split_first().expect("missing Python comparison");
                *ops = rest;
                (Relation::Python(*first), rest.is_empty())
            }
        }
    }
}

/// Owns the CFG and live middle operand between successive, source-ordered values.
pub(crate) struct ComparisonChain<'a> {
    ops: Relations<'a>,
    truth: bool,
    root: Option<(String, (String, String))>,
    right_scope: Option<(String, (String, String))>,
    left: Option<CheckedValue>,
    done: String,
    join: ValueJoin,
    branched: bool,
}

impl<'a> ComparisonChain<'a> {
    fn new(code: &mut CheckedBody<'_, '_, '_, '_>, ops: Relations<'a>, truth: bool) -> Self {
        let root = match ops {
            Relations::Python(ops)
                if ops.len() > 1 || matches!(ops, [CmpOp::Is | CmpOp::IsNot]) =>
            {
                Some(code.python().compare_scope())
            }
            _ => None,
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

    /// Each reached link pulls its next rooted operand in the active frame.
    pub(crate) fn emit_registered<'f, 'p, C, T>(
        context: &mut C,
        ops: &'a [CmpOp],
        truth: bool,
        frame: impl Fn(&mut C) -> &mut RegisteredFrame<'f, 'p>,
        operands: impl IntoIterator<Item = T>,
        mut evaluate: impl FnMut(&mut C, T) -> String,
    ) -> CheckedValue {
        let sites = CheckedSites::default();
        let mut chain = Self::new(
            &mut CheckedBody::registered(Frame::for_expression(frame(context), &sites)),
            Relations::Python(ops),
            truth,
        );
        for operand in operands {
            let handle = evaluate(context, operand);
            let mut body = CheckedBody::registered(Frame::for_expression(frame(context), &sites));
            if let Some(result) = chain.push(&mut body, CheckedValue::handle(handle)) {
                return result;
            }
        }
        unreachable!("a comparison has a final link")
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
        let (op, last) = self.ops.next();
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
            let value = if truth {
                CheckedValue::condition(
                    predicate.unwrap_or_else(|| code.python().comparison_truth(&value, None)),
                )
            } else {
                CheckedValue::handle(value)
            };
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
