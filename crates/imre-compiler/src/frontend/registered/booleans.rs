//! Prepare expression inputs owned by residual source suites.
use super::values::{self, RegisteredSource};
use crate::ir;
use ir::control::{BranchShape, Context, Depth, Edges, Split, edges};
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::collections::HashMap;
use std::rc::Rc;

impl BranchShape for ast::Expr {
    fn split(&self, context: Context) -> Split<'_, Self> {
        match self {
            Self::BoolOp(op) => Split::Operands(op.op == ast::BoolOp::And, &op.values),
            Self::If(test) => Split::Conditional(&test.test, &test.body, &test.orelse),
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
    lexical: HashMap<(TextRange, ast::NodeKind), Rc<Selection>>,
}

pub(crate) enum Selection {
    Checked(ir::Expr),
    Operand(Box<super::RegisteredOperand>),
    Refused(TextRange, &'static str),
    Inputs(Box<PhysicalInputs>),
    /// Only the child remains physical. It runs under the suspension's operand mark.
    Suspension(ir::SuspendValue, OwnedExpression),
    Comparison {
        ops: Box<[ast::CmpOp]>,
        operands: Box<[LogicalOperand]>,
    },
    Mixed {
        parts: Option<Box<[LogicalOperand; 3]>>,
        branch: PreparedSteps,
        value: Option<PreparedSteps>,
        physical: Option<Box<PhysicalInputs>>,
    },
}

impl Selection {
    pub(crate) fn physical(&self) -> Option<&PhysicalInputs> {
        match self {
            Self::Inputs(inputs)
            | Self::Mixed {
                physical: Some(inputs),
                ..
            } => Some(inputs),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn replace_residual_input(&mut self, at: usize, value: ir::Expr) {
        let inputs: &mut [LogicalOperand] = match self {
            Self::Inputs(inputs)
            | Self::Mixed {
                physical: Some(inputs),
                ..
            } => &mut inputs.children,
            Self::Mixed {
                parts: Some(parts), ..
            } => parts.as_mut_slice(),
            _ => panic!("expected prepared mixed operands"),
        };
        assert!(matches!(inputs[at].as_ref(), SourceOperand::Residual(_)));
        inputs[at] = Rc::new(SourceOperand::Checked(value));
    }
}

/// A physical owner supplies any remaining inputs before the checked operation runs.
pub(crate) struct PhysicalInputs {
    pub(crate) operation: ir::Expr,
    pub(crate) position: TextSize,
    children: Box<[LogicalOperand]>,
}

pub(crate) type PreparedInput = SourceOperand<ir::Expr, OwnedExpression>;

impl PhysicalInputs {
    pub(crate) fn operand(&self, slot: usize) -> BorrowedOperand<'_> {
        self.children[slot].operand()
    }

    pub(crate) fn operands(&self) -> impl Iterator<Item = BorrowedOperand<'_>> {
        self.children.iter().map(|child| child.operand())
    }
}

impl PreparedInput {
    pub(crate) fn operand(&self) -> BorrowedOperand<'_> {
        match self {
            Self::Checked(value) => SourceOperand::Checked(value),
            Self::Residual(input) => SourceOperand::Residual(input.as_input()),
        }
    }
}

/// A projection shares the same physical operand as every other use of this occurrence.
pub(crate) type LogicalOperand = Rc<PreparedInput>;

pub(crate) struct PreparedSteps {
    pub(crate) edges: Edges,
    pub(crate) operands: Vec<LogicalOperand>,
}

impl ExpressionSites {
    fn governs(node: &ast::Expr) -> bool {
        matches!(node, ast::Expr::BoolOp(_) | ast::Expr::If(_))
            || matches!(node, ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not)
    }

    pub(super) fn register_lexical(&mut self, node: &ast::Expr, selected: Rc<Selection>) {
        self.lexical
            .insert((node.range(), ast::AnyNodeRef::from(node).kind()), selected);
    }

    /// A template owns its lexical operand and any refusal in its interpolation.
    pub(super) fn prepare_template(
        &mut self,
        node: &ast::Expr,
        source: RegisteredSource<'_>,
        owner: super::RegisteredScope,
    ) {
        let operand = super::RegisteredOperand::prepare_required(
            node,
            source,
            owner,
            "cannot compile this template ahead of time",
            |scope| scope.report_child_refusals = true,
        );
        let selection = match operand {
            Ok(operand) => Selection::Operand(Box::new(operand)),
            Err((range, reason)) => Selection::Refused(range, reason),
        };
        self.register_lexical(node, Rc::new(selection));
    }

    #[cfg(test)]
    pub(crate) fn templates(&self) -> impl Iterator<Item = (TextRange, &Selection)> {
        self.lexical.iter().filter_map(|(&(range, kind), value)| {
            (kind == ast::NodeKind::ExprTString).then_some((range, value.as_ref()))
        })
    }

    #[cfg(test)]
    pub(crate) fn take_lexical(&mut self) -> Self {
        std::mem::take(self)
    }

    pub(super) fn owned_input(
        &self,
        expr: &ast::Expr,
        source: RegisteredSource<'_>,
        scope: super::RegisteredScope,
        suspended: bool,
    ) -> OwnedExpression {
        PhysicalPreparation::new(self, source, scope, suspended).owned(expr)
    }
}

struct PhysicalPreparation<'a, 's> {
    sites: &'a ExpressionSites,
    source: RegisteredSource<'s>,
    scope: super::RegisteredScope,
    suspended: bool,
    // The source tree stays borrowed for this root. Only the owned operand escapes.
    occurrences: Option<HashMap<*const ast::Expr, LogicalOperand>>,
}

impl<'a, 's> PhysicalPreparation<'a, 's> {
    fn new(
        sites: &'a ExpressionSites,
        source: RegisteredSource<'s>,
        scope: super::RegisteredScope,
        suspended: bool,
    ) -> Self {
        Self {
            sites,
            source,
            scope,
            suspended,
            occurrences: None,
        }
    }
}

impl PhysicalPreparation<'_, '_> {
    fn owned(&mut self, node: &ast::Expr) -> OwnedExpression {
        self.occurrence(node, Self::select_value)
    }

    fn occurrence(
        &mut self,
        node: &ast::Expr,
        select: fn(&mut Self, &ast::Expr) -> Option<Selection>,
    ) -> OwnedExpression {
        // Lexical bindings and template fields have already been compiled in their own scope.
        let kind = ast::AnyNodeRef::from(node).kind();
        let lexical = if node.is_lambda_expr() || node.is_t_string_expr() {
            self.sites.lexical.get(&(node.range(), kind))
        } else {
            None
        };
        let selected = match lexical {
            Some(selected) => Rc::clone(selected),
            None => Rc::new(select(self, node).unwrap_or_else(|| {
                Selection::Refused(node.range(), "expression has no checked operand")
            })),
        };
        Occurrence {
            range: node.range(),
            kind,
            selected,
        }
    }

    fn select_value(&mut self, node: &ast::Expr) -> Option<Selection> {
        let region = if self.suspended {
            values::Region::SuspendedOperand
        } else {
            values::Region::Residual
        };
        if !node.is_t_string_expr()
            && let Some(value) = region.operand(node, self.source)
        {
            return Some(Selection::Checked(value));
        }
        self.select_remaining(node)
    }

    fn select_remaining(&mut self, node: &ast::Expr) -> Option<Selection> {
        if !self.suspended && ExpressionSites::governs(node) {
            return self.residual(node);
        }
        match super::inline_operand(node, self.source, self.scope, self.suspended) {
            Ok(Some(operand)) => return Some(Selection::Operand(Box::new(operand))),
            Err((range, reason)) => return Some(Selection::Refused(range, reason)),
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
                    None => Selection::Suspension(kind, self.owned(child)),
                };
            return Some(selected);
        }
        // The lambda site owns the complete binding or its build diagnostic.
        if node.is_lambda_expr() {
            return None;
        }
        self.residual(node)
    }

    fn shared_input(&mut self, node: &ast::Expr) -> LogicalOperand {
        let source = self.source;
        self.share(node, || values::registered_value(node, source))
    }

    fn share(
        &mut self,
        node: &ast::Expr,
        ordinary: impl FnOnce() -> Option<ir::Expr>,
    ) -> LogicalOperand {
        let occurrence = std::ptr::from_ref(node);
        if let Some(input) = self
            .occurrences
            .as_ref()
            .and_then(|inputs| inputs.get(&occurrence))
        {
            return Rc::clone(input);
        }
        let input = Rc::new(self.input(node, ordinary()));
        if let Some(inputs) = &mut self.occurrences {
            inputs.insert(occurrence, Rc::clone(&input));
        }
        input
    }

    fn steps(&mut self, node: &ast::Expr, context: Context) -> PreparedSteps {
        let (edges, leaves) = edges(node, context, Depth::All);
        let operands = leaves
            .into_iter()
            .map(|child| self.shared_input(child))
            .collect();
        PreparedSteps { edges, operands }
    }

    fn residual(&mut self, node: &ast::Expr) -> Option<Selection> {
        let source = self.source;
        if ExpressionSites::governs(node) {
            self.occurrences.get_or_insert_with(HashMap::new);
            let branch = self.steps(node, Context::Condition);
            let value = node
                .is_bool_op_expr()
                .then(|| self.steps(node, Context::Value));
            let parts = node.as_if_expr().map(|choice| {
                Box::new(
                    [&*choice.test, &*choice.body, &*choice.orelse]
                        .map(|part| self.shared_input(part)),
                )
            });
            let physical = node.as_unary_op_expr().map(|unary| {
                let operation = super::operation(
                    ir::Builtin::RegisteredUnary(unary.op.into()),
                    ir::Type::PythonObject,
                    vec![ir::Expr::local(0, ir::Type::PythonObject)],
                );
                let children = vec![self.shared_input(&unary.operand)].into_boxed_slice();
                self.inputs(node, operation, children)
            });
            return Some(Selection::Mixed {
                parts,
                branch,
                value,
                physical,
            });
        }
        if let ast::Expr::Compare(cmp) = node {
            return Some(Selection::Comparison {
                ops: cmp.ops.clone(),
                operands: cmp
                    .operands
                    .iter()
                    .map(|child| self.shared_input(child))
                    .collect(),
            });
        }
        let (operation, children) = if let Some((op, operands)) = values::binary_operands(node) {
            let operation = values::binary(
                op,
                ir::Expr::local(0, ir::Type::PythonObject),
                ir::Expr::local(1, ir::Type::PythonObject),
            );
            (
                operation,
                operands
                    .into_iter()
                    .map(|child| self.shared_input(child))
                    .collect(),
            )
        } else if let Some(index) = values::index_operands(node, source) {
            (
                index.value,
                index
                    .inputs
                    .into_iter()
                    .map(|(child, ordinary)| self.share(child, || ordinary))
                    .collect(),
            )
        } else if let Some(object) = source.object_operation(node) {
            let children: Box<[_]> = std::iter::once(object.target)
                .chain(object.arguments())
                .map(|child| self.shared_input(child))
                .collect();
            let slots = (0..children.len())
                .map(|i| ir::Expr::local(i, ir::Type::PythonObject))
                .collect();
            (
                super::operation(object.opcode, ir::Type::PythonObject, slots),
                children,
            )
        } else {
            return None;
        };
        Some(Selection::Inputs(self.inputs(node, operation, children)))
    }

    fn inputs(
        &self,
        node: &ast::Expr,
        operation: ir::Expr,
        children: Box<[LogicalOperand]>,
    ) -> Box<PhysicalInputs> {
        Box::new(PhysicalInputs {
            operation,
            position: imre_ast::expression_position(
                node,
                self.source.method_imports,
                self.source.private_names,
            ),
            children,
        })
    }

    fn input(&mut self, child: &ast::Expr, ordinary: Option<ir::Expr>) -> PreparedInput {
        let checked = ordinary.or_else(|| {
            self.suspended
                .then(|| values::suspended_operand(child, self.source))
                .flatten()
        });
        match checked {
            Some(value) => SourceOperand::Checked(value),
            None => SourceOperand::Residual(self.occurrence(child, Self::select_remaining)),
        }
    }
}

/// Checked operations and their irreducible inputs, owned by a plan or borrowed at a site.
#[derive(Clone, Copy)]
pub(crate) enum SourceOperand<C, S> {
    Checked(C),
    Residual(S),
}

/// A physical child owns its selection. The range describes its Python location;
/// different generated occurrences can share both a range and a syntax kind.
#[derive(Clone, Copy)]
pub(crate) struct Occurrence<V> {
    pub(crate) range: TextRange,
    pub(crate) kind: ast::NodeKind,
    pub(crate) selected: V,
}

pub(crate) type OwnedExpression = Occurrence<Rc<Selection>>;
pub(crate) type ExpressionInput<'a> = Occurrence<&'a Selection>;

impl OwnedExpression {
    pub(crate) fn as_input(&self) -> ExpressionInput<'_> {
        Occurrence {
            range: self.range,
            kind: self.kind,
            selected: self.selected.as_ref(),
        }
    }
}

impl<V> Ranged for Occurrence<V> {
    fn range(&self) -> TextRange {
        self.range
    }
}

pub(crate) type BorrowedOperand<'a> = SourceOperand<&'a ir::Expr, ExpressionInput<'a>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_names_sharing_a_source_range_keep_their_own_selected_inputs() {
        use crate::frontend::keyword_forwarding;
        use crate::modules::SourceGraph;
        use ast::HasNodeIndex;

        let text = include_str!("../../../../../tests/fixtures/calls/forwarded_keywords.py");
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let mut forward = module.parsed.syntax().body[1]
            .as_function_def_stmt()
            .unwrap()
            .clone();
        assert_eq!(forward.name.as_str(), "forward");
        let defaults = keyword_forwarding::prepare(&graph, graph.entry, &mut forward);
        assert!(defaults.forwarded.is_some());
        let assignment = forward.body[1].as_assign_stmt().unwrap();
        let call = assignment.value.as_call_expr().unwrap();
        let [before, after] = call.arguments.keywords.as_slice() else {
            panic!("the source forwarding expansion produces two keyword loads");
        };
        let loads = [&before.value, &after.value];
        let range = before.value.range();
        assert_eq!(range, TextRange::new(218.into(), 227.into()));
        assert_eq!(&text[range], "**options");
        assert_eq!(after.value.range(), range);
        assert_eq!(
            loads.map(|expr| expr.as_name_expr().unwrap().id.as_str()),
            ["before", "after"]
        );
        assert!(
            loads
                .iter()
                .all(|expr| expr.node_index().load() == ast::AtomicNodeIndex::NONE.load())
        );

        let context = values::ModuleSource::new(module);
        let source = context.context(text);
        let sites = ExpressionSites::default();
        let mut prepare = PhysicalPreparation::new(
            &sites,
            source,
            super::super::RegisteredScope::Function { is_async: false },
            false,
        );
        // Prepare each original normalized occurrence before inspecting either selected load.
        assert!(
            loads
                .iter()
                .all(|expr| values::registered_value(expr, source).is_some())
        );
        let held = loads.map(|expr| prepare.owned(expr));
        for (input, expected) in held.iter().zip(["before", "after"]) {
            let ExpressionInput {
                range: site_range,
                selected: Selection::Checked(value),
                ..
            } = input.as_input()
            else {
                panic!("the operand must retain its own selected load");
            };
            assert_eq!(site_range, range);
            let mut names = Vec::new();
            ir::walk_operand_expressions(value, &mut |child| {
                if let ir::ExprKind::Builtin(ir::Builtin::RegisteredName(name), _) = &child.kind {
                    names.push(name.clone());
                }
            });
            assert_eq!(names, [expected]);
        }
    }

    #[test]
    fn logical_projections_share_occurrences_with_distinct_comparison_operators() {
        let text = "result = ((lambda: 2)() < before) and ((lambda: 3)() > after)\n";
        let graph = crate::modules::SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let context = values::ModuleSource::new(module);
        let mut root = module.parsed.syntax().body[0]
            .as_assign_stmt()
            .unwrap()
            .value
            .clone();
        let [first, second] = root.as_bool_op_expr_mut().unwrap().values.as_mut_slice() else {
            unreachable!()
        };
        let range = first.range();
        second.as_compare_expr_mut().unwrap().range = range;
        let sites = ExpressionSites::default();
        let mut prepare = PhysicalPreparation::new(
            &sites,
            context.context(text),
            super::super::RegisteredScope::Module,
            false,
        );
        let Selection::Mixed {
            branch,
            value: Some(value),
            ..
        } = prepare.select_value(&root).unwrap()
        else {
            panic!("expected both short-circuit projections");
        };
        assert_eq!(branch.operands.len(), 2);
        assert_eq!(value.operands.len(), 2);
        assert!(!Rc::ptr_eq(&branch.operands[0], &branch.operands[1]));
        for ((condition, value), (op, expected_name)) in branch
            .operands
            .iter()
            .zip(&value.operands)
            .rev()
            .zip([(ast::CmpOp::Lt, "before"), (ast::CmpOp::Gt, "after")])
        {
            assert!(Rc::ptr_eq(condition, value));
            let SourceOperand::Residual(input) = condition.operand() else {
                panic!("expected a selected comparison");
            };
            assert_eq!(input.range, range);
            let Selection::Comparison { ops, operands, .. } = input.selected else {
                panic!("expected a comparison");
            };
            assert_eq!(ops.as_ref(), [op]);
            let SourceOperand::Checked(name) = operands[1].operand() else {
                panic!("expected the checked name operand");
            };
            let mut names = Vec::new();
            ir::walk_operand_expressions(name, &mut |child| {
                if let ir::ExprKind::Builtin(ir::Builtin::RegisteredName(name), _) = &child.kind {
                    names.push(name.clone());
                }
            });
            assert_eq!(names, [expected_name]);
        }
    }
}
