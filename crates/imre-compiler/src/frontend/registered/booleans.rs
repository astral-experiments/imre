//! Prepare expression inputs owned by residual source suites.
use super::values::{self, RegisteredSource};
use super::{PhysicalOperand, PreparedRefusal, RegionRoot, RegisteredOperand, SuitePreparation};
use crate::ir;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange};
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
}

impl Selection {
    /// Value and branch emission bind the same physical inputs in operation order.
    fn refusal(&self, prepare: &SuitePreparation<'_>) -> Option<PreparedRefusal> {
        match self {
            Self::Refused(range, reason) => Some(PreparedRefusal::Operand((*range, reason))),
            Self::Checked(value) => prepare.literal_refusal(RegionRoot::Operand(value)),
            Self::Operand(operand) => prepare
                .operand_refusals(&operand.inputs)
                .or_else(|| prepare.literal_refusal(RegionRoot::Operand(&operand.value))),
        }
    }
}

#[cfg(test)]
impl Selection {
    pub(crate) fn physical(&self) -> Option<&RegisteredOperand> {
        match self {
            Self::Operand(operand) => Some(operand),
            _ => None,
        }
    }

    pub(crate) fn replace_residual_input(&mut self, at: usize, value: ir::Expr) {
        let Self::Operand(operand) = self else {
            panic!("expected a checked operation with bound inputs");
        };
        assert!(matches!(operand.inputs[at], PhysicalOperand::Residual(_)));
        operand.inputs[at] = PhysicalOperand::Checked(Box::new(RegisteredOperand::with_inputs(
            value,
            Box::default(),
        )));
    }
}

#[cfg(test)]
impl RegisteredOperand {
    pub(crate) fn operands(
        &self,
    ) -> impl Iterator<Item = SourceOperand<&ir::Expr, ExpressionInput<'_>>> {
        self.inputs.iter().map(|child| match child {
            PhysicalOperand::Checked(value) => SourceOperand::Checked(&value.value),
            PhysicalOperand::Residual(input) => SourceOperand::Residual(input.as_input()),
        })
    }
}

impl ExpressionSites {
    fn governs(node: &ast::Expr) -> bool {
        matches!(node, ast::Expr::BoolOp(_) | ast::Expr::If(_))
            || matches!(node, ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not)
    }

    pub(super) fn register_lexical(&mut self, node: ast::AnyNodeRef<'_>, selected: Rc<Selection>) {
        self.lexical.insert((node.range(), node.kind()), selected);
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
        self.register_lexical(node.into(), Rc::new(selection));
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
}

impl SuitePreparation<'_> {
    /// Only a complete, occurrence-owned expression can move into the consuming
    /// frame. Shared lexical sites and operands with private slots stay in place.
    pub(super) fn bind_value(
        &self,
        inputs: &mut Vec<PhysicalOperand>,
        input: PhysicalOperand,
    ) -> ir::Expr {
        if let PhysicalOperand::Residual(mut occurrence) = input {
            if let Some(Selection::Checked(value)) = Rc::get_mut(&mut occurrence.selected)
                && self.literal_refusal(RegionRoot::Operand(value)).is_none()
            {
                return std::mem::replace(value, ir::Expr::none());
            }
            super::bind_operand(
                inputs,
                PhysicalOperand::Residual(occurrence),
                ir::Type::PythonObject,
            )
        } else {
            super::bind_operand(inputs, input, ir::Type::PythonObject)
        }
    }

    pub(super) fn owned_input(
        &self,
        expr: &ast::Expr,
        scope: super::RegisteredScope,
        suspended: bool,
    ) -> OwnedExpression {
        PhysicalPreparation {
            suite: self,
            scope,
            suspended,
        }
        .owned(expr)
    }
}

struct PhysicalPreparation<'a, 's> {
    suite: &'a SuitePreparation<'s>,
    scope: super::RegisteredScope,
    suspended: bool,
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
            self.suite
                .sites
                .expressions
                .lexical
                .get(&(node.range(), kind))
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

    fn checked_value(&self, node: &ast::Expr) -> Option<ir::Expr> {
        if self.suspended {
            values::suspended_operand(node, self.suite.source)
        } else {
            values::registered_value(node, self.suite.source)
        }
    }

    fn select_value(&mut self, node: &ast::Expr) -> Option<Selection> {
        if !node.is_t_string_expr()
            && let Some(value) = self.checked_value(node)
        {
            return Some(Selection::Checked(value));
        }
        self.select_remaining(node)
    }

    fn select_remaining(&mut self, node: &ast::Expr) -> Option<Selection> {
        if !self.suspended && ExpressionSites::governs(node) {
            return self.residual(node);
        }
        match super::inline_operand(node, self.suite.source, self.scope, self.suspended) {
            Ok(Some(operand)) => return Some(Selection::Operand(Box::new(operand))),
            Err((range, reason)) => return Some(Selection::Refused(range, reason)),
            Ok(None) => {}
        }
        if self.suspended
            && let Some((kind, Some(child))) = ir::SuspendValue::operand(node)
        {
            let selected = match super::RegisteredOperand::prepare_with(
                self.suite.source,
                self.scope,
                |scope| scope.with_suspending_operands(|scope| scope.operand(node)),
            ) {
                Some(operand) => Selection::Operand(Box::new(operand)),
                None => {
                    let input = self.ordinary_input(child);
                    let value = super::operation(
                        ir::Builtin::RegisteredSuspend(kind, node.range()),
                        ir::Type::PythonObject,
                        vec![ir::Expr::registered_input(0, ir::Type::PythonObject)],
                    );
                    Selection::Operand(self.inputs(node, value, Box::new([input])))
                }
            };
            return Some(selected);
        }
        // The lambda site owns the complete binding or its build diagnostic.
        if node.is_lambda_expr() {
            return None;
        }
        self.residual(node)
    }

    fn ordinary_input(&mut self, node: &ast::Expr) -> PhysicalOperand {
        self.input(node, self.checked_value(node))
    }

    /// Keep control flow in the checked operation and bind each leaf at its own use.
    fn logical_operation(
        &mut self,
        node: &ast::Expr,
        inputs: &mut Vec<PhysicalOperand>,
    ) -> ir::Expr {
        if ExpressionSites::governs(node) || node.is_compare_expr() {
            let value = bound_operation(node, self.suite.source, &mut |child| {
                self.logical_operation(child, inputs)
            })
            .expect("a logical or comparison expression has a checked operation");
            return self.suite.source.located(node, value);
        }
        let checked = match self.checked_value(node) {
            Some(value) if ir::control::boolean_literal(&value).is_some() => return value,
            checked => checked,
        };
        let input = self.input(node, checked);
        self.suite.source.located(
            node,
            super::bind_operand(inputs, input, ir::Type::PythonObject),
        )
    }

    fn residual(&mut self, node: &ast::Expr) -> Option<Selection> {
        let source = self.suite.source;
        if ExpressionSites::governs(node) {
            let mut inputs = Vec::new();
            let operation = self.logical_operation(node, &mut inputs);
            return Some(Selection::Operand(self.inputs(
                node,
                operation,
                inputs.into_boxed_slice(),
            )));
        }
        let mut children = Vec::new();
        let op = if let Some(value) = values::index_operation(node, source, |child, ordinary| {
            let value = ordinary.or_else(|| {
                self.suspended
                    .then(|| values::suspended_operand(child, source))
                    .flatten()
            });
            super::bind_operand(
                &mut children,
                self.input(child, value),
                ir::Type::PythonObject,
            )
        }) {
            value
        } else {
            bound_operation(node, source, &mut |child| {
                self.bound_value(child, &mut children)
            })?
        };
        if children.is_empty() {
            return Some(Selection::Checked(source.located_operation(node, op)));
        }
        Some(Selection::Operand(self.inputs(
            node,
            op,
            children.into_boxed_slice(),
        )))
    }

    /// Check the original child before placing it. A mixed child keeps its own
    /// frame, and a refused child keeps its position among the physical inputs.
    fn bound_value(&mut self, node: &ast::Expr, inputs: &mut Vec<PhysicalOperand>) -> ir::Expr {
        if let Some(value) = self.checked_value(node) {
            if self
                .suite
                .literal_refusal(RegionRoot::Operand(&value))
                .is_none()
            {
                return value;
            }
            let input = self.input(node, Some(value));
            return super::bind_operand(inputs, input, ir::Type::PythonObject);
        }
        let child = PhysicalOperand::Residual(self.occurrence(node, Self::select_remaining));
        self.suite.bind_value(inputs, child)
    }

    fn inputs(
        &self,
        node: &ast::Expr,
        operation: ir::Expr,
        children: Box<[PhysicalOperand]>,
    ) -> Box<RegisteredOperand> {
        Box::new(RegisteredOperand::with_inputs(
            self.suite.source.located_operation(node, operation),
            children,
        ))
    }

    fn input(&mut self, child: &ast::Expr, checked: Option<ir::Expr>) -> PhysicalOperand {
        match checked {
            Some(value) => PhysicalOperand::Checked(Box::new(RegisteredOperand::with_inputs(
                value,
                Box::default(),
            ))),
            None => PhysicalOperand::Residual(self.occurrence(child, Self::select_remaining)),
        }
    }
}

/// Bind a physical input at its use in the ordinary Python operation.
fn bound_operation(
    node: &ast::Expr,
    source: RegisteredSource<'_>,
    child: &mut dyn FnMut(&ast::Expr) -> ir::Expr,
) -> Option<ir::Expr> {
    let Ok(value) = values::operation(
        node,
        source,
        None,
        |_, _, _| true,
        ir::control::boolean_literal,
        |_| true,
        |node| Ok::<_, std::convert::Infallible>(child(node)),
    );
    value
}

/// Checked operations and their irreducible inputs, owned by a plan or borrowed at a site.
#[derive(Clone, Copy)]
pub(crate) enum SourceOperand<C, S> {
    Checked(C),
    Residual(S),
}

impl<C> SourceOperand<C, OwnedExpression> {
    pub(super) fn refusal(
        &self,
        prepare: &SuitePreparation<'_>,
        value: impl FnOnce(&C) -> &ir::Expr,
    ) -> Option<PreparedRefusal> {
        match self {
            Self::Checked(checked) => prepare.literal_refusal(RegionRoot::Operand(value(checked))),
            Self::Residual(input) => input.selected.refusal(prepare),
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
#[cfg(test)]
pub(crate) type ExpressionInput<'a> = Occurrence<&'a Selection>;

#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_bare_target_evaluates_each_bound_under_its_discard_mark() {
        use super::super::{PhysicalOperand, RegisteredScope, SuitePreparation, SuiteSelection};
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
        let mut preparation = SuitePreparation::new(&operations, source, true);
        let suite = preparation.physical_suite(
            statements,
            RegisteredScope::Module,
            SuiteSelection::Physical,
            false,
        );
        let inputs = &suite.operands;
        assert!(matches!(
            inputs.as_slice(),
            [
                PhysicalOperand::Checked(_),
                PhysicalOperand::Residual(_),
                PhysicalOperand::Checked(_)
            ]
        ));
        assert!(
            matches!(suite.checked.function.body.as_slice(), [.., ir::Statement::Expr(value)]
            if matches!(&value.kind, ir::ExprKind::Builtin(ir::Builtin::RegisteredDiscard, slots)
                if slots.len() == inputs.len() && slots.iter().enumerate().all(|(slot, input)|
                    matches!(input.kind, ir::ExprKind::Builtin(ir::Builtin::RegisteredInput(i), _) if i == slot))))
        );
    }

    #[test]
    fn forwarded_names_sharing_a_source_range_keep_their_own_selected_inputs() {
        use crate::frontend::keyword_forwarding;
        use crate::modules::SourceGraph;
        use ast::HasNodeIndex;

        let fixture = include_str!("../../../../../tests/fixtures/calls/forwarded_keywords.py");
        let graph = SourceGraph::single(fixture).unwrap();
        let module = &graph.modules[graph.entry];
        let text = &module.source;
        let mut forward = std::borrow::Cow::Borrowed(
            module.parsed.syntax().body[1]
                .as_function_def_stmt()
                .unwrap(),
        );
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
                .all(|expr| expr.node_index().load().as_u32().is_none())
        );

        let context = values::ModuleSource::new(module);
        let source = context.context(text);
        let operations = HashMap::new();
        let suite = SuitePreparation::new(&operations, source, true);
        let mut prepare = PhysicalPreparation {
            suite: &suite,
            scope: super::super::RegisteredScope::Function { is_async: false },
            suspended: false,
        };
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
        let operations = HashMap::new();
        let suite = SuitePreparation::new(&operations, context.context(text), true);
        let mut prepare = PhysicalPreparation {
            suite: &suite,
            scope: super::super::RegisteredScope::Module,
            suspended: false,
        };
        let Selection::Operand(inputs) = prepare.select_value(&root).unwrap() else {
            panic!("expected a checked logical operation with prepared inputs");
        };
        let mut ops = Vec::new();
        ir::walk_operand_expressions(&inputs.value, &mut |expr| {
            if let ir::ExprKind::Compare(_, comparison) = &expr.kind {
                ops.extend_from_slice(comparison);
            }
        });
        assert_eq!(ops, [ast::CmpOp::Lt, ast::CmpOp::Gt]);
        assert_eq!(inputs.inputs.len(), 4);
        for (slot, expected_name) in [(1, "before"), (3, "after")] {
            let SourceOperand::Checked(name) = inputs.operands().nth(slot).unwrap() else {
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
