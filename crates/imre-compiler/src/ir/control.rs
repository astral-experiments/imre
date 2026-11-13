//! Successor planning for truth-consuming tests and operand-returning expressions.
use super::{Builtin, Expr, ExprKind, TruthPredicate};
use ruff_python_ast::UnaryOp;

/// A truth-consuming expression or one whose selected operand escapes.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Context {
    Condition,
    Value,
}

impl Context {
    fn operand(self, mut expr: &Expr) -> &Expr {
        while let ExprKind::Builtin(op, operands) = &expr.kind {
            match op {
                Builtin::RegisteredLocation(..) => {}
                Builtin::Bool if self == Self::Condition => {}
                _ => break,
            }
            expr = &operands[0];
        }
        expr
    }
}

pub(crate) fn boolean_literal(expr: &Expr) -> Option<bool> {
    match Context::Condition.operand(expr).kind {
        ExprKind::Bool(value) => Some(value),
        _ => None,
    }
}

/// A single operand can go straight to its truth test and the caller's two successors.
pub(crate) fn needs_truth_plan(expr: &Expr) -> bool {
    matches!(
        Context::Condition.operand(expr).kind,
        ExprKind::Conditional(..)
            | ExprKind::Boolean(..)
            | ExprKind::BooleanValue(..)
            | ExprKind::Unary(UnaryOp::Not, _)
    )
}

#[derive(Clone, Copy)]
pub(crate) enum Target {
    Step(usize),
    Yes,
    No,
}
/// Checked operands and their truth successors, in reverse execution order.
pub(crate) struct Steps<'a>(pub(crate) Vec<Step<'a>>);

pub(crate) struct Step<'a> {
    pub(crate) value: &'a Expr,
    pub(crate) predicate: Option<&'a TruthPredicate>,
    pub(crate) yes: Target,
    pub(crate) no: Target,
}

pub(crate) fn steps(expr: &Expr, context: Context, depth: Depth) -> Steps<'_> {
    let mut plan = Steps(Vec::new());
    plan.lower(
        Step {
            value: expr,
            predicate: None,
            yes: Target::Yes,
            no: Target::No,
        },
        context,
        depth,
    );
    plan
}

/// Typed boolean operands already lower to i1 and retain their own specializations.
#[derive(Clone, Copy)]
pub(crate) enum Depth {
    Root,
    All,
}

impl<'a> Steps<'a> {
    fn leaf(&mut self, step: Step<'a>) -> Target {
        let index = self.0.len();
        self.0.push(step);
        Target::Step(index)
    }

    fn lower(&mut self, step: Step<'a>, context: Context, depth: Depth) -> Target {
        let Step {
            value,
            predicate,
            yes,
            no,
        } = step;
        let mut child = |value, yes, no, predicate| {
            let step = Step {
                value,
                predicate,
                yes,
                no,
            };
            match depth {
                Depth::Root => self.leaf(step),
                Depth::All => self.lower(step, context, depth),
            }
        };
        match &context.operand(value).kind {
            ExprKind::Boolean(and, operands) => {
                let (last, prefix) = operands.split_last().expect("nonempty boolean expression");
                let mut next = child(last, yes, no, predicate);
                for operand in prefix.iter().rev() {
                    next = if *and {
                        child(operand, next, no, None)
                    } else {
                        child(operand, yes, next, None)
                    };
                }
                next
            }
            ExprKind::BooleanValue(and, left, right, left_test) => {
                let next = child(right, yes, no, predicate);
                if *and {
                    child(left, next, no, left_test.as_ref())
                } else {
                    child(left, yes, next, left_test.as_ref())
                }
            }
            ExprKind::Unary(UnaryOp::Not, operand) if context == Context::Condition => {
                child(operand, no, yes, None)
            }
            ExprKind::Conditional(test, body, otherwise) => {
                let no_arm = child(otherwise, yes, no, predicate);
                // The else arm falls through into the enclosing Boolean expression.
                // The then arm jumps over it with a materialized result.
                let yes_arm = match context {
                    Context::Condition => child(body, yes, no, predicate),
                    Context::Value => self.leaf(Step {
                        value: body,
                        ..step
                    }),
                };
                let condition = Step {
                    value: test,
                    predicate: None,
                    yes: yes_arm,
                    no: no_arm,
                };
                match depth {
                    Depth::Root => self.leaf(condition),
                    Depth::All => self.lower(condition, Context::Condition, depth),
                }
            }
            _ => self.leaf(step),
        }
    }
}
