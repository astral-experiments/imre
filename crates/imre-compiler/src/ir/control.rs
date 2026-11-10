//! Successor planning for truth-consuming tests and operand-returning expressions.
use super::{Builtin, Expr, ExprKind, ObjectUnary, TruthPredicate};

/// A truth-consuming expression or one whose selected operand escapes.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Context {
    Condition,
    Value,
}

pub(crate) enum Split<'a, E> {
    Operands(bool, &'a [E]),
    Pair(bool, &'a E, &'a E, Option<&'a TruthPredicate>),
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
            ExprKind::BooleanValue(and, left, right, predicate) => {
                Split::Pair(*and, left, right, predicate.as_ref())
            }
            ExprKind::Boolean(and, operands) => Split::Operands(*and, operands),
            ExprKind::Conditional(test, yes, no) => Split::Conditional(test, yes, no),
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

/// The expression to evaluate and, when needed, its specialized truth operation.
pub(crate) struct Input<'a, E> {
    pub(crate) value: &'a E,
    pub(crate) predicate: Option<&'a TruthPredicate>,
}

pub(crate) fn edges<E: BranchShape>(
    expr: &E,
    context: Context,
    depth: Depth,
) -> (Edges, Vec<Input<'_, E>>) {
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
    fn leaf<'a, E>(
        &mut self,
        inputs: &mut Vec<Input<'a, E>>,
        input: Input<'a, E>,
        yes: Target,
        no: Target,
    ) -> Target {
        let index = inputs.len();
        inputs.push(input);
        self.0.push((yes, no));
        Target::Step(index)
    }

    fn lower<'a, E: BranchShape>(
        &mut self,
        inputs: &mut Vec<Input<'a, E>>,
        input: Input<'a, E>,
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
        match input.value.split(context) {
            Split::Operands(and, operands) => {
                let (last, prefix) = operands.split_last().expect("nonempty boolean expression");
                let mut next = child(last, yes, no, predicate);
                for operand in prefix.iter().rev() {
                    next = if and {
                        child(operand, next, no, None)
                    } else {
                        child(operand, yes, next, None)
                    };
                }
                next
            }
            Split::Pair(and, left, right, left_test) => {
                let next = child(right, yes, no, predicate);
                if and {
                    child(left, next, no, left_test)
                } else {
                    child(left, yes, next, left_test)
                }
            }
            Split::Not(operand) => child(operand, no, yes, None),
            Split::Conditional(test, body, otherwise) => {
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
            Split::Leaf => self.leaf(inputs, input, yes, no),
        }
    }
}
