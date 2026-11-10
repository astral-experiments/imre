//! Prepare expression inputs owned by residual source suites.
use super::values::{self, RegisteredSource};
use crate::ir;
use ast::visitor::{self, Visitor};
use ir::control::{BranchShape, Context, Depth, Edges, Split, edges};
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};
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

/// Complete expressions own their checked children. Mixed sites retain the operation
/// and the inputs the frontend could check in their physical activation.
#[derive(Default)]
pub(crate) struct ExpressionSites {
    selected: HashMap<(TextRange, ast::NodeKind), Selection>,
    covered: Option<TextRange>,
}

pub(crate) enum Selection {
    Checked(ir::Expr),
    Operand(Box<super::RegisteredOperand>),
    Refused(TextRange, &'static str),
    Inputs(Box<PhysicalInputs>),
    /// Only the child remains physical. It runs under the suspension's operand mark.
    Suspension(ir::SuspendValue, Box<ast::Expr>),
    Comparison(Vec<Option<CheckedChild>>),
    Mixed {
        parts: Option<Box<[PreparedInput; 3]>>,
        branch: PreparedSteps,
        value: Option<PreparedSteps>,
        physical: Option<Box<PhysicalInputs>>,
    },
}

impl Selection {
    fn mixed(node: &ast::Expr, source: RegisteredSource<'_>) -> Self {
        Self::Mixed {
            parts: None,
            physical: None,
            branch: PreparedSteps::new(node, Context::Condition, source),
            value: node
                .is_bool_op_expr()
                .then(|| PreparedSteps::new(node, Context::Value, source)),
        }
    }
}

/// A physical owner supplies any remaining inputs before the checked operation runs.
pub(crate) struct PhysicalInputs {
    pub(crate) operation: ir::Expr,
    pub(crate) position: TextSize,
    children: Box<[PreparedInput]>,
}

pub(crate) type PreparedInput = SourceOperand<ir::Expr, ResidualInput>;

pub(crate) struct ResidualInput {
    expression: Box<ast::Expr>,
    // The same site can be emitted in an ordinary or a suspended activation.
    suspended: Option<ir::Expr>,
}

fn site(node: &ast::Expr) -> (TextRange, ast::NodeKind) {
    (node.range(), ast::AnyNodeRef::from(node).kind())
}

impl PhysicalInputs {
    pub(crate) fn operands(&self, suspended: bool) -> impl Iterator<Item = BorrowedOperand<'_>> {
        self.children
            .iter()
            .map(move |child| child.operand(suspended))
    }
}

impl PreparedInput {
    pub(crate) fn operand(&self, suspended: bool) -> BorrowedOperand<'_> {
        match self {
            SourceOperand::Checked(value) => SourceOperand::Checked(value),
            SourceOperand::Residual(input) => BorrowedOperand::prepared(
                &input.expression,
                input.suspended.as_ref().filter(|_| suspended),
            ),
        }
    }
}

pub(crate) struct CheckedChild {
    value: ir::Expr,
    requires_suspension: bool,
}

impl CheckedChild {
    fn prepare(node: &ast::Expr, source: RegisteredSource<'_>) -> Option<Self> {
        let ordinary = values::registered_value(node, source);
        let requires_suspension = ordinary.is_none();
        let value = ordinary.or_else(|| values::suspended_operand(node, source))?;
        Some(Self {
            value,
            requires_suspension,
        })
    }

    fn value(&self, suspended: bool) -> Option<&ir::Expr> {
        (suspended || !self.requires_suspension).then_some(&self.value)
    }
}

pub(crate) type LogicalOperand = SourceOperand<ir::Expr, Box<ast::Expr>>;

pub(crate) struct PreparedSteps {
    pub(crate) edges: Edges,
    pub(crate) operands: Vec<LogicalOperand>,
}

impl PreparedSteps {
    fn new(node: &ast::Expr, context: Context, source: RegisteredSource<'_>) -> Self {
        let (edges, leaves) = edges(node, context, Depth::All);
        let operands = leaves
            .into_iter()
            .map(|leaf| match values::registered_value(leaf, source) {
                Some(value) => SourceOperand::Checked(value),
                None => SourceOperand::Residual(Box::new(leaf.clone())),
            })
            .collect();
        Self { edges, operands }
    }
}

impl ExpressionSites {
    fn governs(node: &ast::Expr) -> bool {
        matches!(node, ast::Expr::BoolOp(_) | ast::Expr::If(_))
            || matches!(node, ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not)
    }

    pub(crate) fn get(&self, node: &ast::Expr) -> Option<&Selection> {
        self.selected.get(&site(node))
    }

    pub(crate) fn comparison<'a>(
        &'a self,
        node: &'a ast::Expr,
        suspended: bool,
    ) -> Option<(
        &'a [ast::CmpOp],
        impl Iterator<Item = BorrowedOperand<'a>> + 'a,
    )> {
        let cmp = node.as_compare_expr()?;
        let inputs = match self.get(node) {
            Some(Selection::Comparison(inputs)) => inputs.as_slice(),
            _ => &[],
        };
        let operands = cmp.operands.iter().enumerate().map(move |(index, child)| {
            let value = inputs
                .get(index)
                .and_then(Option::as_ref)
                .and_then(|v| v.value(suspended));
            BorrowedOperand::prepared(child, value)
        });
        Some((&cmp.ops, operands))
    }

    #[cfg(test)]
    pub(crate) fn replace_residual_input(&mut self, node: &ast::Expr, at: usize, value: ir::Expr) {
        let Some(
            Selection::Inputs(inputs)
            | Selection::Mixed {
                physical: Some(inputs),
                ..
            },
        ) = self.selected.get_mut(&site(node))
        else {
            panic!("expected prepared mixed operands");
        };
        assert!(matches!(inputs.children[at], SourceOperand::Residual(_)));
        inputs.children[at] = SourceOperand::Checked(value);
    }

    pub(crate) fn physical(&self, node: &ast::Expr) -> Option<&PhysicalInputs> {
        match self.selected.get(&site(node)) {
            Some(Selection::Inputs(inputs))
            | Some(Selection::Mixed {
                physical: Some(inputs),
                ..
            }) => Some(inputs),
            _ => None,
        }
    }

    /// Nested suites and fully checked declaration bodies have their own plans.
    pub(super) fn prepare_inputs(
        &mut self,
        stmt: &ast::Stmt,
        source: RegisteredSource<'_>,
        scope: super::RegisteredScope,
        suspended: bool,
    ) {
        PhysicalPreparation {
            sites: self,
            source,
            scope,
            suspended,
        }
        .visit_stmt(stmt);
    }

    pub(crate) fn prepare_input(
        &mut self,
        expr: &ast::Expr,
        source: RegisteredSource<'_>,
        scope: super::RegisteredScope,
        suspended: bool,
    ) {
        PhysicalPreparation {
            sites: self,
            source,
            scope,
            suspended,
        }
        .visit_expr(expr);
    }

    pub(crate) fn visit(&mut self, node: &ast::Expr, source: RegisteredSource<'_>) {
        if let Some(cmp) = node.as_compare_expr() {
            let inputs: Vec<_> = cmp
                .operands
                .iter()
                .map(|child| CheckedChild::prepare(child, source))
                .collect();
            // Whole checked comparisons, including suspendable ones, keep their normal entry.
            if inputs.iter().any(Option::is_none) {
                self.selected
                    .insert(site(node), Selection::Comparison(inputs));
            }
            return;
        }
        if !Self::governs(node)
            || self
                .covered
                .is_some_and(|covered| covered.contains_range(node.range()))
        {
            return;
        }
        let selected = match values::registered_value(node, source) {
            Some(value) => Selection::Checked(value),
            None => Selection::mixed(node, source),
        };
        if matches!(selected, Selection::Checked(_)) {
            self.covered = Some(node.range());
        }
        self.selected.insert(site(node), selected);
    }
}

struct PhysicalPreparation<'a, 's> {
    sites: &'a mut ExpressionSites,
    source: RegisteredSource<'s>,
    scope: super::RegisteredScope,
    suspended: bool,
}

impl<'a> Visitor<'a> for PhysicalPreparation<'_, '_> {
    fn visit_body(&mut self, _body: &'a [ast::Stmt]) {}

    fn visit_expr(&mut self, node: &'a ast::Expr) {
        // Store and delete targets consume their receivers, keys or unpacked children.
        // They have no value to emit for the target itself.
        if let ast::Expr::Subscript(index) = node
            && index.ctx != ast::ExprContext::Load
        {
            self.visit_expr(&index.value);
            if let Some(key) = values::registered_index_key(&index.slice, self.source) {
                self.sites
                    .selected
                    .insert(site(&index.slice), Selection::Checked(key));
            } else {
                self.visit_expr(&index.slice);
            }
            return;
        }
        let target = match node {
            ast::Expr::Name(v) => Some(v.ctx),
            ast::Expr::Attribute(v) => Some(v.ctx),
            ast::Expr::List(v) => Some(v.ctx),
            ast::Expr::Tuple(v) => Some(v.ctx),
            ast::Expr::Starred(v) => Some(v.ctx),
            _ => None,
        };
        if target.is_some_and(|ctx| ctx != ast::ExprContext::Load) {
            visitor::walk_expr(self, node);
            return;
        }
        let selected = self.sites.get(node);
        match selected {
            Some(Selection::Comparison(_)) => {
                visitor::walk_expr(self, node);
                return;
            }
            Some(
                Selection::Mixed { parts: Some(_), .. }
                | Selection::Mixed {
                    physical: Some(_), ..
                },
            ) => return,
            Some(Selection::Mixed { .. }) if !self.suspended => {
                self.residual(node);
                return;
            }
            Some(Selection::Mixed { .. }) | None => {}
            Some(_) => return,
        }
        let region = if self.suspended {
            values::Region::SuspendedOperand
        } else {
            values::Region::Residual
        };
        if !node.is_t_string_expr()
            && let Some(value) = region.operand(node, self.source)
        {
            self.sites
                .selected
                .insert(site(node), Selection::Checked(value));
            return;
        }
        match super::inline_operand(node, self.source, self.scope, self.suspended) {
            Ok(Some(operand)) => {
                self.sites
                    .selected
                    .insert(site(node), Selection::Operand(Box::new(operand)));
                return;
            }
            Err((range, reason)) => {
                self.sites
                    .selected
                    .insert(site(node), Selection::Refused(range, reason));
                return;
            }
            Ok(None) => {}
        }
        if self.suspended
            && let Some((kind, Some(child))) = ir::SuspendValue::operand(node)
        {
            let selected =
                match super::RegisteredOperand::prepare_with(self.source, self.scope, |scope| {
                    scope.with_suspending_operands(|scope| scope.operand(node))
                }) {
                    Some(operand) => Selection::Operand(Box::new(operand)),
                    None => {
                        self.visit_expr(child);
                        Selection::Suspension(kind, Box::new(child.clone()))
                    }
                };
            self.sites.selected.insert(site(node), selected);
            return;
        }
        // The lambda site owns the complete binding or its build diagnostic.
        if node.is_lambda_expr() {
            return;
        }
        self.residual(node);
    }
}

impl PhysicalPreparation<'_, '_> {
    fn residual(&mut self, node: &ast::Expr) {
        let source = self.source;
        if let ast::Expr::If(choice) = node {
            let mut selected = self
                .sites
                .selected
                .remove(&site(node))
                .unwrap_or_else(|| Selection::mixed(node, source));
            let Selection::Mixed { parts, branch, .. } = &mut selected else {
                unreachable!("complete conditional already selected");
            };
            *parts = Some(Box::new([&choice.test, &choice.body, &choice.orelse].map(
                |child| self.input(child, values::registered_value(child, source)),
            )));
            // The condition graph may consume a descendant of a whole value operand.
            for leaf in &branch.operands {
                if let SourceOperand::Residual(expr) = leaf
                    && self.sites.get(expr).is_none()
                {
                    self.visit_expr(expr);
                }
            }
            self.sites.selected.insert(site(node), selected);
            return;
        }
        let selected = if let Some((op, operands)) = values::binary_operands(node) {
            let operation = values::binary(
                op,
                ir::Expr::local(0, ir::Type::PythonObject),
                ir::Expr::local(1, ir::Type::PythonObject),
            );
            self.inputs(
                node,
                operation,
                operands
                    .into_iter()
                    .map(|child| (child, values::registered_value(child, source))),
            )
        } else if let Some(index) = values::index_operands(node, source) {
            self.inputs(node, index.value, index.inputs)
        } else if let Some(object) = source.object_operation(node) {
            let children = std::iter::once(object.target)
                .chain(object.arguments())
                .collect::<Vec<_>>();
            let slots = (0..children.len())
                .map(|i| ir::Expr::local(i, ir::Type::PythonObject))
                .collect();
            let operation = super::operation(object.opcode, ir::Type::PythonObject, slots);
            self.inputs(
                node,
                operation,
                children
                    .into_iter()
                    .map(|child| (child, values::registered_value(child, source))),
            )
        } else {
            visitor::walk_expr(self, node);
            return;
        };
        match (self.sites.selected.get_mut(&site(node)), selected) {
            // The value of a mixed `not` consumes an object operand. Its condition
            // still owns the original short-circuit graph.
            (Some(Selection::Mixed { physical, .. }), Selection::Inputs(inputs)) => {
                *physical = Some(inputs)
            }
            (_, selected) => {
                self.sites.selected.insert(site(node), selected);
            }
        }
    }

    fn inputs<'a>(
        &mut self,
        node: &ast::Expr,
        operation: ir::Expr,
        inputs: impl IntoIterator<Item = (&'a ast::Expr, Option<ir::Expr>)>,
    ) -> Selection {
        let children = inputs
            .into_iter()
            .map(|(child, ordinary)| self.input(child, ordinary))
            .collect();
        Selection::Inputs(Box::new(PhysicalInputs {
            operation,
            position: imre_ast::expression_position(
                node,
                self.source.method_imports,
                self.source.private_names,
            ),
            children,
        }))
    }

    fn input(&mut self, child: &ast::Expr, ordinary: Option<ir::Expr>) -> PreparedInput {
        if let Some(value) = ordinary {
            return SourceOperand::Checked(value);
        }
        let suspended = values::suspended_operand(child, self.source);
        self.visit_expr(child);
        SourceOperand::Residual(ResidualInput {
            expression: Box::new(child.clone()),
            suspended,
        })
    }
}

/// Checked operations and their irreducible inputs, owned by a plan or borrowed at a site.
#[derive(Clone, Copy)]
pub(crate) enum SourceOperand<C, S> {
    Checked(C),
    Residual(S),
}

pub(crate) type BorrowedOperand<'a> = SourceOperand<&'a ir::Expr, &'a ast::Expr>;

impl<'a> BorrowedOperand<'a> {
    fn prepared(node: &'a ast::Expr, value: Option<&'a ir::Expr>) -> Self {
        match value {
            Some(value) => Self::Checked(value),
            None => Self::Residual(node),
        }
    }
}
