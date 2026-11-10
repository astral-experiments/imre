//! Select operands for a logical operation inside a residual source suite.
use super::values::{self, RegisteredSource};
use crate::ir;
use ir::control::{BranchShape, Context, Depth, Edges, Split, edges};
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange};
use std::collections::HashMap;

impl BranchShape for ast::Expr {
    fn split(&self, context: Context) -> Split<'_, Self> {
        match self {
            Self::BoolOp(op) => Split::Operands(op.op == ast::BoolOp::And, &op.values),
            Self::If(test) if context == Context::Condition => {
                Split::Conditional(&test.test, &test.body, &test.orelse)
            }
            Self::UnaryOp(op) if context == Context::Condition && op.op == ast::UnaryOp::Not => {
                Split::Not(&op.operand)
            }
            _ => Split::Leaf,
        }
    }
}

/// Only complete semantic expressions are retained. A residual operation keeps its
/// original source owner. Nested work is already present in a complete parent.
#[derive(Default)]
pub(crate) struct LogicalSites {
    selected: HashMap<TextRange, Selection>,
    covered: Option<TextRange>,
}

pub(crate) enum Selection {
    Checked(ir::Expr),
    Mixed {
        parts: Option<Box<[Option<ir::Expr>; 3]>>,
        branch: PreparedSteps,
        value: Option<PreparedSteps>,
    },
}

pub(crate) struct PreparedSteps {
    edges: Edges,
    operands: Vec<Option<ir::Expr>>,
}

impl PreparedSteps {
    fn new(node: &ast::Expr, context: Context, source: RegisteredSource<'_>) -> Self {
        let (edges, leaves) = edges(node, context, Depth::All);
        let operands = leaves
            .into_iter()
            .map(|leaf| values::registered_value(leaf, source))
            .collect();
        Self { edges, operands }
    }

    /// Find the original, explicitly residual leaves at this operation. No syntax
    /// is stored in the plan or used to reconstruct its edges or its checked values.
    fn inputs<'a>(&self, node: &'a ast::Expr, context: Context) -> Option<Vec<SourceOperand<'a>>> {
        let mut pending = vec![node];
        let mut inputs = Vec::with_capacity(self.operands.len());
        while let Some(node) = pending.pop() {
            match node.split(context) {
                Split::Operands(_, children) => pending.extend(children),
                Split::Pair(_, left, right) => pending.extend([left, right]),
                Split::Conditional(test, yes, no) => pending.extend([test, yes, no]),
                Split::Not(child) => pending.push(child),
                Split::Leaf => inputs.push(match self.operands.get(inputs.len())? {
                    Some(value) => SourceOperand::Checked(value.clone()),
                    None => SourceOperand::Residual(node),
                }),
            }
        }
        (inputs.len() == self.operands.len()).then_some(inputs)
    }
}

impl LogicalSites {
    fn governs(node: &ast::Expr) -> bool {
        matches!(node, ast::Expr::BoolOp(_) | ast::Expr::If(_))
            || matches!(node, ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not)
    }

    pub(crate) fn get(&self, node: &ast::Expr) -> Option<&Selection> {
        Self::governs(node)
            .then(|| self.selected.get(&node.range()))
            .flatten()
    }

    pub(crate) fn choice(&self, node: &ast::ExprIf) -> Option<&[Option<ir::Expr>; 3]> {
        match self.selected.get(&node.range()) {
            Some(Selection::Mixed {
                parts: Some(parts), ..
            }) => Some(parts),
            _ => None,
        }
    }

    pub(crate) fn visit(&mut self, node: &ast::Expr, source: RegisteredSource<'_>) {
        if !Self::governs(node)
            || self
                .covered
                .is_some_and(|covered| covered.contains_range(node.range()))
        {
            return;
        }
        let mixed = |parts| Selection::Mixed {
            parts,
            branch: PreparedSteps::new(node, Context::Condition, source),
            value: node
                .is_bool_op_expr()
                .then(|| PreparedSteps::new(node, Context::Value, source)),
        };
        let selected = if let ast::Expr::If(branch) = node {
            match [&branch.test, &branch.body, &branch.orelse]
                .map(|part| values::registered_value(part, source))
            {
                [Some(test), Some(yes), Some(no)] => {
                    Selection::Checked(source.located(node, super::select(test, yes, no)))
                }
                parts => mixed(Some(Box::new(parts))),
            }
        } else {
            match values::registered_value(node, source) {
                Some(value) => Selection::Checked(value),
                None => mixed(None),
            }
        };
        if matches!(selected, Selection::Checked(_)) {
            self.covered = Some(node.range());
        }
        self.selected.insert(node.range(), selected);
    }
}

pub(crate) enum SourceOperand<'a> {
    Checked(ir::Expr),
    Residual(&'a ast::Expr),
}

/// The source adapter owns each irreducible operation. Only successor indices and
/// evaluated values cross into the LLVM frame.
pub(crate) fn prepare_boolean<'a>(
    expression: &'a ast::Expr,
    context: Context,
    source: RegisteredSource<'_>,
    sites: &LogicalSites,
) -> (Edges, Vec<SourceOperand<'a>>) {
    if let Some(Selection::Mixed { branch, value, .. }) = sites.get(expression) {
        let plan = match context {
            Context::Condition => Some(branch),
            Context::Value => value.as_ref(),
        };
        if let Some(plan) = plan
            && let Some(inputs) = plan.inputs(expression, context)
        {
            return (plan.edges.clone(), inputs);
        }
    }
    let (edges, leaves) = edges(expression, context, Depth::All);
    let leaves = leaves
        .into_iter()
        .map(|leaf| match values::registered_value(leaf, source) {
            Some(operand) => SourceOperand::Checked(operand),
            None => SourceOperand::Residual(leaf),
        })
        .collect();
    (edges, leaves)
}
