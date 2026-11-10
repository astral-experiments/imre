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

fn conditional_parts<E: BranchShape>(node: &E) -> Option<[&E; 3]> {
    match node.split(Context::Condition) {
        Split::Conditional(test, yes, no) => Some([test, yes, no]),
        _ => None,
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
    Comparison(Vec<Option<CheckedChild>>),
    Mixed {
        parts: Option<Box<[Option<ir::Expr>; 3]>>,
        branch: PreparedSteps,
        value: Option<PreparedSteps>,
        physical: Option<Box<PhysicalInputs>>,
    },
}

/// A physical owner supplies any remaining inputs before the checked operation runs.
pub(crate) struct PhysicalInputs {
    pub(crate) operation: ir::Expr,
    pub(crate) position: TextSize,
    children: Box<[Option<CheckedChild>]>,
    /// A subscript with an incomplete slice key evaluates its receiver and then
    /// the individual bounds. Other expressions consume their immediate children.
    split_slice: bool,
}

fn site(node: &ast::Expr) -> (TextRange, ast::NodeKind) {
    (node.range(), ast::AnyNodeRef::from(node).kind())
}

fn physical_children(node: &ast::Expr, split_slice: bool) -> Vec<&ast::Expr> {
    #[derive(Default)]
    struct Inputs<'a>(Vec<&'a ast::Expr>);
    impl<'a> Visitor<'a> for Inputs<'a> {
        fn visit_expr(&mut self, child: &'a ast::Expr) {
            self.0.push(child);
        }
    }
    let mut inputs = Inputs::default();
    if split_slice {
        let index = node.as_subscript_expr().expect("prepared subscript");
        inputs.visit_expr(&index.value);
        visitor::walk_expr(&mut inputs, &index.slice);
    } else {
        visitor::walk_expr(&mut inputs, node);
    }
    inputs.0
}

impl PhysicalInputs {
    pub(crate) fn operands<'a>(
        &'a self,
        node: &'a ast::Expr,
        suspended: bool,
    ) -> impl Iterator<Item = SourceOperand<'a>> + 'a {
        let mut physical = None;
        self.children.iter().enumerate().map(move |(i, child)| {
            if let Some(value) = child.as_ref().and_then(|child| child.value(suspended)) {
                SourceOperand::Checked(value)
            } else if let ir::ExprKind::Builtin(ir::Builtin::RegisteredCall(call), _) =
                &self.operation.kind
            {
                // A method consumes its receiver. The frame performs descriptor lookup
                // before it starts unpacking arguments.
                let original = node.as_call_expr().expect("prepared call");
                let input = if i != 0 {
                    values::registered_argument_at(&original.arguments, i - 1)
                } else if matches!(call.target, ir::RegisteredCallTarget::Method { .. }) {
                    &original
                        .func
                        .as_attribute_expr()
                        .expect("prepared method")
                        .value
                } else {
                    &original.func
                };
                SourceOperand::Residual(input)
            } else {
                SourceOperand::Residual(
                    physical.get_or_insert_with(|| physical_children(node, self.split_slice))[i],
                )
            }
        })
    }
}

pub(crate) struct CheckedChild {
    value: ir::Expr,
    requires_suspension: bool,
}

impl CheckedChild {
    fn prepare(node: &ast::Expr, source: RegisteredSource<'_>) -> Option<Self> {
        Self::from_ordinary(node, source, values::registered_value(node, source))
    }

    fn from_ordinary(
        node: &ast::Expr,
        source: RegisteredSource<'_>,
        ordinary: Option<ir::Expr>,
    ) -> Option<Self> {
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
    fn inputs<'a>(
        &'a self,
        node: &'a ast::Expr,
        context: Context,
    ) -> Option<Vec<SourceOperand<'a>>> {
        let mut pending = vec![node];
        let mut inputs = Vec::with_capacity(self.operands.len());
        while let Some(node) = pending.pop() {
            match node.split(context) {
                Split::Operands(_, children) => pending.extend(children),
                Split::Pair(_, left, right) => pending.extend([left, right]),
                Split::Conditional(test, yes, no) => pending.extend([test, yes, no]),
                Split::Not(child) => pending.push(child),
                Split::Leaf => inputs.push(SourceOperand::prepared(
                    node,
                    self.operands.get(inputs.len())?.as_ref(),
                )),
            }
        }
        (inputs.len() == self.operands.len()).then_some(inputs)
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

    pub(crate) fn value<'a>(
        &'a self,
        node: &'a ast::Expr,
    ) -> Option<(Edges, Vec<SourceOperand<'a>>)> {
        matches!(node.split(Context::Value), Split::Operands(..))
            .then(|| prepare_boolean(node, Context::Value, self))
    }

    pub(crate) fn conditional<'a>(&'a self, node: &'a ast::Expr) -> Option<[SourceOperand<'a>; 3]> {
        let children = conditional_parts(node)?;
        Some(match self.selected.get(&site(node)) {
            Some(Selection::Checked(value)) => {
                conditional_parts(value)?.map(SourceOperand::Checked)
            }
            Some(Selection::Mixed {
                parts: Some(parts), ..
            }) => std::array::from_fn(|i| SourceOperand::prepared(children[i], parts[i].as_ref())),
            _ => children.map(SourceOperand::Residual),
        })
    }

    pub(crate) fn comparison<'a>(
        &'a self,
        node: &'a ast::Expr,
        suspended: bool,
    ) -> Option<(
        &'a [ast::CmpOp],
        impl Iterator<Item = SourceOperand<'a>> + 'a,
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
            SourceOperand::prepared(child, value)
        });
        Some((&cmp.ops, operands))
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
        let mixed = |parts| Selection::Mixed {
            parts,
            physical: None,
            branch: PreparedSteps::new(node, Context::Condition, source),
            value: node
                .is_bool_op_expr()
                .then(|| PreparedSteps::new(node, Context::Value, source)),
        };
        let selected = if let Some(children) = conditional_parts(node) {
            match children.map(|part| values::registered_value(part, source)) {
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
            Some(Selection::Mixed {
                physical: Some(_), ..
            }) => return,
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
                false,
            )
        } else if let Some(index) = values::index_operands(node, source) {
            self.inputs(node, index.value, index.inputs, index.split_slice)
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
                false,
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
        split_slice: bool,
    ) -> Selection {
        let children = inputs
            .into_iter()
            .map(|(child, ordinary)| {
                let checked = CheckedChild::from_ordinary(child, self.source, ordinary);
                if checked
                    .as_ref()
                    .is_none_or(|value| value.requires_suspension)
                {
                    self.visit_expr(child);
                }
                checked
            })
            .collect();
        Selection::Inputs(Box::new(PhysicalInputs {
            operation,
            position: imre_ast::expression_position(
                node,
                self.source.method_imports,
                self.source.private_names,
            ),
            children,
            split_slice,
        }))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SourceOperand<'a> {
    Checked(&'a ir::Expr),
    Residual(&'a ast::Expr),
}

impl<'a> SourceOperand<'a> {
    fn prepared(node: &'a ast::Expr, value: Option<&'a ir::Expr>) -> Self {
        match value {
            Some(value) => Self::Checked(value),
            None => Self::Residual(node),
        }
    }
}

/// The source adapter owns each irreducible operation. Only successor indices and
/// evaluated values cross into the LLVM frame.
pub(crate) fn prepare_boolean<'a>(
    expression: &'a ast::Expr,
    context: Context,
    sites: &'a ExpressionSites,
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
        .map(|leaf| match sites.get(leaf) {
            Some(Selection::Checked(value)) => SourceOperand::Checked(value),
            _ => SourceOperand::Residual(leaf),
        })
        .collect();
    (edges, leaves)
}
