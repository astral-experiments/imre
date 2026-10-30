//! Control flow for branching on a boxed expression or returning its selected operand.

use ruff_python_ast::{BoolOp, Expr, UnaryOp};

/// A successor step, a known truth value, or the original operand.
#[derive(Clone, Copy)]
pub(super) enum Target {
    Step(usize),
    Truth(bool),
    Operand,
}

/// Boolean expressions select their next step by the truth of this operand.
pub(super) struct Step<'a> {
    pub(super) expr: &'a Expr,
    pub(super) yes: Target,
    pub(super) no: Target,
}

/// A branch consumes truth, while an expression can return a user-defined object.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Context {
    Condition,
    Value,
}

impl Context {
    /// Single operands can be tested directly without building a graph.
    pub(super) fn splits(self, expr: &Expr) -> bool {
        matches!(expr, Expr::BoolOp(_))
            || self == Self::Condition
                && (matches!(expr, Expr::If(_))
                    || matches!(expr, Expr::UnaryOp(op) if op.op == UnaryOp::Not))
    }
}

/// A forward-only graph. The entry is the last step, and each successor has a lower index.
pub(super) struct Flow<'a> {
    pub(super) steps: Vec<Step<'a>>,
}

impl<'a> Flow<'a> {
    /// Lower Python's branches or its operand-returning boolean expressions.
    ///
    /// In `if a or b`, a truthy `a` goes straight to the body. In `x = a or b`,
    /// the same edge returns `a` itself, and the final operand needs no truth test.
    pub(super) fn new(expr: &'a Expr, context: Context) -> Self {
        let mut flow = Self { steps: Vec::new() };
        let (yes, no) = match context {
            Context::Condition => (Target::Truth(true), Target::Truth(false)),
            Context::Value => (Target::Operand, Target::Operand),
        };
        flow.lower(expr, context, yes, no);
        flow
    }

    fn lower(&mut self, expr: &'a Expr, context: Context, yes: Target, no: Target) -> Target {
        match expr {
            Expr::BoolOp(op) => {
                let (last, rest) = op
                    .values
                    .split_last()
                    .expect("boolean expression has operands");
                let mut next = self.lower(last, context, yes, no);
                for operand in rest.iter().rev() {
                    next = match op.op {
                        BoolOp::And => self.lower(operand, context, next, no),
                        BoolOp::Or => self.lower(operand, context, yes, next),
                    };
                }
                next
            }
            Expr::UnaryOp(op) if context == Context::Condition && op.op == UnaryOp::Not => {
                self.lower(&op.operand, context, no, yes)
            }
            Expr::If(op) if context == Context::Condition => {
                let otherwise = self.lower(&op.orelse, context, yes, no);
                let body = self.lower(&op.body, context, yes, no);
                self.lower(&op.test, context, body, otherwise)
            }
            _ => {
                let index = self.steps.len();
                self.steps.push(Step { expr, yes, no });
                Target::Step(index)
            }
        }
    }
}
