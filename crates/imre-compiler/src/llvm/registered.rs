//! Checked operations inside a Python function with a registered frame and object arguments.

mod abi;
pub(crate) mod context;
mod frame;
mod handlers;
pub(crate) mod patterns;
mod sequences;
pub(crate) mod sites;
pub(crate) mod suspension;
use crate::frontend::{LocalBinding, RegisteredFunction, RegisteredOperand, RegisteredRegion};
use crate::ir::DisplayKind;
use crate::ir::{self, Binary, Builtin, Expr, ExprKind, Statement, Type};
use crate::llvm::body::FunctionBody;
pub(crate) use abi::{ABI, CallArguments, Output, permanent};
use abi::{GUARDED_LIT, GUARDED_NAMED_OP, NAMED_OP};
pub(crate) use frame::{AnnotationScope, FrameEmitter, RegisteredFrame, SourcePositions};
use sites::CheckedSites;

use crate::llvm::operations::{CheckedBody, CheckedValue, LoopEdges};
use context::Region;
use std::cell::Cell;
use std::collections::HashMap;

use ruff_text_size::{TextRange, TextSize};

/// Only take a body when all its operations have a lowering for the registered calling convention.
pub(crate) fn supports(checked: &RegisteredFunction) -> bool {
    let body = &checked.function.body;
    let mut uses_python_protocol = false;
    let private = checked.region().private_slots();
    let has_slot = |slot: &usize| private.contains(slot) || checked.local_names.contains_key(slot);
    let mut supported = true;
    let mut cause_wrapper = None;
    ir::walk_nodes(body, &mut |node, _| {
        let expr = match node {
            ir::Node::Statement(stmt) => {
                supported &= match stmt {
                    Statement::Assign(slot, _) => has_slot(slot),
                    Statement::Raise(value) => {
                        uses_python_protocol = true;
                        cause_wrapper = value
                            .as_ref()
                            .filter(|value| value.explicit_cause().is_some());
                        true
                    }
                    Statement::Return(_)
                    | Statement::Expr(_)
                    | Statement::While(..)
                    | Statement::Break
                    | Statement::Continue
                    | Statement::Exhaust => true,
                    Statement::Try {
                        handlers,
                        finalbody,
                        ..
                    } => {
                        uses_python_protocol |= handlers
                            .iter()
                            .any(|h| !matches!(h.matcher, ir::ExceptionMatcher::Categories(_)));
                        handlers::supported(handlers, finalbody)
                            && handlers.iter().all(|h| {
                                h.name.is_none_or(|slot| {
                                    checked
                                        .local_names
                                        .get(&slot)
                                        .is_some_and(|binding| binding.namespace.is_none())
                                })
                            })
                    }
                    Statement::With { binding, body, .. } => {
                        uses_python_protocol = true;
                        binding.as_ref().is_none_or(|binding| {
                            private.contains(&binding.entered) && !suspends(&binding.body)
                        }) && !suspends(body)
                    }
                    Statement::Scope { body, .. } => !suspends(body),
                    Statement::SetItem(receiver, _, _)
                    | Statement::AugmentItem(receiver, _, ir::ItemUpdate::Protocol(_), _) => {
                        uses_python_protocol |= receiver.ty == Type::PythonObject;
                        matches!(receiver.ty, Type::Dict(_) | Type::PythonObject)
                    }
                    Statement::Clear(slot) => private.contains(slot),
                    Statement::If(..) => true,
                    Statement::Match(_, cases) => {
                        cases.iter().flat_map(|case| &case.captures).all(has_slot)
                    }
                    Statement::IterNext {
                        kind: kind @ (ir::IterNextKind::Generic | ir::IterNextKind::Async(_)),
                        iterator,
                        key,
                        value,
                        ready,
                    } => {
                        checked.function.locals[*iterator] == Type::PythonObject
                            && value.is_none_or(|slot| {
                                *kind == ir::IterNextKind::Generic && private.contains(&slot)
                            })
                            && [iterator, key, ready]
                                .into_iter()
                                .all(|slot| private.contains(slot))
                    }
                    _ => false,
                };
                return;
            }
            ir::Node::Expression(expr) => expr,
        };
        // Only a raise consumes the cause wrapper directly. Its children still need normal lowering.
        if cause_wrapper
            .take()
            .is_some_and(|value| std::ptr::eq(value, expr))
        {
            return;
        }
        supported &= match &expr.kind {
            ExprKind::Local(slot) => has_slot(slot),
            ExprKind::Bind(slot, _) => {
                uses_python_protocol = true;
                checked.local_names.contains_key(slot)
            }
            ExprKind::None
            | ExprKind::Bool(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bytes(_)
            | ExprKind::Not(_) => true,
            ExprKind::Scope { body, value, .. } => {
                let mut suspension = false;
                ir::walk_operand_expressions(value, &mut |expr| {
                    suspension |= matches!(
                        expr.kind,
                        ExprKind::Builtin(Builtin::RegisteredSuspend(..), _)
                    )
                });
                !suspends(body) && !suspension
            }
            ExprKind::Conditional(..) | ExprKind::BooleanValue(..) | ExprKind::Boolean(..) => true,
            ExprKind::Dict { entries, .. } => entries.is_empty(),
            ExprKind::Builtin(op, args) if let Some(iteration) = op.iteration_operands(args) => {
                uses_python_protocol |= matches!(iteration, Ok(ir::IterationOperands::Iter(..)));
                iteration.is_ok()
            }
            ExprKind::Builtin(Builtin::FormatConversion(_), args) => args.len() == 1,
            ExprKind::Builtin(Builtin::FormatField(_), args) => {
                uses_python_protocol |= args
                    .first()
                    .is_some_and(|value| value.ty == Type::PythonObject);
                args.len() == 2
            }
            ExprKind::Builtin(Builtin::FormatJoin, parts) => parts.len() >= 2,
            ExprKind::Binary(..) => true,
            ExprKind::List(_) | ExprKind::Tuple(_) | ExprKind::Set { .. } => {
                uses_python_protocol = true;
                true
            }
            ExprKind::Builtin(Builtin::RegisteredSpread, args) => args.len() == 1,
            ExprKind::Builtin(Builtin::RegisteredLambda(_, scoped), operands) => {
                uses_python_protocol = true;
                !scoped || !operands.is_empty()
            }
            ExprKind::Builtin(Builtin::RegisteredGenerator(_, scoped), operands) => {
                uses_python_protocol = true;
                ir::generator_operands(*scoped, operands).is_ok()
            }
            ExprKind::Builtin(Builtin::RegisteredCompare(ops), values) => {
                !ops.is_empty() && values.len() == ops.len() + 1
            }
            ExprKind::Builtin(Builtin::RegisteredContainsLiteral(_), values) => !values.is_empty(),
            ExprKind::Invoke { .. } => {
                uses_python_protocol = true;
                expr.is_registered_invoke()
            }
            ExprKind::Builtin(Builtin::RegisteredImport(_), operands) => {
                uses_python_protocol = true;
                operands.is_empty()
            }
            ExprKind::Builtin(Builtin::RegisteredCall(call), operands) => {
                uses_python_protocol = true;
                operands.len() > usize::from(call.explicit_scope)
            }
            ExprKind::Builtin(Builtin::RegisteredIndexConstant(_), args) => args.len() == 1,
            ExprKind::Builtin(Builtin::RegisteredSlice, args) => matches!(args.len(), 2 | 3),
            ExprKind::Builtin(
                Builtin::RegisteredDeleteItem | Builtin::RegisteredInPlace(_),
                args,
            ) => {
                uses_python_protocol = true;
                args.len() == 2
            }
            ExprKind::Builtin(Builtin::RegisteredInput(_), args) => args.is_empty(),
            ExprKind::Builtin(Builtin::RegisteredStatement(..) | Builtin::RegisteredDiscard, _) => {
                true
            }
            ExprKind::Builtin(Builtin::RegisteredLocation(..), operands) => operands.len() == 1,
            ExprKind::Builtin(Builtin::RegisteredAttribute { action, .. }, operands) => {
                use ir::AttributeAction;
                uses_python_protocol |= *action != AttributeAction::Load;
                operands.len()
                    == match action {
                        AttributeAction::Load | AttributeAction::Delete => 1,
                        AttributeAction::Store | AttributeAction::InPlace(_) => 2,
                    }
            }
            ExprKind::Builtin(Builtin::RegisteredUnary(_), operands) => {
                matches!(operands.as_slice(), [operand] if operand.ty == Type::PythonObject)
            }
            ExprKind::Builtin(Builtin::RegisteredSubscript(..), operands) => operands.len() == 2,
            ExprKind::Builtin(Builtin::RegisteredName(_), scope) => scope.len() <= 1,
            ExprKind::Builtin(
                Builtin::RegisteredEllipsis
                | Builtin::RegisteredString(_)
                | Builtin::RegisteredComplex(..),
                operands,
            ) => operands.is_empty(),
            ExprKind::Builtin(Builtin::RegisteredTemplate(segments), fields) => {
                segments.len() == fields.len() + 1
            }
            ExprKind::Builtin(Builtin::RegisteredInterpolation { .. }, args) => args.len() == 2,
            ExprKind::Builtin(Builtin::RegisteredDict(entries), operands) => {
                operands.len()
                    == entries
                        .iter()
                        .map(|entry| match entry {
                            ir::DictEntryKind::Pair => 2,
                            ir::DictEntryKind::Unpack => 1,
                        })
                        .sum::<usize>()
            }
            _ => false,
        };
    });
    // Only an unresolved signature needs a Python operation to establish its
    // calling convention. Solved functions already have an ordinary checked caller.
    let has_live_parameter = checked
        .function
        .params
        .iter()
        .any(|slot| checked.function.locals[*slot] == Type::PythonObject);
    supported && (!has_live_parameter || uses_python_protocol)
}

fn suspends(body: &[Statement]) -> bool {
    let mut suspension = false;
    ir::walk_expressions(body, &mut |expr| {
        suspension |= matches!(
            expr.kind,
            ExprKind::Builtin(Builtin::RegisteredSuspend(..), _)
        )
    });
    suspension
}

#[derive(Default)]
pub(super) struct IterationState {
    private_slots: Vec<usize>,
    first_slot: usize,
    cursors: HashMap<usize, ir::IterNextKind>,
    in_expression: bool,
}

/// Branches to the next pull and to exhaustion keep the loop's cursor live.
pub(crate) struct LoopTarget {
    edges: LoopEdges,
    step_depth: usize,
    exit_depth: usize,
}

pub(super) struct ExpressionScope {
    mark: String,
    outer_expression: bool,
    error_targets: (String, String),
}

/// A checked return can reach a finally prepared by its enclosing suite.
pub(crate) trait PreparedSuiteEmitter<'a, 'p>:
    std::ops::DerefMut<Target = RegisteredFrame<'a, 'p>>
{
    /// Place any child definitions before borrowing the frame for its operand.
    fn frame_and_sites(
        &mut self,
        operand: Option<&Expr>,
    ) -> (&mut RegisteredFrame<'a, 'p>, &CheckedSites);
    fn place_definition(&mut self, site: TextSize);
    fn place_statements(&mut self, body: &[Statement]);
    /// Return an ordinary checked statement for the enclosing frame to emit with its inputs.
    fn emit_effect(
        &mut self,
        effect: &'p crate::frontend::SourceEffect,
    ) -> Option<(&'p Statement, &'p [crate::frontend::PhysicalOperand])>;
}

/// Evaluate an operand in its lexical activation, with the same demand used by
/// ordinary registered expressions. Child placement stays with the owning scope.
pub(crate) fn emit_operand<'a, 'p>(
    owner: &mut dyn PreparedSuiteEmitter<'a, 'p>,
    operand: crate::frontend::BorrowedOperand<'_>,
    condition: bool,
) -> String {
    use crate::frontend::{ExpressionSelection, SourceOperand};

    let emit_checked = |owner: &mut dyn PreparedSuiteEmitter<'a, 'p>, value: &Expr| {
        let (frame, sites) = owner.frame_and_sites(Some(value));
        frame.registered_expression(value, sites, condition)
    };
    let input = match operand {
        SourceOperand::Checked(value) => return emit_checked(owner, value),
        SourceOperand::Residual(input) => input,
    };
    let previous = std::mem::replace(&mut owner.current_range, input.range);
    let result = match input.selected {
        ExpressionSelection::Checked(value) => emit_checked(owner, value),
        ExpressionSelection::Operand(operand) => {
            let (frame, sites) = owner.frame_and_sites(Some(&operand.value));
            frame.prepared_operand(operand, sites, condition)
        }
        ExpressionSelection::Inputs(inputs) => {
            let location = owner.enter_expression(input.range, inputs.position);
            let result = Frame::prepared_operation(owner, inputs, condition);
            owner.leave_expression(location);
            result
        }
        ExpressionSelection::Refused(range, reason) => {
            owner.refuse_at(*range, reason);
            "0".into()
        }
    };
    owner.current_range = previous;
    result
}

/// A statement operand may own a checked region with its own local bindings.
pub(crate) fn emit_physical_operand<'a, 'p>(
    owner: &mut dyn PreparedSuiteEmitter<'a, 'p>,
    operand: &crate::frontend::PhysicalOperand,
) -> String {
    match operand {
        crate::frontend::PhysicalOperand::Checked(value) => {
            let (frame, sites) = owner.frame_and_sites(Some(&value.value));
            frame.prepared_operand(value, sites, false)
        }
        crate::frontend::PhysicalOperand::Residual(input) => emit_operand(
            owner,
            crate::frontend::SourceOperand::Residual(input.as_input()),
            false,
        ),
    }
}

/// Emit checked statements and the operations that interrupt them in one activation.
pub(crate) fn emit_prepared_suite<'a, 'p>(
    owner: &mut dyn PreparedSuiteEmitter<'a, 'p>,
    suite: &'p crate::frontend::RegisteredSuite,
) {
    emit_checked_region(
        owner,
        &suite.checked,
        &suite.residual,
        FrameInputs::Handles(&[]),
    );
}

fn emit_checked_region<'a, 'p>(
    owner: &mut dyn PreparedSuiteEmitter<'a, 'p>,
    checked: &'p RegisteredFunction,
    steps: &'p [crate::frontend::SuiteOperation],
    inputs: FrameInputs<'_, 'p>,
) {
    let Some(enclosing) = owner.enter_checked_slots(checked.region()) else {
        return;
    };
    let mut frame = Frame {
        code: FrameOwner::Prepared(owner),
        names: Some(&checked.local_names),
        inputs,
        fixed_sites: None,
    };
    let body = &checked.function.body;
    let mut offset = 0;
    for step in steps {
        frame.checked_statements(&body[offset..step.at]);
        offset = step.at;
        if !frame.next_statement() {
            continue;
        }
        let owner = frame.code.prepared_owner();
        match &step.effect {
            Ok(effect) => {
                if let Some((statement, inputs)) = owner.emit_effect(effect) {
                    frame.emit_with_operands(statement, inputs);
                }
            }
            Err((range, reason)) => owner.refuse_at(*range, reason),
        }
    }
    frame.checked_statements(&body[offset..]);
    frame.restore_checked_slots(enclosing);
}

pub(crate) fn prepared_condition<'a, 'p>(
    owner: &mut dyn PreparedSuiteEmitter<'a, 'p>,
    test: &crate::frontend::PhysicalOperand,
) -> String {
    match test {
        crate::frontend::PhysicalOperand::Checked(operand) => {
            let (frame, sites) = owner.frame_and_sites(Some(&operand.value));
            frame.prepared_test(operand, sites)
        }
        crate::frontend::PhysicalOperand::Residual(input) => {
            let mark = owner.temporary_mark();
            let bit = emit_operand(
                owner,
                crate::frontend::SourceOperand::Residual(input.as_input()),
                true,
            );
            owner.discard_truth(&mark);
            bit
        }
    }
}

/// A checked run borrows its enclosing suite when a return can reach a prepared finally.
enum FrameOwner<'f, 'a, 'p> {
    Registered(&'f mut RegisteredFrame<'a, 'p>, &'f CheckedSites),
    Prepared(&'f mut dyn PreparedSuiteEmitter<'a, 'p>),
}

impl<'a, 'p> FrameOwner<'_, 'a, 'p> {
    fn reborrow(&mut self) -> FrameOwner<'_, 'a, 'p> {
        match self {
            Self::Registered(code, sites) => FrameOwner::Registered(code, sites),
            Self::Prepared(owner) => FrameOwner::Prepared(&mut **owner),
        }
    }

    fn parts(&mut self) -> (&mut RegisteredFrame<'a, 'p>, &CheckedSites) {
        match self {
            Self::Registered(code, sites) => (code, sites),
            Self::Prepared(owner) => owner.frame_and_sites(None),
        }
    }

    fn prepared_owner(&mut self) -> &mut dyn PreparedSuiteEmitter<'a, 'p> {
        let Self::Prepared(owner) = self else {
            unreachable!("prepared operation outside its owning suite")
        };
        &mut **owner
    }
}

impl<'a, 'p> std::ops::Deref for FrameOwner<'_, 'a, 'p> {
    type Target = RegisteredFrame<'a, 'p>;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Registered(code, _) => code,
            Self::Prepared(function) => function,
        }
    }
}

impl std::ops::DerefMut for FrameOwner<'_, '_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.parts().0
    }
}

#[derive(Clone, Copy)]
enum FrameInputs<'f, 'p> {
    Handles(&'f [String]),
    Prepared(&'f crate::frontend::PhysicalInputs),
    Statement(&'f [crate::frontend::PhysicalOperand]),
    Region(&'p crate::frontend::PreparedRegion),
}

/// The registered binder owns the scope, argument identities and frame.
pub(crate) struct Frame<'f, 'a, 'p> {
    code: FrameOwner<'f, 'a, 'p>,
    names: Option<&'f HashMap<usize, LocalBinding>>,
    inputs: FrameInputs<'f, 'p>,
    fixed_sites: Option<(&'f RegisteredFunction, &'f Cell<usize>)>,
}

impl<'a, 'p> std::ops::Deref for Frame<'_, 'a, 'p> {
    type Target = RegisteredFrame<'a, 'p>;
    fn deref(&self) -> &Self::Target {
        &self.code
    }
}

impl std::ops::DerefMut for Frame<'_, '_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.code
    }
}

impl<'f, 'a, 'p> Frame<'f, 'a, 'p> {
    pub(crate) fn raise_exception(&mut self, value: Option<&Expr>) {
        let (exception, cause) = match value.and_then(Expr::explicit_cause) {
            Some((exception, cause)) => (Some(exception), Some(cause)),
            None => (value, None),
        };
        self.raise_operands(exception, cause, Self::evaluate_handle);
    }

    fn next_statement(&mut self) -> bool {
        if self.body.terminated {
            if self.module_annotation_sites.is_none() {
                return false;
            }
            // A module provider still reserves the sites in unreachable statements.
            let dead = self.name("b");
            self.label(&dead);
        }
        true
    }

    fn checked_statements(&mut self, body: &[Statement]) {
        if !body.is_empty() && self.next_statement() {
            self.code.prepared_owner().place_statements(body);
            self.emit_suite(body);
        }
    }

    fn emit_with_operands(
        &mut self,
        statement: &Statement,
        inputs: &[crate::frontend::PhysicalOperand],
    ) {
        let mut statement_frame = self.reborrow();
        statement_frame.inputs = FrameInputs::Statement(inputs);
        CheckedBody::registered(statement_frame).emit_statement(statement);
    }

    pub(crate) fn emit_region(
        owner: &'f mut dyn PreparedSuiteEmitter<'a, 'p>,
        region: &'p crate::frontend::PreparedRegion,
    ) {
        emit_checked_region(owner, &region.checked, &[], FrameInputs::Region(region));
    }

    /// Bind the owned occurrence slots used by the checked operation.
    pub(crate) fn prepared_operation(
        owner: &'f mut dyn PreparedSuiteEmitter<'a, 'p>,
        inputs: &'f crate::frontend::PhysicalInputs,
        condition: bool,
    ) -> String {
        if let Some(operands) = inputs.eager_operands() {
            let handles: Vec<_> = operands
                .map(|child| emit_operand(owner, child, false))
                .collect();
            let (code, sites) = owner.frame_and_sites(None);
            return code.with_checked_frame(sites, None, |mut enclosing| {
                let mut frame = enclosing.reborrow();
                frame.inputs = FrameInputs::Handles(&handles);
                frame.evaluate_operand(&inputs.operation, condition)
            });
        }
        Self {
            code: FrameOwner::Prepared(owner),
            names: None,
            fixed_sites: None,
            inputs: FrameInputs::Prepared(inputs),
        }
        .evaluate_operand(&inputs.operation, condition)
    }

    #[cfg(test)]
    pub(super) fn for_expression(
        code: &'f mut RegisteredFrame<'a, 'p>,
        sites: &'f CheckedSites,
    ) -> Self {
        Self {
            code: FrameOwner::Registered(code, sites),
            names: None,
            fixed_sites: None,
            inputs: FrameInputs::Handles(&[]),
        }
    }

    pub(super) fn reborrow(&mut self) -> Frame<'_, 'a, 'p> {
        Frame {
            code: self.code.reborrow(),
            names: self.names,
            fixed_sites: self.fixed_sites,
            inputs: self.inputs,
        }
    }
}

impl Frame<'_, '_, '_> {
    pub(super) fn next_fixed_site(&self) -> Option<TextRange> {
        let (function, next) = self.fixed_sites?;
        let i = next.get();
        next.set(i + 1);
        Some(function.fixed_sites[i])
    }

    pub(super) fn load_fixed_field(&mut self, receiver: &Expr, range: TextRange) -> CheckedValue {
        let (function, _) = self
            .fixed_sites
            .expect("placed field belongs to a fixed body");
        let field = function
            .fixed_fields
            .as_ref()
            .and_then(|fields| fields.get(&range))
            .expect("placed field has a physical attribute");
        self.located_value(range, field.position, |frame| {
            let value = frame.attribute_operation(
                ir::AttributeAction::Load,
                &field.name,
                range,
                std::iter::once(receiver),
                Self::evaluate_handle,
            );
            CheckedValue::handle(value)
        })
    }

    pub(super) fn statement_site(&mut self, range: TextRange, simple: Option<TextRange>) {
        self.code.current_range = range;
        self.code.checkpoint(range);
        self.code.simple_call = simple;
    }

    pub(super) fn located_value(
        &mut self,
        range: TextRange,
        position: TextSize,
        emit: impl FnOnce(&mut Self) -> CheckedValue,
    ) -> CheckedValue {
        let previous_range = std::mem::replace(&mut self.code.current_range, range);
        let previous_line = self.code.enter_expression(range, position);
        let result = emit(self);
        self.code.leave_expression(previous_line);
        self.code.current_range = previous_range;
        result
    }

    fn evaluate(&mut self, expr: &Expr) -> CheckedValue {
        CheckedBody::registered(self.reborrow()).emit_value(expr)
    }

    pub(crate) fn evaluate_handle(&mut self, expr: &Expr) -> String {
        let value = self.evaluate(expr);
        self.materialize(&value)
    }

    fn evaluate_operand(&mut self, expr: &Expr, condition: bool) -> String {
        if condition {
            self.evaluate_test(expr, false)
        } else {
            self.evaluate_handle(expr)
        }
    }

    fn evaluate_test(&mut self, expr: &Expr, already_rooted: bool) -> String {
        let mut inner = expr;
        while let ExprKind::Builtin(Builtin::RegisteredLocation(..), operands) = &inner.kind {
            inner = &operands[0];
        }
        if matches!(
            inner.kind,
            ExprKind::Conditional(..)
                | ExprKind::Boolean(..)
                | ExprKind::BooleanValue(..)
                | ExprKind::Not(_)
                | ExprKind::Builtin(
                    Builtin::Bool | Builtin::RegisteredUnary(ir::ObjectUnary::Not),
                    _
                )
        ) {
            return crate::llvm::operations::ValueJoin::select(
                self,
                Self::body,
                |frame, yes, no| {
                    CheckedBody::registered(frame.reborrow()).branch_condition(expr, yes, no)
                },
                |_| CheckedValue::condition("true".to_owned()),
                |_| CheckedValue::condition("false".to_owned()),
            )
            .expect("logical test has a normal successor")
            .into_condition();
        }
        // A single leaf can use the enclosing test's mark. Short-circuit operands
        // above still release their own temporaries before evaluating the next one.
        CheckedBody::registered(self.reborrow()).test_operand_in_scope(expr, already_rooted)
    }

    fn evaluate_eager_handle(&mut self, expr: &Expr) -> String {
        self.with_operand_source(expr, |code| code.evaluate_handle(expr))
    }

    pub(crate) fn sequence(&mut self, values: &[Expr], display: DisplayKind) -> CheckedValue {
        CheckedValue::handle(self.checked_sequence(values, display, |code, operand| {
            code.evaluate_handle(operand)
        }))
    }

    pub(super) fn has_region_inputs(&self) -> bool {
        matches!(self.inputs, FrameInputs::Region(_))
    }

    pub(super) fn bound_condition(&mut self, expr: &Expr) -> Option<String> {
        let ExprKind::Builtin(Builtin::RegisteredInput(index), _) = &expr.kind else {
            return None;
        };
        Some(self.registered_input(*index, true))
    }

    fn registered_input(&mut self, index: usize, condition: bool) -> String {
        let operand = match self.inputs {
            FrameInputs::Statement(inputs) => &inputs[index],
            FrameInputs::Region(region) => &region.operands[index],
            _ => unreachable!("bound operand outside its owning checked region"),
        };
        let owner = self.code.prepared_owner();
        if condition {
            prepared_condition(owner, operand)
        } else {
            emit_physical_operand(owner, operand)
        }
    }

    pub(crate) fn lookup_local(&mut self, slot: usize) -> String {
        match self.inputs {
            FrameInputs::Region(_) | FrameInputs::Statement(_) | FrameInputs::Handles([]) => {}
            FrameInputs::Handles(inputs) => return inputs[slot].clone(),
            FrameInputs::Prepared(inputs) => {
                return emit_operand(self.code.prepared_owner(), inputs.operand(slot), false);
            }
        }
        if self
            .code
            .iteration
            .private_slots
            .binary_search(&slot)
            .is_ok()
        {
            return self.load_private(slot);
        }
        let binding = &self.names.expect("registered local names")[&slot];
        if let Some(slot) = binding.namespace {
            let scope = self.load_private(slot);
            self.code.named_op("lookup", &scope, &binding.name, "0")
        } else {
            self.code.lookup(&binding.name)
        }
    }

    pub(crate) fn assign(&mut self, slot: usize, value: CheckedValue) {
        let handle = self.materialize(&value);
        if self
            .code
            .iteration
            .private_slots
            .binary_search(&slot)
            .is_ok()
        {
            self.store_private(slot, &handle);
            return;
        }
        let (scope, name) = self.assignment_name(slot);
        let mark = self.target_mark();
        self.code
            .assign_owned_name(&scope, &name, &handle, mark.as_deref());
    }

    fn assignment_name(&mut self, slot: usize) -> (String, String) {
        let binding = &self.names.expect("registered local names")[&slot];
        let scope = if let Some(slot) = binding.namespace {
            self.load_private(slot)
        } else {
            self.code.scope.clone()
        };
        let name = self.code.code_name(&binding.name);
        (scope, name)
    }

    /// Retain the original RHS across a replaced owner's finalizer.
    pub(crate) fn bind_expression(&mut self, slot: usize, mark: &str, handle: &str) {
        let (scope, name) = self.assignment_name(slot);
        let replaced = self
            .code
            .op("assign_named_target", &[&scope, &name, handle]);
        self.code.release_replaced(&replaced, Some((mark, handle)));
    }

    pub(crate) fn body(&mut self) -> &mut FunctionBody {
        self.code.flush_check();
        &mut self.code.body
    }

    pub(crate) fn block_name(&mut self) -> String {
        self.code.name("b")
    }
    pub(crate) fn compare_scope(&mut self) -> (String, (String, String)) {
        (self.code.temporary_mark(), self.begin_operand_scope())
    }
    pub(crate) fn close_compare_scope(&mut self, edge: (String, String), mark: &str) {
        self.finish_operand_scope(mark, edge);
    }

    pub(crate) fn comparison_truth(&mut self, value: &str, mark: Option<&str>) -> String {
        if let Some(mark) = mark {
            self.unwind_operands(mark, |f| f.truth(value))
        } else {
            self.truth(value)
        }
    }

    pub(crate) fn rich_comparison(
        &mut self,
        op: ruff_python_ast::CmpOp,
        left: &str,
        right: &str,
    ) -> (String, Option<String>) {
        if matches!(
            op,
            ruff_python_ast::CmpOp::Is | ruff_python_ast::CmpOp::IsNot
        ) {
            let relation = if op == ruff_python_ast::CmpOp::Is {
                "eq"
            } else {
                "ne"
            };
            let bit = self
                .code
                .value(&format!("icmp {relation} i64 {left}, {right}"));
            let value = self.value(&format!(
                "select i1 {bit}, i64 {}, i64 {}",
                permanent::TRUE,
                permanent::FALSE
            ));
            (value, Some(bit))
        } else {
            (self.code.op(op.as_str(), &[left, right]), None)
        }
    }

    pub(crate) fn target_mark(&mut self) -> Option<String> {
        self.code
            .iteration
            .in_expression
            .then(|| self.temporary_mark())
    }
    pub(crate) fn unpack(&mut self, value: &str, count: usize, star: Option<usize>) -> String {
        let count = self.code.integer(count);
        let star = star.map_or_else(|| "0".into(), |index| self.code.integer(index));
        self.code.op("unpack_read", &[value, &count, &star])
    }
    pub(crate) fn comprehension_scope(
        &mut self,
        parent: Option<String>,
        names: &[String],
    ) -> String {
        let parent = parent.unwrap_or_else(|| self.code.scope.clone());
        let names = self.code.lit(1, names.join(" ").as_bytes());
        self.code.op("comprehension_scope", &[&parent, &names])
    }
    pub(super) fn start_expression(&mut self) -> ExpressionScope {
        ExpressionScope {
            mark: self.temporary_mark(),
            outer_expression: std::mem::replace(&mut self.code.iteration.in_expression, true),
            error_targets: self.begin_operand_scope(),
        }
    }
    pub(super) fn finish_expression(
        &mut self,
        scope: ExpressionScope,
        clear: &[usize],
        result: &CheckedValue,
    ) {
        self.code.iteration.in_expression = scope.outer_expression;
        let mark = scope.mark;
        self.code.finish_unwind(scope.error_targets, |code| {
            // A handler checkpoint republishes these slots. Forget the abandoned handles
            // before releasing their roots, including when nested inside another expression.
            for slot in clear {
                if let Ok(index) = code.iteration.private_slots.binary_search(slot) {
                    let physical = code.iteration.first_slot + index;
                    code.line(&format!("store i64 0, ptr %ir_slot{physical}"));
                }
            }
            code.release_owned_operands(&mark);
        });
        let handle = match result {
            CheckedValue::Handle { id, .. } => id.as_str(),
            CheckedValue::IntOrHandle { fallback, .. } => fallback.as_str(),
            CheckedValue::Native(..) => "0",
        };
        self.code.keep_operands(&mark, handle, "0");
    }
    /// A nested handler inside the scope keeps its cursor. Only a transfer to
    /// the scope's enclosing error target abandons these physical root words.
    pub(crate) fn unwind_scope(&mut self, clear: &[usize]) {
        if clear.is_empty() || self.code.body.error_target == "failed" {
            return;
        }
        let frame = &mut *self.code;
        let slots = frame
            .scope_unwinds
            .entry(frame.body.error_target.clone())
            .or_default();
        // Stack bottom first: an inner scope unwinds before its parent, and
        // each scope consumes its pending slots in the declared clear order.
        for slot in clear.iter().rev() {
            if let Ok(index) = frame.iteration.private_slots.binary_search(slot) {
                slots.insert(frame.iteration.first_slot + index);
            }
        }
    }

    pub(crate) fn enter_scope(&mut self, clear: &[usize]) {
        self.unwind_scope(clear);
        self.code.regions.push(Region::Slots(clear.to_vec()));
    }
    pub(crate) fn leave_scope(&mut self) {
        self.code.regions.pop();
    }
    pub(crate) fn enter_loop(&mut self, edges: &LoopEdges) {
        let depth = self.code.regions.len();
        self.push_loop(edges.clone(), depth, depth);
    }
    pub(crate) fn leave_loop(&mut self) {
        self.code.loops.pop();
    }
    pub(crate) fn transfer_loop(&mut self, stmt: &Statement) {
        self.unwind_loop(stmt);
    }
    pub(crate) fn begin_step(&mut self, mark: &str) {
        self.code.line(&format!(
            "call void @imre_dyn_comprehension_step(ptr %ctx, i64 {mark})"
        ));
    }
    pub(crate) fn pull(
        &mut self,
        iterator: usize,
        key: usize,
        second: Option<usize>,
        ready: usize,
        kind: ir::IterNextKind,
    ) {
        let cursor = self.load_private(iterator);
        let done = self.code.name("b");
        self.store_private(ready, &permanent::FALSE.to_string());
        let header = self.code.current_range.start();
        let has_else = self
            .code
            .loops
            .last()
            .is_some_and(|target| target.edges.exhausted != target.edges.done);
        if let Some(second) = second {
            let available = self.code.name("b");
            self.code
                .next_item_pair(&cursor, header, has_else, &available, &done);
            self.code.label(&available);
            self.code.store_item_pair([key, second]);
        } else {
            let item = self.code.pull_item(&cursor, kind, header, has_else, &done);
            self.store_private(key, &item);
        }
        self.store_private(ready, &permanent::TRUE.to_string());
        self.code.jump(&done);
        self.code.label(&done);
    }
    pub(crate) fn declare_integer_arithmetic(&mut self, op: Binary) {
        match op {
            Binary::Add => self.code.output.declarations.checked_integer_add = true,
            Binary::Subtract => self.code.output.declarations.checked_integer_subtract = true,
            Binary::Multiply => self.code.output.declarations.checked_integer_multiply = true,
            Binary::Modulo => self.code.output.declarations.checked_integer_modulo = true,
            _ => unreachable!("expected checked integer operation"),
        }
    }

    pub(crate) fn integer(&mut self, text: &str, integer: Option<i64>) -> CheckedValue {
        match integer {
            Some(n) => CheckedValue::rooted_integer(self.code.integer(n), n),
            None => CheckedValue::handle(self.code.lit(0, text.as_bytes())),
        }
    }

    pub(crate) fn string(&mut self, value: &str) -> CheckedValue {
        CheckedValue::handle(self.code.source_text(value, None))
    }

    pub(crate) fn float(&mut self, value: f64) -> CheckedValue {
        CheckedValue::handle(self.code.lit(3, &value.to_le_bytes()))
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) -> CheckedValue {
        CheckedValue::handle(self.code.lit(2, value))
    }

    pub(crate) fn boolean(&mut self, value: bool) -> CheckedValue {
        CheckedValue::handle(
            (if value {
                permanent::TRUE
            } else {
                permanent::FALSE
            })
            .to_string(),
        )
    }

    pub(crate) fn consume_expression(&mut self, expr: &Expr) -> bool {
        if let ExprKind::Builtin(
            Builtin::RegisteredAssertionFailure { statement, test },
            operands,
        ) = &expr.kind
        {
            let message = operands.first().map(|expr| self.evaluate_handle(expr));
            self.code
                .assertion_failure(*statement, *test, message.as_deref());
            true
        } else if let ExprKind::Builtin(Builtin::RegisteredSuite(index), _) = &expr.kind {
            let FrameInputs::Region(region) = self.inputs else {
                unreachable!("ordered child suite outside its control region")
            };
            emit_prepared_suite(self.code.prepared_owner(), &region.suites[*index]);
            true
        } else if let ExprKind::Builtin(Builtin::RegisteredStatement(range, simple), _) = &expr.kind
        {
            self.statement_site(*range, *simple);
            true
        } else if matches!(
            expr.kind,
            ExprKind::Builtin(
                Builtin::RegisteredAttribute {
                    action: ir::AttributeAction::Store,
                    ..
                },
                _
            )
        ) {
            let mark = self.target_mark();
            self.evaluate(expr);
            self.release_target(mark.as_deref());
            true
        } else if matches!(
            expr.kind,
            ExprKind::Builtin(
                Builtin::GlobalSet(_)
                    | Builtin::RegisteredImport(_)
                    | Builtin::RegisteredDeleteName(..)
                    | Builtin::RegisteredNamespaceAnnotation(_)
                    | Builtin::RegisteredDiscard
                    | Builtin::RegisteredAnnotationStart
                    | Builtin::RegisteredAnnotationFormat
                    | Builtin::RegisteredAnnotationInsert(_)
                    | Builtin::RegisteredAttribute {
                        action: ir::AttributeAction::Delete | ir::AttributeAction::InPlace(_),
                        ..
                    }
                    | Builtin::RegisteredDeleteItem,
                _
            )
        ) {
            self.evaluate(expr);
            true
        } else {
            false
        }
    }

    pub(crate) fn discard_expression(&mut self, value: &CheckedValue) {
        // Only the Python edge can produce an unawaited coroutine.
        let handle = match value {
            CheckedValue::Handle { id, .. } => Some(id),
            CheckedValue::IntOrHandle { fallback, .. } => Some(fallback),
            CheckedValue::Native(..) => None,
        };
        if let Some(handle) = handle {
            self.code.discard_handle(handle);
        }
    }

    pub(crate) fn return_result(&mut self, value: CheckedValue) {
        let value = self.materialize(&value);
        self.return_from_regions(value);
    }

    pub(crate) fn terminated(&self) -> bool {
        self.code.body.terminated
    }

    pub(crate) fn release_word(&mut self, owner: usize) {
        let word = self.code.value(&format!("load i64, ptr %int_owner{owner}"));
        self.code
            .line(&format!("store i64 1, ptr %int_owner{owner}"));
        self.code
            .line(&format!("call void @imre_int_release(i64 {word})"));
    }

    pub(crate) fn builtin(&mut self, op: &Builtin, operands: &[Expr]) -> CheckedValue {
        let handle = match (op, operands) {
            (Builtin::RegisteredInput(index), []) => self.registered_input(*index, false),
            (op, args) if let Some(iteration) = op.iteration_operands(args) => {
                use ir::IterationOperands::*;
                match iteration.expect("checked iteration operands") {
                    Iter(value, op) => {
                        self.acquire_iterator(op, |frame| frame.evaluate_handle(value))
                    }
                    PairNeedsRow(value) => {
                        let cursor = self.evaluate_handle(value);
                        let plain = self.code.plain_items_cursor(&cursor);
                        return self.code.boolean_result(&plain, true);
                    }
                    Truth(value) => {
                        let value = self.evaluate_handle(value);
                        self.code.op("truth", &[&value])
                    }
                    Get([base, key]) => {
                        let base = self.evaluate_handle(base);
                        let key = self.evaluate_handle(key);
                        self.code.op("get", &[&base, &key])
                    }
                    Insert([out, item]) => {
                        let out = self.evaluate_handle(out);
                        let (op, item) = match sequences::spread(item) {
                            Some(iterable) => ("extend", iterable),
                            None => ("append", item),
                        };
                        let item = self.evaluate_handle(item);
                        self.code.op(op, &[&out, &item])
                    }
                    Unpack(value, count, star, target) => {
                        let value = self.evaluate_handle(value);
                        let unpack = |frame: &mut Self| frame.unpack(&value, count, star);
                        match target {
                            Some(range) => self.with_source_range(range, unpack),
                            None => unpack(self),
                        }
                    }
                    Namespace(names, parent) => {
                        let parent = parent.map(|value| self.evaluate_handle(value));
                        self.comprehension_scope(parent, names)
                    }
                }
            }
            (Builtin::RegisteredContainsLiteral(negate), [needle, items @ ..]) => {
                let needle = self.evaluate_handle(needle);
                let items = items
                    .iter()
                    .map(|item| self.evaluate_handle(item))
                    .collect::<Vec<_>>();
                self.contains_literal(&needle, &items, *negate)
            }
            (Builtin::FormatConversion(_) | Builtin::FormatField(_) | Builtin::FormatJoin, _) => {
                let mark = self.temporary_mark();
                self.unwind_stack_operands(&mark, |frame| {
                    let result = match (op, operands) {
                        (Builtin::FormatConversion(conversion), [value]) => {
                            let value = frame.evaluate_handle(value);
                            frame.keep_operands(&mark, &value, "0");
                            frame.format_conversion(&value, *conversion)
                        }
                        (Builtin::FormatField(_), [value, spec]) => {
                            let value = frame.evaluate_handle(value);
                            frame.keep_operands(&mark, &value, "0");
                            let spec = frame.evaluate_handle(spec);
                            frame.format_field(&value, &spec)
                        }
                        (Builtin::FormatJoin, parts) => {
                            let parts = parts
                                .iter()
                                .map(|part| frame.evaluate_handle(part))
                                .collect();
                            return frame.format_join(&mark, parts);
                        }
                        _ => unreachable!("checked format operands"),
                    };
                    frame.keep_operands(&mark, &result, "0");
                    result
                })
            }
            (Builtin::RegisteredName(name), []) => self.code.lookup(name),
            (Builtin::RegisteredName(name), [scope]) => {
                let scope = self.evaluate_handle(scope);
                self.code.named_op("lookup", &scope, name, "0")
            }
            (Builtin::RegisteredSuspend(kind, range), [value]) => {
                self.suspend_value(*kind, *range, |code| code.evaluate_handle(value))
            }
            (Builtin::RegisteredEllipsis, []) => permanent::ELLIPSIS.to_string(),
            (Builtin::RegisteredComplex(real, imag), []) => self
                .code
                .lit(4, &[real.to_le_bytes(), imag.to_le_bytes()].concat()),
            (Builtin::RegisteredString(points), []) => self.code.linked_string(points.clone()),
            (Builtin::RegisteredTemplate(segments), fields) => {
                let strings = self.code.emit_string_tuple(segments.to_vec());
                let parts = self.code.op("list", &[]);
                for field in fields {
                    let value = self.evaluate_handle(field);
                    self.code.op("append", &[&parts, &value]);
                }
                self.code.op("build_template", &[&strings, &parts])
            }
            (
                Builtin::RegisteredInterpolation {
                    expression,
                    conversion,
                },
                [value, spec],
            ) => self.with_operand_source(value, |frame| {
                let value = frame.evaluate_handle(value);
                let spec = frame.evaluate_handle(spec);
                frame
                    .code
                    .interpolation(&value, expression, *conversion, &spec)
            }),
            (Builtin::RegisteredLambda(site, scoped), operands) => {
                let mut operands = operands.iter();
                let scope = if *scoped {
                    self.evaluate_handle(operands.next().expect("explicit lambda scope"))
                } else {
                    self.code.scope.clone()
                };
                let defaults = operands
                    .map(|value| self.evaluate_eager_handle(value))
                    .collect::<Vec<_>>();
                let (code, sites) = self.code.parts();
                sites.bind_lambda(code, *site, &scope, &defaults)
            }
            (Builtin::RegisteredGenerator(site, scoped), operands) => {
                let (scope, cursor) =
                    ir::generator_operands(*scoped, operands).expect("checked generator operands");
                let scope = scope
                    .map(|scope| self.evaluate_handle(scope))
                    .unwrap_or_else(|| self.code.scope.clone());
                let mark = self.temporary_mark();
                self.unwind_operands(&mark, |frame| {
                    let cursor = frame.evaluate_handle(cursor);
                    frame.keep_operands(&mark, &cursor, "0");
                    let (code, sites) = frame.code.parts();
                    let result = sites.bind_generator(code, *site, &scope, &cursor);
                    frame.keep_operands(&mark, &result, "0");
                    result
                })
            }
            (Builtin::RegisteredDefinition(site), header) => {
                let (decorators, defaults, arguments) = match site {
                    ir::DefinitionSite::Class {
                        arguments: Some(layout),
                        ..
                    } => {
                        let count = layout.starred.len() + layout.keywords.len();
                        let (decorators, operands) = header.split_at(header.len() - count);
                        (decorators, &[][..], Some((layout, operands)))
                    }
                    ir::DefinitionSite::Class {
                        arguments: None, ..
                    } => (header, &[][..], None),
                    ir::DefinitionSite::Function(start) => {
                        let definition = &self.code.parts().1.definitions[start];
                        let (decorators, defaults) =
                            header.split_at(definition.decorator_offsets.len());
                        (decorators, defaults, None)
                    }
                };
                let decorators = self.decorator_operands(decorators, |code, value| {
                    code.evaluate_eager_handle(value)
                });
                let defaults = defaults
                    .iter()
                    .map(|value| self.evaluate_eager_handle(value))
                    .collect::<Vec<_>>();
                let class_arguments = arguments.map(|(layout, operands)| {
                    self.class_arguments(layout, |code, index| {
                        code.evaluate_handle(&operands[index])
                    })
                });
                if matches!(self.inputs, FrameInputs::Statement(_)) {
                    // Report a refused header input before compiling the nested body.
                    self.code.prepared_owner().place_definition(site.start());
                }
                let (code, sites) = self.code.parts();
                sites.definition(code, site.start(), &decorators, &defaults, class_arguments)
            }
            (Builtin::RegisteredClassNamespace(site, layout), operands) => {
                let (code, sites) = self.code.parts();
                let mut frame = Frame {
                    code: FrameOwner::Registered(code, sites),
                    names: self.names,
                    inputs: self.inputs,
                    fixed_sites: self.fixed_sites,
                };
                let (namespace, parameters, range) = &sites.class_namespaces[site];
                namespace.generic_class(&mut frame, parameters, *range, layout, |code, index| {
                    code.evaluate_handle(&operands[index])
                })
            }
            (Builtin::RegisteredTypeAlias(site), []) => {
                let (code, sites) = self.code.parts();
                match &sites.aliases[site] {
                    Ok(alias) => alias.instantiate(code),
                    Err(error) => {
                        code.output.diagnostic.get_or_insert_with(|| error.clone());
                        "0".to_owned()
                    }
                }
            }
            (Builtin::RegisteredImport(import), []) => {
                self.code.namespace_import(import);
                "0".to_owned()
            }
            (Builtin::RegisteredAnnotationFormat, [format]) => {
                let format = self.evaluate_handle(format);
                self.code.op("annotation_format", &[&format])
            }
            (Builtin::RegisteredDiscard, operands) => {
                for operand in operands {
                    self.discard_operand(|frame| frame.evaluate_handle(operand));
                }
                "0".to_owned()
            }
            (Builtin::RegisteredAnnotationStart, []) => {
                self.code.begin_annotations();
                "0".to_owned()
            }
            (Builtin::RegisteredAnnotationInsert(field), [value]) => {
                let target = self
                    .code
                    .annotation_target(field.index, &field.name, field.range);
                let value = self.evaluate_handle(value);
                self.code.store_annotation(target, &value);
                "0".to_owned()
            }
            (Builtin::BindAnnotationExpression { site, spelling }, []) => {
                let (code, sites) = self.code.parts();
                sites.annotation_string(code, *site, spelling)
            }
            (Builtin::RegisteredAnnotationResult, []) => self.code.take_annotation_result(),
            (
                Builtin::RegisteredNamespaceAnnotation(ir::NamespaceAnnotation::Deferred {
                    site,
                    name,
                }),
                [],
            ) => {
                self.code.mark_annotation(*site, name);
                "0".to_owned()
            }
            (
                Builtin::RegisteredNamespaceAnnotation(ir::NamespaceAnnotation::String { name }),
                [value],
            ) => {
                let key = self.code.code_name(name);
                let value = self.evaluate_handle(value);
                let scope = self.code.scope.clone();
                self.code.op("annotate", &[&scope, &key, &value]);
                "0".to_owned()
            }
            (Builtin::RegisteredDeleteName(name, range), []) => self.code.delete_name(name, *range),
            (Builtin::GlobalSet(slot), [value]) => {
                let value = self.evaluate_handle(value);
                let name = self.names.expect("registered local names")[slot]
                    .name
                    .clone();
                self.bind(&name, &value);
                return CheckedValue::handle("0".into());
            }
            (Builtin::RegisteredIndexConstant(key), [value]) => self.index_constant(key, value),
            (Builtin::RegisteredSlice, bounds) => {
                let bounds = [bounds.first(), bounds.get(1), bounds.get(2)].map(|value| {
                    value.map_or_else(|| "0".into(), |value| self.evaluate_handle(value))
                });
                self.slice_object(bounds)
            }
            (Builtin::RegisteredInPlace(op), [left, right]) => {
                let left = self.evaluate_handle(left);
                let right = self.evaluate_handle(right);
                self.code.inplace(op, &left, &right)
            }
            (Builtin::RegisteredSubscript(range, base_end), [base, key]) => {
                let base = self.evaluate_handle(base);
                let key = self.read_key(key);
                let read = |frame: &mut Self| frame.read_item(&base, key, *range, *base_end);
                // An injected exception belongs to the parked key. Once it returns,
                // the read itself can fail across several source lines.
                if self.native_suspension
                    && self.positions.line(range.start()) != self.positions.line(range.end())
                {
                    self.with_source_range(*range, read)
                } else {
                    read(self)
                }
            }
            (Builtin::RegisteredDict(entries), values) => {
                let mut values = values.iter();
                let items = entries.iter().map(|entry| {
                    let key = (*entry == ir::DictEntryKind::Pair)
                        .then(|| values.next().expect("checked dictionary key"));
                    (key, values.next().expect("checked dictionary value"))
                });
                self.dict_display(items, |code, expr| code.evaluate_handle(expr))
            }
            _ => unreachable!("unsupported registered operation"),
        };
        CheckedValue::handle(handle)
    }

    pub(crate) fn bind(&mut self, name: &str, handle: &str) {
        let name = self.code.code_name(name);
        let scope = self.code.scope.clone();
        self.code.assign_owned_name(&scope, &name, handle, None);
    }

    pub(crate) fn object_operation(&mut self, op: &Builtin, operands: &[Expr]) -> CheckedValue {
        let (target, args) = operands
            .split_first()
            .expect("checked object operation has a target");
        let handle = FrameEmitter::object_operation(self, op, target, args.iter(), |code, expr| {
            code.evaluate_handle(expr)
        });
        CheckedValue::handle(handle)
    }

    pub(crate) fn own_word(&mut self, word: &str) -> usize {
        let slot = self.code.integer_owners;
        self.code.integer_owners += 1;
        self.code
            .line(&format!("store i64 {word}, ptr %int_owner{slot}"));
        slot
    }

    pub(crate) fn materialize(&mut self, value: &CheckedValue) -> String {
        match value {
            CheckedValue::Handle { id, .. } => id.clone(),
            CheckedValue::IntOrHandle {
                word,
                fallback,
                owner,
            } => {
                let frame = &mut self.code;
                let native = frame.name("box_int");
                let done = frame.name("handle");
                let exact = frame.value(&format!("icmp ne i64 {word}, 0"));
                let mut join = crate::llvm::operations::ValueJoin::default();
                frame.flush_check();
                join.incoming(&mut frame.body, CheckedValue::handle(fallback.clone()));
                frame.branch(&exact, &native, &done);
                frame.label(&native);
                let boxed = frame.move_integer(*owner, word);
                frame.flush_check();
                join.jump(&mut frame.body, &done, CheckedValue::handle(boxed));
                frame.label(&done);
                let Some(CheckedValue::Handle { id, .. }) = join.finish(&mut frame.body) else {
                    unreachable!("both conversion edges hold Python handles")
                };
                id
            }
            CheckedValue::Native(value, owner) => {
                let (word, small) = value.int_word(owner.is_some());
                if let Some(slot) = owner {
                    return self.code.move_integer(*slot, word);
                }
                // A native producer registered the integer ABI before creating this value.
                if !small {
                    self.code
                        .line(&format!("call void @imre_int_retain(i64 {word})"));
                }
                self.code.checked(&format!(
                    "call i64 @imre_dyn_take_int(ptr %ctx, i64 {word})"
                ))
            }
        }
    }

    fn index_constant(&mut self, key: &ir::Constant, candidate: &Expr) -> String {
        if matches!(candidate.kind, ExprKind::Tuple(_)) {
            return self.evaluate_handle(candidate);
        }
        let slot = self.code.constant_slot(key.clone());
        self.cached_tuple(&slot, |code| {
            let value = code.evaluate_handle(candidate);
            code.checked(&format!(
                "call i64 @imre_dyn_linked_value(ptr %ctx, ptr {slot}, i64 {value})"
            ))
        })
    }

    fn read_key(&mut self, key: &Expr) -> ReadKey {
        if let ExprKind::Builtin(Builtin::RegisteredLocation(..), located) = &key.kind
            && let [value] = located.as_slice()
        {
            let (value, constant) = match &value.kind {
                ExprKind::Builtin(Builtin::RegisteredIndexConstant(identity), args) => {
                    (&args[0], Some(identity))
                }
                _ => (value, None),
            };
            if let ExprKind::Builtin(Builtin::RegisteredSlice, bounds) = &value.kind
                && let [lower, upper] = bounds.as_slice()
            {
                let start = self.evaluate_handle(lower);
                let stop = self.evaluate_handle(upper);
                let slot = constant.map(|value| self.constant_slot(value.clone()));
                return ReadKey::Bounds { start, stop, slot };
            }
        }
        ReadKey::Object(self.evaluate_handle(key))
    }
}

pub(crate) enum ReadKey {
    Object(String),
    Bounds {
        start: String,
        stop: String,
        slot: Option<String>,
    },
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn discard_handle(&mut self, handle: &str) {
        self.line(&format!(
            "call void @imre_dyn_discard_expression(ptr %ctx, i64 {handle})"
        ));
    }

    /// Construct an interpolation from evaluated handles and its constant source spelling.
    fn interpolation(
        &mut self,
        value: &str,
        expression: &str,
        conversion: Option<char>,
        spec: &str,
    ) -> String {
        let expression = self.source_text(expression, None);
        let conversion = conversion.map_or_else(|| "0".to_owned(), |c| self.text(&c.to_string()));
        let metadata = self.op("list", &[]);
        self.op("append", &[&metadata, &conversion]);
        self.op("append", &[&metadata, spec]);
        self.op("template_interpolation", &[value, &expression, &metadata])
    }

    fn emit_string_tuple(&mut self, segments: Vec<Vec<u8>>) -> String {
        let slot = self.constant_slot(ir::Constant::Tuple(
            segments.iter().cloned().map(ir::Constant::Text).collect(),
        ));
        let mut frame = self;
        frame.cached_tuple(&slot, |code| {
            let values = segments
                .into_iter()
                .map(|segment| code.linked_string(segment))
                .collect::<Vec<_>>();
            code.build_sequence(&values, true, Some(&slot))
        })
    }
}

impl RegisteredFrame<'_, '_> {
    /// The heap takes the reference even if adding its temporary root fails.
    fn move_integer(&mut self, slot: usize, word: &str) -> String {
        self.line(&format!("store i64 1, ptr %int_owner{slot}"));
        self.checked(&format!(
            "call i64 @imre_dyn_take_int(ptr %ctx, i64 {word})"
        ))
    }
}

pub(crate) struct EnclosingSlots {
    iteration: IterationState,
    live_roots: usize,
}

impl RegisteredFrame<'_, '_> {
    /// Give a prepared suite private slots within its physical function.
    pub(crate) fn enter_checked_slots(
        &mut self,
        checked: RegisteredRegion<'_>,
    ) -> Option<EnclosingSlots> {
        #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
        if let Err(error) = crate::verify::registered(checked) {
            self.output.diagnostic = Some(format!(
                "internal compiler error: invalid registered IR: {error}"
            ));
            return None;
        }
        let iteration = std::mem::take(&mut self.iteration);
        let live_roots = self.live_slots.len();
        let first = self.private_slot_count;
        self.iteration.private_slots = checked.private_slots().into_iter().collect();
        self.iteration.first_slot = first;
        let end = first + self.iteration.private_slots.len();
        self.private_slot_count = end;
        self.live_slots.extend(first..end);
        checked.walk(&mut |node, _| {
            if let ir::Node::Statement(Statement::IterNext { iterator, kind, .. }) = node {
                self.iteration.cursors.insert(*iterator, *kind);
            }
        });
        Some(EnclosingSlots {
            iteration,
            live_roots,
        })
    }

    pub(crate) fn restore_checked_slots(&mut self, enclosing: EnclosingSlots) {
        self.live_slots.truncate(enclosing.live_roots);
        self.iteration = enclosing.iteration;
    }

    fn load_private(&mut self, slot: usize) -> String {
        let slot = self.physical_slot(slot);
        self.value(&format!("load i64, ptr %ir_slot{slot}"))
    }
    pub(super) fn owns_sync_cursor(&self, slots: &[usize]) -> bool {
        !self.iteration.in_expression
            && slots
                .iter()
                .any(|slot| self.iteration.cursors.get(slot) == Some(&ir::IterNextKind::Generic))
    }

    pub(crate) fn clear_slots(&mut self, slots: &[usize]) {
        for &slot in slots {
            if self.iteration.private_slots.binary_search(&slot).is_err() {
                continue;
            }
            if self.iteration.cursors.contains_key(&slot) {
                let old = self.load_private(slot);
                self.store_private(slot, "0");
                self.op("break_iter", &[&old]);
            } else {
                self.store_private(slot, "0");
            }
        }
        if self.owns_sync_cursor(slots) {
            self.release_target(None);
        }
    }
    fn physical_slot(&self, slot: usize) -> usize {
        self.iteration.first_slot
            + self
                .iteration
                .private_slots
                .binary_search(&slot)
                .expect("checked private slot")
    }

    pub(crate) fn store_private(&mut self, slot: usize, value: &str) {
        let slot = self.physical_slot(slot);
        self.line(&format!("store i64 {value}, ptr %ir_slot{slot}"));
    }

    /// Emit a registered expression whose names are resolved through this live frame.
    pub(crate) fn with_checked_frame<R>(
        &mut self,
        sites: &CheckedSites,
        names: Option<&HashMap<usize, LocalBinding>>,
        emit: impl FnOnce(Frame<'_, '_, '_>) -> R,
    ) -> R {
        let first = self.integer_owners;
        let outer = self.expression_integer_start.replace(first);
        let result = emit(Frame {
            code: FrameOwner::Registered(self, sites),
            names,
            fixed_sites: None,
            inputs: FrameInputs::Handles(&[]),
        });
        self.expression_integer_start = outer;
        result
    }

    pub(crate) fn registered_expression(
        &mut self,
        value: &Expr,
        sites: &CheckedSites,
        condition: bool,
    ) -> String {
        self.with_checked_frame(sites, None, |mut frame| {
            frame.evaluate_operand(value, condition)
        })
    }

    pub(crate) fn prepared_operand(
        &mut self,
        operand: &RegisteredOperand,
        sites: &CheckedSites,
        condition: bool,
    ) -> String {
        self.with_prepared_operand(operand, sites, |frame| {
            frame.evaluate_operand(&operand.value, condition)
        })
    }

    pub(crate) fn prepared_test(
        &mut self,
        operand: &RegisteredOperand,
        sites: &CheckedSites,
    ) -> String {
        self.with_prepared_operand(operand, sites, |frame| {
            frame.test_with_temporaries(|frame| frame.evaluate_test(&operand.value, true))
        })
    }

    fn with_prepared_operand(
        &mut self,
        operand: &RegisteredOperand,
        sites: &CheckedSites,
        emit: impl FnOnce(&mut Frame<'_, '_, '_>) -> String,
    ) -> String {
        let Some(enclosing) = self.enter_checked_slots(operand.region()) else {
            return "0".to_owned();
        };
        let result = self.with_checked_frame(sites, Some(&operand.local_names), |mut frame| {
            emit(&mut frame)
        });
        self.restore_checked_slots(enclosing);
        result
    }

    fn store_item_pair(&mut self, items: [usize; 2]) {
        let key = self.value("load i64, ptr %live");
        let slot = self.value("getelementptr i64, ptr %live, i64 1");
        let value = self.value(&format!("load i64, ptr {slot}"));
        self.store_private(items[0], &key);
        self.store_private(items[1], &value);
    }

    pub(crate) fn checked_body(&mut self, checked: &RegisteredFunction, sites: &CheckedSites) {
        let Some(enclosing) = self.enter_checked_slots(checked.region()) else {
            return;
        };
        let next = Cell::new(0);
        CheckedBody::registered(Frame {
            code: FrameOwner::Registered(self, sites),
            names: Some(&checked.local_names),
            fixed_sites: (!checked.fixed_sites.is_empty()).then_some((checked, &next)),
            inputs: FrameInputs::Handles(&[]),
        })
        .emit_function(&checked.function);
        self.restore_checked_slots(enclosing);
    }
}

#[cfg(test)]
mod tests;
