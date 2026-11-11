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

#[derive(Clone, Copy)]
pub(crate) enum Target {
    Step(usize),
    Yes,
    No,
}
/// Successors of each operand, in reverse execution order. The leaves stay
/// with the caller and are never evaluated while planning.
pub(crate) struct Edges(pub(crate) Vec<(Target, Target)>);

/// The expression to evaluate and, when needed, its specialized truth operation.
pub(crate) struct Input<'a> {
    pub(crate) value: &'a Expr,
    pub(crate) predicate: Option<&'a TruthPredicate>,
}

pub(crate) fn edges(expr: &Expr, context: Context, depth: Depth) -> (Edges, Vec<Input<'_>>) {
    let mut plan = Edges(Vec::new());
    let mut inputs = Vec::new();
    plan.lower(
        &mut inputs,
        Input {
            value: expr,
            predicate: None,
        },
        context,
        depth,
        Target::Yes,
        Target::No,
    );
    (plan, inputs)
}

/// Typed boolean operands already lower to i1 and retain their own specializations.
#[derive(Clone, Copy)]
pub(crate) enum Depth {
    Root,
    All,
}

impl Edges {
    fn leaf<'a>(
        &mut self,
        inputs: &mut Vec<Input<'a>>,
        input: Input<'a>,
        yes: Target,
        no: Target,
    ) -> Target {
        let index = inputs.len();
        inputs.push(input);
        self.0.push((yes, no));
        Target::Step(index)
    }

    fn lower<'a>(
        &mut self,
        inputs: &mut Vec<Input<'a>>,
        input: Input<'a>,
        context: Context,
        depth: Depth,
        yes: Target,
        no: Target,
    ) -> Target {
        let predicate = input.predicate;
        let mut child = |value, yes, no, predicate| {
            let input = Input { value, predicate };
            match depth {
                Depth::Root => self.leaf(inputs, input, yes, no),
                Depth::All => self.lower(inputs, input, context, depth, yes, no),
            }
        };
        match &context.operand(input.value).kind {
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
                    Context::Value => self.leaf(
                        inputs,
                        Input {
                            value: body,
                            predicate,
                        },
                        yes,
                        no,
                    ),
                };
                let condition = Input {
                    value: test,
                    predicate: None,
                };
                match depth {
                    Depth::Root => self.leaf(inputs, condition, yes_arm, no_arm),
                    Depth::All => self.lower(
                        inputs,
                        condition,
                        Context::Condition,
                        depth,
                        yes_arm,
                        no_arm,
                    ),
                }
            }
            _ => self.leaf(inputs, input, yes, no),
        }
    }
}
