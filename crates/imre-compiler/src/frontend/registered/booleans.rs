//! Prepare expression inputs owned by residual source suites.
use super::values::{self, RegisteredSource};
use crate::ir;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::collections::HashMap;
use std::rc::Rc;

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
}

impl Selection {
    #[cfg(test)]
    pub(crate) fn physical(&self) -> Option<&PhysicalInputs> {
        match self {
            Self::Inputs(inputs) => Some(inputs),
            _ => None,
        }
    }

    /// Value and branch emission bind the same physical inputs in operation order.
    fn refusal(&self) -> Option<super::OperandRefusal> {
        match self {
            Self::Refused(range, reason) => Some((*range, reason)),
            Self::Checked(_) | Self::Operand(_) => None,
            Self::Inputs(inputs) => inputs.children.iter().find_map(PreparedInput::refusal),
        }
    }

    #[cfg(test)]
    pub(crate) fn replace_residual_input(&mut self, at: usize, value: ir::Expr) {
        let Self::Inputs(inputs) = self else {
            panic!("expected prepared mixed operands");
        };
        assert!(matches!(inputs.children[at], SourceOperand::Residual(_)));
        inputs.children[at] = SourceOperand::Checked(value);
    }
}

enum InputEvaluation {
    BeforeOperation,
    AtUse,
}

/// The physical owner supplies the operands of a checked operation.
pub(crate) struct PhysicalInputs {
    pub(crate) operation: ir::Expr,
    pub(crate) position: TextSize,
    children: Box<[PreparedInput]>,
    evaluation: InputEvaluation,
}

pub(crate) type PreparedInput = SourceOperand<ir::Expr, OwnedExpression>;

impl PhysicalInputs {
    pub(crate) fn operand(&self, slot: usize) -> BorrowedOperand<'_> {
        self.children[slot].operand()
    }

    pub(crate) fn eager_operands(&self) -> Option<impl Iterator<Item = BorrowedOperand<'_>>> {
        matches!(self.evaluation, InputEvaluation::BeforeOperation).then(|| self.operands())
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
                    None => Selection::Inputs(self.slotted_operation(
                        node,
                        ir::Builtin::RegisteredSuspend(kind, node.range()),
                        std::iter::once(child),
                    )),
                };
            return Some(selected);
        }
        // The lambda site owns the complete binding or its build diagnostic.
        if node.is_lambda_expr() {
            return None;
        }
        self.residual(node)
    }

    fn ordinary_input(&mut self, node: &ast::Expr) -> PreparedInput {
        self.input(node, values::registered_value(node, self.source))
    }

    /// Keep control flow in the checked operation and bind each leaf at its own use.
    fn logical_operation(&mut self, node: &ast::Expr, inputs: &mut Vec<PreparedInput>) -> ir::Expr {
        if ExpressionSites::governs(node) || node.is_compare_expr() {
            let value = values::operation(
                node,
                self.source,
                None,
                false,
                |_| true,
                |child| Ok::<_, std::convert::Infallible>(self.logical_operation(child, inputs)),
            )
            .unwrap()
            .expect("a logical or comparison expression has a checked operation");
            return self.source.located(node, value);
        }
        let slot = inputs.len();
        inputs.push(self.ordinary_input(node));
        self.source
            .located(node, ir::Expr::local(slot, ir::Type::PythonObject))
    }

    fn residual(&mut self, node: &ast::Expr) -> Option<Selection> {
        let source = self.source;
        if ExpressionSites::governs(node) {
            let mut inputs = Vec::new();
            let operation = self.logical_operation(node, &mut inputs);
            return Some(Selection::Inputs(self.inputs(
                node,
                operation,
                inputs.into_boxed_slice(),
                InputEvaluation::AtUse,
            )));
        }
        if let ast::Expr::Compare(cmp) = node {
            return Some(Selection::Inputs(self.slotted_operation(
                node,
                ir::Builtin::RegisteredCompare(cmp.ops.to_vec()),
                cmp.operands.iter(),
            )));
        }
        let (op, children) = if let Some((op, operands)) = values::binary_operands(node) {
            let operation = values::binary(
                op,
                ir::Expr::local(0, ir::Type::PythonObject),
                ir::Expr::local(1, ir::Type::PythonObject),
            );
            (
                operation,
                operands
                    .into_iter()
                    .map(|child| self.ordinary_input(child))
                    .collect(),
            )
        } else if let Some(index) = values::index_operands(node, source) {
            (
                index.value,
                index
                    .inputs
                    .into_iter()
                    .map(|(child, ordinary)| self.input(child, ordinary))
                    .collect(),
            )
        } else if let Some(object) = source.object_operation(node) {
            let operands = std::iter::once(object.target).chain(object.arguments());
            return Some(Selection::Inputs(self.slotted_operation(
                node,
                object.opcode,
                operands,
            )));
        } else {
            return None;
        };
        Some(Selection::Inputs(self.inputs(
            node,
            op,
            children,
            InputEvaluation::BeforeOperation,
        )))
    }

    fn slotted_operation<'n>(
        &mut self,
        node: &ast::Expr,
        opcode: ir::Builtin,
        operands: impl Iterator<Item = &'n ast::Expr>,
    ) -> Box<PhysicalInputs> {
        let children: Box<[_]> = operands.map(|child| self.ordinary_input(child)).collect();
        let slots = (0..children.len())
            .map(|slot| ir::Expr::local(slot, ir::Type::PythonObject))
            .collect();
        let operation = super::operation(opcode, ir::Type::PythonObject, slots);
        self.inputs(node, operation, children, InputEvaluation::AtUse)
    }

    fn inputs(
        &self,
        node: &ast::Expr,
        operation: ir::Expr,
        children: Box<[PreparedInput]>,
        evaluation: InputEvaluation,
    ) -> Box<PhysicalInputs> {
        Box::new(PhysicalInputs {
            operation,
            position: imre_ast::expression_position(
                node,
                self.source.method_imports,
                self.source.private_names,
            ),
            children,
            evaluation,
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

impl<C> SourceOperand<C, OwnedExpression> {
    pub(super) fn refusal(&self) -> Option<super::OperandRefusal> {
        match self {
            Self::Checked(_) => None,
            Self::Residual(input) => input.selected.refusal(),
        }
    }
}

/// A physical child owns its selection. The range describes its Python location;
/// different generated occurrences can share both a range and a syntax kind.
#[derive(Clone, Copy)]
pub(crate) struct Occurrence<V> {
    pub(crate) range: TextRange,
    pub(crate) selected: V,
}

pub(crate) type OwnedExpression = Occurrence<Rc<Selection>>;
pub(crate) type ExpressionInput<'a> = Occurrence<&'a Selection>;

impl OwnedExpression {
    pub(crate) fn as_input(&self) -> ExpressionInput<'_> {
        Occurrence {
            range: self.range,
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
    fn a_partial_bare_target_evaluates_each_bound_under_its_discard_mark() {
        use super::super::{
            PhysicalOperand, RegisteredScope, SourceEffect, SuitePreparation, SuiteSelection,
        };
        let text = "owner()[(lambda: start())():stop()]: Unused\n";
        let graph = crate::modules::SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let statements = &module.parsed.syntax().body;
        let context = values::ModuleSource::new(module);
        // Without a prepared lambda site, only the lower bound needs an external operand.
        let missing = HashMap::new();
        let source = RegisteredSource {
            lambdas: Some(&missing),
            ..context.context(text)
        };
        let operations = HashMap::new();
        let mut preparation = SuitePreparation::new(&operations, source, statements, true);
        let suite = preparation.physical_suite(
            statements,
            RegisteredScope::Module,
            SuiteSelection::Physical,
            false,
        );
        let [site] = &suite.residual[..] else {
            panic!("expected one partial annotation")
        };
        let Ok(SourceEffect::Value(statement, inputs)) = &site.effect else {
            panic!("expected a checked statement")
        };
        assert!(matches!(
            &inputs[..],
            [
                PhysicalOperand::Checked(_),
                PhysicalOperand::Residual(_),
                PhysicalOperand::Checked(_)
            ]
        ));
        assert!(matches!(statement.as_ref(), ir::Statement::Expr(value)
            if matches!(&value.kind, ir::ExprKind::Builtin(ir::Builtin::RegisteredDiscard, slots)
                if slots.len() == inputs.len())));
    }

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
    fn logical_inputs_keep_distinct_comparisons_at_the_same_range() {
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
        let Selection::Inputs(inputs) = prepare.select_value(&root).unwrap() else {
            panic!("expected a checked logical operation with prepared inputs");
        };
        let mut ops = Vec::new();
        ir::walk_operand_expressions(&inputs.operation, &mut |expr| {
            if let ir::ExprKind::Builtin(ir::Builtin::RegisteredCompare(comparison), _) = &expr.kind
            {
                ops.extend_from_slice(comparison);
            }
        });
        assert_eq!(ops, [ast::CmpOp::Lt, ast::CmpOp::Gt]);
        assert_eq!(inputs.children.len(), 4);
        for (slot, expected_name) in [(1, "before"), (3, "after")] {
            let SourceOperand::Checked(name) = inputs.operand(slot) else {
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
