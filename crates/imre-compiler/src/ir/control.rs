//! Successor planning for truth-consuming tests and operand-returning expressions.
use super::{Builtin, Expr, ExprKind, ObjectUnary};

/// A truth-consuming expression or one whose selected operand escapes.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Context {
    Condition,
    Value,
}

pub(crate) enum Split<'a, E> {
    Operands(bool, &'a [E]),
    Pair(bool, &'a E, &'a E),
    Not(&'a E),
    Conditional(&'a E, &'a E, &'a E),
    Leaf,
}

/// Structural decomposition for planning. The graph contains only successor indices.
pub(crate) trait BranchShape: Sized {
    fn split(&self, context: Context) -> Split<'_, Self>;
}

impl BranchShape for Expr {
    fn split(&self, context: Context) -> Split<'_, Self> {
        match &self.kind {
            ExprKind::BooleanValue(and, left, right) => Split::Pair(*and, left, right),
            ExprKind::Boolean(and, operands) => Split::Operands(*and, operands),
            ExprKind::Conditional(test, yes, no) if context == Context::Condition => {
                Split::Conditional(test, yes, no)
            }
            ExprKind::Not(operand) if context == Context::Condition => Split::Not(operand),
            ExprKind::Builtin(Builtin::RegisteredUnary(ObjectUnary::Not), operands)
                if context == Context::Condition =>
            {
                Split::Not(&operands[0])
            }
            ExprKind::Builtin(Builtin::Bool, operands) if context == Context::Condition => {
                operands[0].split(context)
            }
            ExprKind::Builtin(Builtin::RegisteredLocation(..), operands) => {
                operands[0].split(context)
            }
            _ => Split::Leaf,
        }
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
#[derive(Clone)]
pub(crate) struct Edges(pub(crate) Vec<(Target, Target)>);

pub(crate) fn edges<E: BranchShape>(expr: &E, context: Context, depth: Depth) -> (Edges, Vec<&E>) {
    let mut plan = Edges(Vec::new());
    let mut inputs = Vec::new();
    plan.lower(&mut inputs, expr, context, depth, Target::Yes, Target::No);
    (plan, inputs)
}

/// Typed boolean operands already lower to i1 and retain their own specializations.
#[derive(Clone, Copy)]
pub(crate) enum Depth {
    Root,
    All,
}

impl Edges {
    fn leaf<'a, E>(
        &mut self,
        inputs: &mut Vec<&'a E>,
        expr: &'a E,
        yes: Target,
        no: Target,
    ) -> Target {
        let index = inputs.len();
        inputs.push(expr);
        self.0.push((yes, no));
        Target::Step(index)
    }

    fn lower<'a, E: BranchShape>(
        &mut self,
        inputs: &mut Vec<&'a E>,
        expr: &'a E,
        context: Context,
        depth: Depth,
        yes: Target,
        no: Target,
    ) -> Target {
        let mut child = |expr, yes, no| match depth {
            Depth::Root => self.leaf(inputs, expr, yes, no),
            Depth::All => self.lower(inputs, expr, context, depth, yes, no),
        };
        match expr.split(context) {
            Split::Operands(and, operands) => {
                let (last, prefix) = operands.split_last().expect("nonempty boolean expression");
                let mut next = child(last, yes, no);
                for operand in prefix.iter().rev() {
                    next = if and {
                        child(operand, next, no)
                    } else {
                        child(operand, yes, next)
                    };
                }
                next
            }
            Split::Pair(and, left, right) => {
                let next = child(right, yes, no);
                if and {
                    child(left, next, no)
                } else {
                    child(left, yes, next)
                }
            }
            Split::Not(operand) => child(operand, no, yes),
            Split::Conditional(test, body, otherwise) => {
                let no_arm = child(otherwise, yes, no);
                let yes_arm = child(body, yes, no);
                child(test, yes_arm, no_arm)
            }
            Split::Leaf => self.leaf(inputs, expr, yes, no),
        }
    }
}
