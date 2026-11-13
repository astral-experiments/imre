//! Checked operations inside a Python function with a registered frame and object arguments.

mod abi;
pub(crate) mod context;
mod frame;
mod handlers;
pub(crate) mod patterns;
mod sequences;
pub(super) use sequences::permanent_handle;
pub(crate) mod sites;
pub(crate) mod suspension;
use crate::frontend::{
    ExpressionSelection, FrameSlots, LocalBinding, PhysicalOperand, RegisteredBody,
    RegisteredFunction, RegisteredOperand, RegisteredRegion, RegisteredSuite, RequiredSite,
};
use crate::ir::{self, Builtin, Expr, ExprKind, Statement, Type};
use crate::llvm::LoopTarget;
use crate::llvm::body::FunctionBody;
pub(crate) use abi::{ABI, CallArguments, Output, permanent};
pub(crate) use frame::{AnnotationScope, RegisteredFrame, SourcePositions};
use ruff_python_ast::UnaryOp;
use sites::CheckedSites;

use crate::llvm::operations::{CheckedBody, CheckedValue, LoopEdges, ValueUse};
use context::Region;
use std::cell::Cell;
use std::collections::HashMap;

use ruff_text_size::{TextRange, TextSize};

#[cfg(test)]
pub(crate) fn supports(checked: &RegisteredFunction) -> bool {
    frame_inputs(
        checked,
        matches!(
            checked.inputs,
            crate::frontend::FrameInputs::Protocol { .. }
        ),
    )
    .is_some()
}

/// Check the frame's operations and retain their lexical requirements at the same visit.
pub(crate) fn frame_inputs(
    checked: &RegisteredFunction,
    live_parameters: bool,
) -> Option<(crate::frontend::FrameInputs, FrameSlots)> {
    use crate::frontend::{FrameInputs, FrameSite};
    let has_scalar = live_parameters
        && checked.function.params.iter().any(|&slot| {
            checked.function.locals[slot]
                .builtin_scalar_name()
                .is_some()
        });
    let scalar_parameter = |value: &Expr| {
        has_scalar
            && value.ty != Type::PythonObject
            && matches!(value.kind, ExprKind::Local(slot)
                if checked.function.params.contains(&slot) && checked.function.locals[slot].builtin_scalar_name().is_some())
    };
    let protocol_parameter = |value: &Expr| usize::from(scalar_parameter(value.without_location()));
    let mut scalar_reads = 0usize;
    let mut protocol_reads = 0usize;
    let mut sites = Vec::new();
    let mut uses_python_protocol = false;
    let mut required_private = std::collections::BTreeSet::new();
    let mut uses_slot = |slot: &usize, private: bool| {
        if checked.local_names.contains_key(slot) {
            return !private;
        }
        required_private.insert(*slot);
        true
    };
    // Scope declarations can follow uses in the walk. Check their complete set afterward.
    let private = checked.region().frame_layout(|node| {
        let expr = match node {
            ir::Node::Statement(stmt) => {
                return match stmt {
                    Statement::Assign(slot, _) => uses_slot(slot, false),
                    Statement::Raise(value) => {
                        uses_python_protocol = true;
                        value.as_ref().is_none_or(|raised| raised.origin.is_none())
                    }
                    Statement::Return(value) => {
                        protocol_reads += protocol_parameter(value);
                        true
                    }
                    Statement::Position(..)
                    | Statement::Expr(_)
                    | Statement::Scope { .. }
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
                    Statement::With { binding, .. } => {
                        uses_python_protocol = true;
                        binding
                            .as_ref()
                            .is_none_or(|binding| uses_slot(&binding.entered, true))
                    }
                    Statement::SetItem(_, receiver, _, _)
                        if matches!(receiver.ty, Type::ListInt | Type::List(_)) =>
                    {
                        uses_python_protocol = true;
                        true
                    }
                    Statement::SetItem(_, receiver, _, _)
                    | Statement::AugmentItem(receiver, _, ir::ItemUpdate::Protocol(_), _) => {
                        uses_python_protocol |= receiver.ty == Type::PythonObject;
                        matches!(receiver.ty, Type::Dict(_) | Type::PythonObject)
                    }
                    Statement::Clear(slot) => uses_slot(slot, true),
                    Statement::If(..) => true,
                    Statement::Match(_, cases) => cases
                        .iter()
                        .flat_map(|case| &case.captures)
                        .all(|slot| uses_slot(slot, false)),
                    Statement::IterNext {
                        kind: kind @ (ir::IterNextKind::Generic | ir::IterNextKind::Async(_)),
                        iterator,
                        key,
                        value,
                        ready,
                    } => {
                        checked.function.locals[*iterator] == Type::PythonObject
                            && value.is_none_or(|slot| {
                                *kind == ir::IterNextKind::Generic && uses_slot(&slot, true)
                            })
                            && [iterator, key, ready]
                                .into_iter()
                                .all(|slot| uses_slot(slot, true))
                    }
                    _ => false,
                };
            }
            ir::Node::Expression(expr) => expr,
        };
        match &expr.kind {
            ExprKind::Local(slot) => {
                scalar_reads += usize::from(scalar_parameter(expr));
                uses_slot(slot, false)
            }
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
            | ExprKind::Scope { .. } => true,
            ExprKind::Present(_) | ExprKind::Unwrap(_) => true,
            ExprKind::Absent(value) => value.ty == Type::None,
            ExprKind::Conditional(..) | ExprKind::BooleanValue(..) | ExprKind::Boolean(..) => true,
            ExprKind::Dict(entries) => expr.ty == Type::PythonObject || entries.is_empty(),
            ExprKind::Builtin(op, args) if let Some(iteration) = op.iteration_operands(args) => {
                if let Ok(ir::IterationOperands::Iter(value, ir::IteratorOp::Loop(_))) = &iteration
                {
                    protocol_reads += protocol_parameter(value);
                }
                uses_python_protocol |= matches!(iteration, Ok(ir::IterationOperands::Iter(..)));
                iteration.is_ok()
            }
            ExprKind::Builtin(Builtin::FormatConversion(_), args) => {
                protocol_reads += args.first().map_or(0, &protocol_parameter);
                args.len() == 1
            }
            ExprKind::Builtin(Builtin::FormatField(_), args) => {
                let Some((value, spec)) = args.split_first() else {
                    return false;
                };
                let reads = protocol_parameter(value);
                uses_python_protocol |= reads != 0 || value.ty == Type::PythonObject;
                protocol_reads += reads;
                spec.len() <= 1
            }
            ExprKind::Builtin(Builtin::FormatJoin, parts) => parts.len() >= 2,
            ExprKind::Binary(ir::Binary::Add, left, right)
                if has_scalar
                    && matches!(
                        (left.checked_operand_type(), right.checked_operand_type()),
                        (Type::Str, Type::Str | Type::PythonObject)
                            | (Type::PythonObject, Type::Str)
                    ) =>
            {
                // The Python addition receives both operands before it dispatches to either type.
                protocol_reads += protocol_parameter(left) + protocol_parameter(right);
                true
            }
            ExprKind::Binary(..) => true,
            ExprKind::List(values) | ExprKind::Tuple(values) | ExprKind::Set { values, .. } => {
                uses_python_protocol = true;
                protocol_reads += values.iter().map(&protocol_parameter).sum::<usize>();
                true
            }
            ExprKind::Builtin(Builtin::RegisteredSpread, args) => args.len() == 1,
            ExprKind::Builtin(Builtin::RegisteredAssertionFailure { .. }, args) => {
                expr.ty == Type::None && args.len() <= 1
            }
            ExprKind::Builtin(
                Builtin::RegisteredDefinition(ir::DefinitionSite::Function(_)),
                _,
            ) => {
                uses_python_protocol = true;
                true
            }
            ExprKind::Builtin(Builtin::RegisteredLambda(site, scoped), operands) => {
                if live_parameters {
                    sites.push(FrameSite::Lambda(*site));
                }
                uses_python_protocol = true;
                !scoped || !operands.is_empty()
            }
            ExprKind::Builtin(Builtin::RegisteredGenerator(range, scoped, _), operands) => {
                if live_parameters {
                    sites.push(FrameSite::Generator(*range));
                }
                uses_python_protocol = true;
                if let Ok((_, iterable)) = ir::generator_operands(*scoped, operands) {
                    protocol_reads += protocol_parameter(iterable);
                    true
                } else {
                    false
                }
            }
            ExprKind::Compare(values, ops) if ir::Compare::uses_python_protocol(expr.ty, ops) => {
                let reads = values.iter().map(&protocol_parameter).sum::<usize>();
                uses_python_protocol |= reads != 0;
                protocol_reads += reads;
                !ops.is_empty() && values.len() == ops.len() + 1
            }
            ExprKind::Invoke { args, .. } => {
                uses_python_protocol = true;
                if has_scalar {
                    // Registered positional calls pass these handles to the Python binder in order.
                    protocol_reads += args.values.iter().map(&protocol_parameter).sum::<usize>();
                }
                expr.is_registered_invoke()
            }
            ExprKind::Builtin(Builtin::RegisteredImport(_), operands) => {
                uses_python_protocol = true;
                operands.is_empty()
            }
            ExprKind::Builtin(Builtin::RegisteredCall(call), operands) => {
                uses_python_protocol = true;
                let scope = usize::from(call.explicit_scope);
                if has_scalar {
                    // The method receiver and arguments are passed as their original handles.
                    protocol_reads += operands
                        .iter()
                        .skip(scope)
                        .map(&protocol_parameter)
                        .sum::<usize>();
                }
                operands.len() > scope
            }
            ExprKind::Builtin(Builtin::RegisteredIndexConstant(_), args) => args.len() == 1,
            ExprKind::Builtin(Builtin::RegisteredSlice, args) => matches!(args.len(), 2 | 3),
            ExprKind::Builtin(Builtin::DeleteItem | Builtin::RegisteredInPlace(_), args) => {
                uses_python_protocol = true;
                args.len() == 2
            }
            ExprKind::Builtin(Builtin::RegisteredInput(_), args) => args.is_empty(),
            ExprKind::Builtin(Builtin::RegisteredDisplayHook, args) => {
                uses_python_protocol = true;
                args.len() == 1
            }
            ExprKind::Builtin(Builtin::RegisteredDiscard, _) => true,
            ExprKind::Builtin(Builtin::RegisteredLocation(..), operands) => operands.len() == 1,
            ExprKind::Builtin(Builtin::RegisteredAttribute { action, .. }, operands) => {
                use ir::AttributeAction;
                uses_python_protocol |= *action != AttributeAction::Load;
                if *action == AttributeAction::Store && has_scalar {
                    protocol_reads += operands.iter().map(&protocol_parameter).sum::<usize>();
                }
                operands.len()
                    == match action {
                        AttributeAction::Load | AttributeAction::Delete => 1,
                        AttributeAction::Store | AttributeAction::InPlace(_) => 2,
                    }
            }
            ExprKind::Unary(op, operand) => {
                if expr.ty == Type::PythonObject {
                    operand.ty == Type::PythonObject
                } else {
                    matches!(op, UnaryOp::Not | UnaryOp::Invert | UnaryOp::UAdd)
                }
            }
            ExprKind::Builtin(Builtin::RegisteredSubscript(..) | Builtin::ListGet(_), operands) => {
                operands.len() == 2
            }
            ExprKind::Builtin(Builtin::RegisteredName(_), scope) => scope.len() <= 1,
            ExprKind::Builtin(
                Builtin::RegisteredEllipsis
                | Builtin::RegisteredString(_)
                | Builtin::RegisteredComplex(..),
                operands,
            ) => operands.is_empty(),
            ExprKind::Builtin(
                Builtin::RegisteredTemplate | Builtin::RegisteredInterpolation { .. },
                args,
            ) => args.len() == 2,
            _ => false,
        }
    })?;
    // Only an unresolved signature needs a Python operation to establish its
    // calling convention. Solved functions already have an ordinary checked caller.
    let has_live_parameter = has_scalar
        || checked
            .function
            .params
            .iter()
            .any(|&slot| checked.function.locals[slot] == Type::PythonObject);
    if required_private
        .iter()
        .any(|slot| private.index(slot).is_none())
        || has_live_parameter && !uses_python_protocol
        || scalar_reads != protocol_reads
    {
        return None;
    }
    let inputs = if live_parameters {
        FrameInputs::Protocol {
            sites: sites.into_boxed_slice(),
        }
    } else {
        FrameInputs::Checked
    };
    Some((inputs, private))
}

#[derive(Default)]
pub(super) struct IterationState {
    storage: FrameSlots,
    first_slot: usize,
    expression_mark: Option<String>,
}

impl IterationState {
    fn physical_slot(&self, slot: usize) -> Option<usize> {
        self.storage
            .index(&slot)
            .map(|index| self.first_slot + index)
    }
}

pub(super) struct ExpressionScope {
    mark: String,
    outer_expression: Option<String>,
    error_targets: (String, String),
}

/// A checked return can reach a finally prepared by its enclosing suite.
pub(crate) trait PreparedSuiteEmitter<'a, 'p>:
    std::ops::DerefMut<Target = RegisteredFrame<'a, 'p>>
{
    fn frame_and_sites(&mut self) -> (&mut RegisteredFrame<'a, 'p>, &CheckedSites);
    fn place_site(&mut self, site: RequiredSite);
}

/// Evaluate the selected input at its use, placing definitions in the lexical owner.
fn emit_physical_operand<'a, 'p>(
    owner: &mut dyn PreparedSuiteEmitter<'a, 'p>,
    input: &PhysicalOperand,
    condition: bool,
) -> String {
    let previous = owner.current_range;
    let (value, operand) = match input {
        PhysicalOperand::Checked(operand) => (&operand.value, Some(operand.as_ref())),
        PhysicalOperand::Residual(input) => {
            owner.current_range = input.range;
            match input.selected.as_ref() {
                ExpressionSelection::Checked(value) => (value, None),
                ExpressionSelection::Operand(operand) => (&operand.value, Some(operand.as_ref())),
                ExpressionSelection::Refused(range, reason) => {
                    owner.refuse_at(*range, reason);
                    owner.current_range = previous;
                    return "0".into();
                }
            }
        }
    };
    let result = Frame::evaluate_operand(owner, value, operand, condition);
    owner.current_range = previous;
    result
}

impl<'p> RegisteredBody<'p> {
    pub(crate) fn emit(self, code: &mut dyn PreparedSuiteEmitter<'_, 'p>) {
        let (checked, suite, inputs) = match self {
            Self::Function(checked) => (checked, None, &[][..]),
            Self::Suite(suite) => (&suite.checked, Some(suite), suite.operands.as_slice()),
        };
        let Some(enclosing) = code.enter_checked_slots(checked.region()) else {
            return;
        };
        let next = Cell::new(0);
        let frame = Frame {
            code,
            names: Some(&checked.local_names),
            inputs,
            suite,
            fixed_sites: (!checked.fixed_sites.is_empty()).then_some((checked, &next)),
        };
        CheckedBody::registered(frame).emit_function(&checked.function);
        code.restore_checked_slots(enclosing);
    }
}

impl<'a, 'p> Frame<'_, 'a, 'p> {
    /// Write a value or truth test in its owning operand region.
    pub(crate) fn evaluate_operand(
        code: &mut dyn PreparedSuiteEmitter<'a, 'p>,
        value: &Expr,
        operand: Option<&RegisteredOperand>,
        condition: bool,
    ) -> String {
        let enclosing = if let Some(operand) = operand {
            let Some(enclosing) = code.enter_checked_slots(operand.region()) else {
                return "0".into();
            };
            Some(enclosing)
        } else {
            None
        };
        let first = code.integer_owners;
        let outer = code.expression_integer_start.replace(first);
        let mut frame = Frame {
            code,
            names: operand.map(|operand| &operand.local_names),
            inputs: operand.map_or(&[], |operand| &*operand.inputs),
            suite: None,
            fixed_sites: None,
        };
        let result = if condition {
            frame.test_with_temporaries(|frame| frame.evaluate_test(value, true))
        } else {
            frame.evaluate_handle(value)
        };
        code.expression_integer_start = outer;
        if let Some(enclosing) = enclosing {
            code.restore_checked_slots(enclosing);
        }
        result
    }
}

/// The registered binder owns the scope, argument identities and frame.
pub(crate) struct Frame<'f, 'a, 'p> {
    code: &'f mut dyn PreparedSuiteEmitter<'a, 'p>,
    names: Option<&'f HashMap<usize, LocalBinding>>,
    inputs: &'f [PhysicalOperand],
    suite: Option<&'p RegisteredSuite>,
    fixed_sites: Option<(&'f RegisteredFunction, &'f Cell<usize>)>,
}

impl<'a, 'p> std::ops::Deref for Frame<'_, 'a, 'p> {
    type Target = RegisteredFrame<'a, 'p>;
    fn deref(&self) -> &Self::Target {
        self.code
    }
}

impl std::ops::DerefMut for Frame<'_, '_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.code
    }
}

impl<'f, 'a, 'p> Frame<'f, 'a, 'p> {
    #[cfg(test)]
    pub(super) fn for_expression(code: &'f mut dyn PreparedSuiteEmitter<'a, 'p>) -> Self {
        Self {
            code,
            names: None,
            fixed_sites: None,
            inputs: &[],
            suite: None,
        }
    }

    pub(super) fn reborrow(&mut self) -> Frame<'_, 'a, 'p> {
        Frame {
            code: self.code,
            names: self.names,
            fixed_sites: self.fixed_sites,
            inputs: self.inputs,
            suite: self.suite,
        }
    }
}

impl<'f> Frame<'f, '_, '_> {
    pub(super) fn next_fixed_site(&self) -> Option<TextRange> {
        let (function, next) = self.fixed_sites?;
        let i = next.get();
        next.set(i + 1);
        Some(function.fixed_sites[i])
    }

    pub(super) fn operand_range(&self, expr: &Expr) -> Option<TextRange> {
        if let Some((function, next)) = self.fixed_sites {
            return function.fixed_sites.get(next.get()).copied();
        }
        match expr.kind {
            ExprKind::Builtin(Builtin::RegisteredLocation(range, ..), _) => Some(range),
            _ => None,
        }
    }

    pub(super) fn fixed_attribute(
        &self,
        range: TextRange,
    ) -> Option<&'f crate::frontend::FixedAttribute> {
        let (function, _) = self.fixed_sites?;
        function.fixed_attributes.as_ref()?.get(&range)
    }

    pub(super) fn statement_site(&mut self, range: TextRange, simple: Option<TextRange>) {
        self.code.current_range = range;
        self.code.checkpoint(range);
        self.code.simple_call = simple;
    }

    pub(super) fn at_expression<T>(
        &mut self,
        range: TextRange,
        position: TextSize,
        emit: impl FnOnce(&mut Self) -> T,
    ) -> T {
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
        let value =
            CheckedBody::registered(self.reborrow()).value_for(expr, ValueUse::PythonHandle);
        self.materialize_owned(value)
    }

    fn evaluate_test(&mut self, expr: &Expr, already_rooted: bool) -> String {
        if ir::control::needs_truth_plan(expr) {
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

    /// Catching an exception must release every integer owned by the enclosing operand.
    fn evaluate_scoped_handle(&mut self, expr: &Expr) -> String {
        let outer = self.expression_integer_start;
        self.expression_integer_start = Some(outer.unwrap_or(self.integer_owners));
        let value = self.evaluate_handle(expr);
        self.expression_integer_start = outer;
        value
    }

    fn evaluate_eager_handle(&mut self, expr: &Expr) -> String {
        self.with_operand_source(expr, |code| code.evaluate_handle(expr))
    }

    pub(super) fn bound_condition(&mut self, expr: &Expr) -> Option<String> {
        let ExprKind::Builtin(Builtin::RegisteredInput(index), _) = &expr.kind else {
            return None;
        };
        Some(emit_physical_operand(self.code, &self.inputs[*index], true))
    }

    fn registered_input(&mut self, index: usize) -> String {
        emit_physical_operand(self.code, &self.inputs[index], false)
    }

    pub(crate) fn lookup_local(&mut self, slot: usize) -> String {
        if let Some(slot) = self.code.iteration.physical_slot(slot) {
            return self.code.load_frame_slot(slot);
        }
        let binding = &self.names.expect("registered local names")[&slot];
        if let Some(slot) = binding.namespace {
            let scope = self.load_private(slot);
            self.code
                .named_op("lookup_local", &scope, &binding.name, "0")
        } else {
            self.code.lookup(&binding.name)
        }
    }

    pub(crate) fn assign(&mut self, slot: usize, value: CheckedValue) {
        let handle = self.materialize_owned(value);
        if let Some(slot) = self.code.iteration.physical_slot(slot) {
            self.code.store_frame_slot(slot, &handle);
            return;
        }
        let (scope, name) = self.assignment_name(slot);
        self.code.assign_owned_name(&scope, &name, &handle);
    }

    fn assignment_name(&mut self, slot: usize) -> (String, String) {
        let binding = &self.names.expect("registered local names")[&slot];
        let scope = if let Some(slot) = binding.namespace {
            self.load_private(slot)
        } else {
            self.code.scope.clone()
        };
        (scope, binding.name.clone())
    }

    /// Retain the original RHS across a replaced owner's finalizer.
    pub(crate) fn bind_expression(&mut self, slot: usize, mark: &str, handle: &str) {
        let (scope, name) = self.assignment_name(slot);
        let name = self.code.code_name(&name);
        let replaced = self
            .code
            .op("assign_named_target", &[&scope, &name, handle]);
        self.code.line(&format!("call void @imre_dyn_named_assignment(ptr %ctx, i64 {mark}, i64 {handle}, i64 {replaced})"));
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

    /// Evaluate every operand before writing to scratch space that nested expressions can reuse.
    pub(crate) fn evaluate_handle_array(&mut self, operands: &[Expr]) -> (&'static str, usize) {
        let handles: Vec<_> = operands
            .iter()
            .map(|operand| self.evaluate_handle(operand))
            .collect();
        self.handle_array(&handles)
    }

    /// A short list or tuple of constants can be searched without building the container.
    pub(crate) fn literal_sequence_membership(
        &mut self,
        needle: &str,
        container: &Expr,
        negate: bool,
    ) -> Option<String> {
        let (ExprKind::List(items) | ExprKind::Tuple(items)) = &container.without_location().kind
        else {
            return None;
        };
        if items.len() > 8 || !items.iter().all(ir::Constant::is_constant) {
            return None;
        }
        let (array, count) = self.evaluate_handle_array(items);
        let negate = u32::from(negate);
        Some(self.checked(&format!("call i64 @imre_dyn_contains_literal_tuple(ptr %ctx, i64 {needle}, ptr {array}, i64 {count}, i32 {negate})")))
    }

    pub(super) fn iteration_mark(&mut self) -> Option<String> {
        self.code
            .iteration
            .expression_mark
            .is_some()
            .then(|| self.temporary_mark())
    }

    pub(super) fn replace_expression_mark(&mut self, mark: Option<String>) -> Option<String> {
        std::mem::replace(&mut self.code.iteration.expression_mark, mark)
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
        let mark = self.temporary_mark();
        ExpressionScope {
            outer_expression: self.replace_expression_mark(Some(mark.clone())),
            mark,
            error_targets: self.begin_operand_scope(),
        }
    }
    pub(super) fn finish_expression(
        &mut self,
        scope: ExpressionScope,
        clear: &[usize],
        result: &CheckedValue,
    ) {
        self.code.iteration.expression_mark = scope.outer_expression;
        let mark = scope.mark;
        let nested = self
            .code
            .scope_unwinds
            .remove(&scope.error_targets.0)
            .filter(|slots| !slots.is_empty());
        self.code.finish_unwind(scope.error_targets, |code| {
            let slots = clear
                .iter()
                .rev()
                .filter_map(|&slot| code.iteration.physical_slot(slot));
            if let Some(nested) = nested {
                // Pending targets sit below the failed receiver or index. Retain
                // them in lexical unwind order until those operands are gone.
                let slots = slots.chain(nested).collect();
                let retiring = code.take_unwind_slots(slots);
                let (ptr, count) = code.handle_array(&retiring);
                code.output.declare(
                    |d| &mut d.unwind_slots,
                    "declare void @imre_dyn_unwind_slots(ptr, i64, ptr, i64)\n",
                );
                code.line(&format!(
                    "call void @imre_dyn_unwind_slots(ptr %ctx, i64 {mark}, ptr {ptr}, i64 {count})"
                ));
            } else {
                for slot in clear {
                    if let Some(physical) = code.iteration.physical_slot(*slot) {
                        code.line(&format!("store i64 0, ptr %ir_slot{physical}"));
                    }
                }
                code.release_owned_operands(&mark);
            }
        });
        let handle = match result {
            CheckedValue::Handle { id, .. } => id.as_str(),
            CheckedValue::IntOrHandle { fallback, .. } => fallback.as_str(),
            CheckedValue::Native(..) | CheckedValue::IntegerLiteral(_) => "0",
        };
        self.code.keep_operands(&mark, handle, "0");
    }
    /// A nested handler inside the scope keeps its cursor. Only a transfer to
    /// the scope's enclosing error target abandons these physical root words.
    pub(crate) fn unwind_scope(&mut self, clear: &[usize]) {
        if clear.is_empty() || self.code.body.error_target == "failed" {
            return;
        }
        let frame = &mut **self.code;
        let slots = frame
            .scope_unwinds
            .entry(frame.body.error_target.clone())
            .or_default();
        // Stack bottom first: an inner scope unwinds before its parent, and
        // each scope consumes its pending slots in the declared clear order.
        for slot in clear.iter().rev() {
            if let Some(physical) = frame.iteration.physical_slot(*slot) {
                slots.insert(physical);
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
        let cleanup_depth = self.code.regions.len();
        // A resumed for statement keeps its header even inside an operand's cleanup scope.
        let line = (self.code.native_suspension && self.code.iteration.expression_mark.is_none())
            .then(|| self.code.positions.line(self.code.current_range.start()));
        self.code.loops.push((
            LoopTarget {
                edges: edges.clone(),
                cleanup_depth,
            },
            line,
        ));
    }
    pub(crate) fn leave_loop(&mut self) {
        self.code.loops.pop();
    }
    pub(crate) fn begin_step(&mut self, mark: &str) {
        self.code.line(&format!(
            "call void @imre_dyn_comprehension_step(ptr %ctx, i64 {mark})"
        ));
    }
    /// Use the pull's exhaustion edge when its next statement tests that same private flag.
    pub(crate) fn pull_and_exhaust(&mut self, statements: &[Statement]) -> usize {
        let [
            Statement::IterNext {
                iterator,
                key,
                value,
                ready,
                kind,
            },
            Statement::If(test, yes, no),
            ..,
        ] = statements
        else {
            return 0;
        };
        if let ExprKind::Unary(ruff_python_ast::UnaryOp::Not, flag) = &test.kind
            && matches!(flag.kind, ExprKind::Local(slot) if slot == *ready)
            && matches!(yes.as_slice(), [Statement::Exhaust])
            && no.is_empty()
        {
            self.pull(*iterator, *key, *value, *ready, *kind, true);
            return 2;
        }
        0
    }

    pub(crate) fn pull(
        &mut self,
        iterator: usize,
        key: usize,
        second: Option<usize>,
        ready: usize,
        kind: ir::IterNextKind,
        exhaust: bool,
    ) {
        let cursor = self.load_private(iterator);
        let done = self.code.name("b");
        let exhausted = exhaust.then(|| self.code.name("b"));
        let empty = exhausted.as_deref().unwrap_or(&done);
        // The landing clears this private flag before its handler can read it.
        // Only normal exhaustion needs to reset it when the pull owns that edge.
        let reset_on_exhaustion = exhaust
            && kind == ir::IterNextKind::Generic
            && !self.code.native_suspension
            && self
                .code
                .iteration
                .physical_slot(ready)
                .is_some_and(|slot| self.code.unwinds_slot(slot));
        if !reset_on_exhaustion {
            self.store_private(ready, &permanent::FALSE.to_string());
        }
        if kind == ir::IterNextKind::Generic
            && let Some(line) = self.code.loops.last().and_then(|(_, line)| *line)
        {
            self.code.set_statement_line(line);
        }
        let has_else = self
            .code
            .loops
            .last()
            .is_some_and(|(target, _)| target.edges.exhausted != target.edges.done);
        if let Some(second) = second {
            let available = self.code.name("b");
            self.code
                .next_item_pair(&cursor, has_else, &available, empty);
            self.code.label(&available);
            self.code.store_item_pair([key, second]);
        } else {
            let item = self.code.pull_item(&cursor, kind, has_else, empty);
            self.store_private(key, &item);
        }
        self.store_private(ready, &permanent::TRUE.to_string());
        self.code.jump(&done);
        if let Some(label) = exhausted {
            self.code.label(&label);
            if reset_on_exhaustion {
                self.store_private(ready, &permanent::FALSE.to_string());
            }
            CheckedBody::registered(self.reborrow()).loop_transfer(&Statement::Exhaust);
        }
        self.code.label(&done);
    }
    pub(crate) fn string(&mut self, value: &str) -> CheckedValue {
        CheckedValue::handle(self.code.source_text(value, None))
    }

    pub(crate) fn float(&mut self, value: f64) -> CheckedValue {
        CheckedValue::handle(self.code.linked_literal(
            3,
            &value.to_le_bytes(),
            ir::Constant::Float(value.to_bits()),
        ))
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) -> CheckedValue {
        CheckedValue::handle(self.code.linked_literal(
            2,
            value,
            ir::Constant::Bytes(value.to_vec()),
        ))
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
            let Some(region) = self.suite else {
                unreachable!("ordered child suite outside its control region")
            };
            RegisteredBody::Suite(&region.suites[*index]).emit(self.code);
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
            self.evaluate(expr);
            self.release_target();
            true
        } else if matches!(
            expr.kind,
            ExprKind::Builtin(
                Builtin::GlobalSet(_)
                    | Builtin::RegisteredImport(_)
                    | Builtin::RegisteredDeleteName(..)
                    | Builtin::RegisteredNamespaceAnnotation(_)
                    | Builtin::RegisteredDiscard
                    | Builtin::RegisteredDisplayHook
                    | Builtin::RegisteredAnnotationFormat
                    | Builtin::RegisteredAttribute {
                        action: ir::AttributeAction::Delete | ir::AttributeAction::InPlace(_),
                        ..
                    }
                    | Builtin::DeleteItem,
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
            CheckedValue::Native(..) | CheckedValue::IntegerLiteral(_) => None,
        };
        if let Some(handle) = handle {
            self.code.discard_handle(handle);
        }
    }

    pub(crate) fn return_result(&mut self, value: CheckedValue) {
        let value = self.materialize_owned(value);
        self.return_from_regions(value);
    }

    pub(crate) fn terminated(&self) -> bool {
        self.code.body.terminated
    }

    pub(crate) fn release_word(&mut self, owner: usize) {
        self.code.flush_check();
        self.code.release_integer_owners(owner..owner + 1, true);
    }

    pub(super) fn builtin(
        &mut self,
        op: &Builtin,
        operands: &[Expr],
        usage: ValueUse,
    ) -> CheckedValue {
        let handle = match (op, operands) {
            (Builtin::RegisteredCall(call), [target, args @ ..]) => {
                self.call_value(call, target, args)
            }
            (
                Builtin::RegisteredAttribute {
                    action,
                    name,
                    range,
                },
                _,
            ) => self.attribute_operation(*action, name, *range, operands.iter(), usage),
            (Builtin::RegisteredInput(index), []) => self.registered_input(*index),
            (op, args) if let Some(iteration) = op.iteration_operands(args) => {
                use ir::IterationOperands::*;
                match iteration.expect("checked iteration operands") {
                    Iter(value, op) => self.iterator_operand(op, value),
                    PairNeedsRow(value) => {
                        let cursor = self.evaluate_handle(value);
                        let plain = self.code.plain_items_cursor(&cursor);
                        return self.code.boolean_result(&plain, true);
                    }
                    Truth(value) => {
                        let bit = CheckedBody::registered(self.reborrow()).test_operand(value);
                        return self.code.boolean_result(&bit, false);
                    }
                    Insert(_) => unreachable!("checked collection insertion"),
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
            (Builtin::FormatConversion(_) | Builtin::FormatField(_), _) => {
                let mark = self.temporary_mark();
                self.unwind_stack_operands(&mark, |frame| {
                    let result = match (op, operands) {
                        (Builtin::FormatConversion(conversion), [value]) => {
                            let value = frame.evaluate_handle(value);
                            frame.keep_operands(&mark, &value, "0");
                            frame.format_conversion(&value, *conversion)
                        }
                        (Builtin::FormatField(_), [value, spec @ ..]) if spec.len() <= 1 => {
                            let value = frame.evaluate_handle(value);
                            frame.keep_operands(&mark, &value, "0");
                            let spec = spec.first().map(|spec| frame.evaluate_handle(spec));
                            frame.format_field(&value, spec.as_deref().unwrap_or("0"))
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
            (Builtin::RegisteredComplex(real, imag), []) => self.code.linked_literal(
                4,
                &[real.to_le_bytes(), imag.to_le_bytes()].concat(),
                ir::Constant::Complex(real.to_bits(), imag.to_bits()),
            ),
            (Builtin::RegisteredString(points), []) => self.code.linked_string(points.clone()),
            (Builtin::RegisteredTemplate, [strings, fields]) => {
                let strings = self.evaluate_handle(strings);
                let fields = self.evaluate_handle(fields);
                self.code.op("build_template", &[&strings, &fields])
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
                self.code.place_site(RequiredSite::Lambda(*site, *scoped));
                let mut operands = operands.iter();
                let scope = if *scoped {
                    self.evaluate_handle(operands.next().expect("explicit lambda scope"))
                } else {
                    self.code.scope.clone()
                };
                let defaults = operands
                    .map(|value| self.evaluate_eager_handle(value))
                    .collect::<Vec<_>>();
                let (code, sites) = self.code.frame_and_sites();
                sites.bind_lambda(code, *site, &scope, &defaults)
            }
            (Builtin::RegisteredGenerator(site, scoped, acquire), operands) => {
                self.code.place_site(RequiredSite::Generator(*site));
                let (scope, iterable) =
                    ir::generator_operands(*scoped, operands).expect("checked generator operands");
                let scope = scope
                    .map(|scope| self.evaluate_handle(scope))
                    .unwrap_or_else(|| self.code.scope.clone());
                let mark = self.temporary_mark();
                self.unwind_operands(&mark, |frame| {
                    let iterable = frame.evaluate_handle(iterable);
                    let cursor = frame.acquire_iterator(*acquire, &iterable);
                    frame.keep_operands(&mark, &cursor, "0");
                    let (code, sites) = frame.code.frame_and_sites();
                    let result = sites.bind_generator(code, *site, &scope, &cursor);
                    frame.keep_operands(&mark, &result, "0");
                    result
                })
            }
            (Builtin::RegisteredDefinition(site), header) => {
                let requested = RequiredSite::definition(site, header);
                self.code.place_site(requested);
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
                        let definition = &self.code.frame_and_sites().1.definitions[start];
                        let (decorators, defaults) =
                            header.split_at(definition.decorator_offsets.len());
                        (decorators, defaults, None)
                    }
                };
                let decorators = self.decorator_operands(decorators);
                let defaults = defaults
                    .iter()
                    .map(|value| self.evaluate_eager_handle(value))
                    .collect::<Vec<_>>();
                let class_arguments =
                    arguments.map(|(layout, operands)| self.pack_arguments(layout, operands, None));
                if let RequiredSite::DefinitionAfterInputs(start) = requested {
                    self.code.place_site(RequiredSite::Definition(start));
                }
                let (code, sites) = self.code.frame_and_sites();
                sites.definition(code, site.start(), &decorators, &defaults, class_arguments)
            }
            (Builtin::RegisteredClassNamespace(site, layout), operands) => {
                self.code.place_site(RequiredSite::ClassNamespace(*site));
                let (code, sites) = self.code.frame_and_sites();
                let (_, parameters, range) = &sites.class_namespaces[site];
                let range = *range;
                code.set_source_range(Some(range));
                let enclosing = code.scope.clone();
                let outer = code.op("annotation_scope", &[&enclosing]);
                let Some(scope) = code.bind_type_parameters(&enclosing, parameters, true) else {
                    return CheckedValue::handle("0".into());
                };
                code.scope = code.op("class_base_scope", &[&scope, &outer]);
                code.set_source_range(None);
                let arguments = self.pack_arguments(layout, operands, None);
                let (code, sites) = self.code.frame_and_sites();
                code.scope = scope;
                sites.class_namespaces[site]
                    .0
                    .instantiate_class(code, arguments, Some(range))
            }
            (Builtin::RegisteredTypeAlias(site), []) => {
                self.code.place_site(RequiredSite::Alias(*site));
                let (code, sites) = self.code.frame_and_sites();
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
            (Builtin::RegisteredDisplayHook, [value]) => {
                self.discard_operand(|frame| {
                    let value = frame.evaluate_handle(value);
                    frame.code.op("displayhook", &[&value])
                });
                "0".to_owned()
            }
            (Builtin::RegisteredAnnotations { fields }, values) => {
                self.code.begin_annotations();
                for (field, value) in fields.iter().zip(values) {
                    let target = self
                        .code
                        .annotation_target(field.index, &field.name, field.range);
                    let value = self.evaluate_handle(value);
                    self.code.store_annotation(target, &value);
                }
                self.code.take_annotation_result()
            }
            (Builtin::BindAnnotationExpression { site, spelling }, []) => {
                self.code.place_site(RequiredSite::AnnotationString(*site));
                let (code, sites) = self.code.frame_and_sites();
                sites.annotation_string(code, *site, spelling)
            }
            (
                Builtin::RegisteredNamespaceAnnotation(ir::NamespaceAnnotation::Module { index }),
                [],
            ) => {
                let index = self.code.integer(*index);
                let scope = self.code.scope.clone();
                self.code.op("mark_module_annotation", &[&scope, &index]);
                "0".to_owned()
            }
            (
                Builtin::RegisteredNamespaceAnnotation(ir::NamespaceAnnotation::Class {
                    site,
                    name,
                }),
                [],
            ) => {
                self.code.mark_class_annotation(*site, name);
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
            (Builtin::Slice, [receiver]) => {
                let end = self
                    .operand_range(receiver)
                    .expect("placed slice receiver")
                    .end();
                let op = Builtin::RegisteredSubscript(self.code.current_range, end);
                let base = self.evaluate_handle(receiver);
                let key = ir::Constant::slice([const { ir::Constant::None }; 3]);
                return self.read_slice(&op, &base, "0", "0", Some(key));
            }
            (Builtin::RegisteredSlice, bounds) => {
                let bounds = [bounds.first(), bounds.get(1), bounds.get(2)].map(|value| {
                    value.map_or_else(|| "0".into(), |value| self.evaluate_handle(value))
                });
                self.slice_object(bounds)
            }
            (Builtin::RegisteredInPlace(op), [left, right]) => {
                let left = self.evaluate_handle(left);
                let right = self.evaluate_handle(right);
                self.inplace(op, &left, &right)
            }
            _ => unreachable!("unsupported registered operation"),
        };
        CheckedValue::handle(handle)
    }

    pub(crate) fn bind(&mut self, name: &str, handle: &str) {
        let scope = self.code.scope.clone();
        self.code.assign_owned_name(&scope, name, handle);
    }

    pub(super) fn exec_literal_at(&mut self, site: TextRange) -> Option<sites::CompiledExec> {
        self.code.place_site(RequiredSite::ExecLiteral(site));
        self.code
            .frame_and_sites()
            .1
            .exec_literals
            .get(&site)
            .cloned()
    }

    pub(crate) fn own_word(&mut self, word: &str) -> usize {
        let slot = self.code.integer_owners;
        self.code.integer_owners += 1;
        self.code
            .line(&format!("store i64 {word}, ptr %int_owner{slot}"));
        slot
    }

    /// Move an existing Python handle out of a completed operand.
    pub(crate) fn materialize_owned(&mut self, value: CheckedValue) -> String {
        match value {
            CheckedValue::Handle { id, .. } => id,
            value => self.materialize(&value),
        }
    }

    pub(crate) fn materialize(&mut self, value: &CheckedValue) -> String {
        match value {
            CheckedValue::Handle { id, .. } => id.clone(),
            CheckedValue::IntegerLiteral(n) => self.code.integer(*n),
            CheckedValue::IntOrHandle {
                word,
                fallback,
                owner,
            } => {
                let native = self.name("box_int");
                let done = self.name("handle");
                let exact = self.value(&format!("icmp ne i64 {word}, 0"));
                let mut join = crate::llvm::operations::ValueJoin::default();
                self.flush_check();
                join.incoming(&mut self.body, CheckedValue::handle(fallback.clone()));
                self.branch(&exact, &native, &done);
                self.label(&native);
                let boxed = self.move_integer(*owner, word);
                self.flush_check();
                join.jump(&mut self.body, &done, CheckedValue::handle(boxed));
                self.label(&done);
                let Some(CheckedValue::Handle { id, .. }) = join.finish(&mut self.body) else {
                    unreachable!("both conversion edges hold Python handles")
                };
                id
            }
            CheckedValue::Native(value, owner) => {
                let (word, small) = value.int_word(owner.is_some());
                if let Some(slot) = owner {
                    return self.move_integer(*slot, word);
                }
                // A native producer registered the integer ABI before creating this value.
                if !small {
                    self.code
                        .line(&format!("call void @imre_int_retain(i64 {word})"));
                }
                self.take_integer(word)
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

    pub(super) fn slice_subscript(
        &mut self,
        op: &Builtin,
        base: &str,
        key: &Expr,
    ) -> Option<CheckedValue> {
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
                return Some(self.read_slice(op, base, &start, &stop, constant.cloned()));
            }
        }
        None
    }

    fn read_slice(
        &mut self,
        op: &Builtin,
        base: &str,
        start: &str,
        stop: &str,
        constant: Option<ir::Constant>,
    ) -> CheckedValue {
        let slot = constant.map(|key| self.constant_slot(key));
        self.subscript_at(op, |frame, span| {
            frame.take_matching_check();
            let position = span.map_or_else(|| "null".into(), |span| frame.output.position(span));
            if let Some(slot) = slot {
                frame.checked(&format!("call i64 @imre_dyn_get_linked_slice_at(ptr %ctx, i64 {base}, i64 {start}, i64 {stop}, ptr {slot}, ptr {position})"))
            } else {
                frame.checked(&format!("call i64 @imre_dyn_get_slice_at(ptr %ctx, i64 {base}, i64 {start}, i64 {stop}, ptr {position})"))
            }
        })
    }

    pub(super) fn read_index(
        &mut self,
        base: &str,
        key: &CheckedValue,
        span: Option<[usize; 4]>,
    ) -> String {
        let (word, fallback, owner) = match key {
            CheckedValue::IntOrHandle {
                word,
                fallback,
                owner,
            } => (word.as_str(), fallback.as_str(), *owner),
            CheckedValue::Native(operand, Some(owner)) => (operand.int_word(true).0, "0", *owner),
            _ => {
                let key = self.materialize(key);
                return self.op_at("get", &[base, &key], span);
            }
        };
        // Transfer the key to the read; a Python receiver may keep the boxed index.
        self.line(&format!("store i64 1, ptr %int_owner{owner}"));
        let position = span.map_or_else(|| "null".into(), |span| self.output.position(span));
        self.checked(&format!(
            "call i64 @imre_dyn_get_int_item(ptr %ctx, i64 {base}, i64 {word}, i64 {fallback}, ptr {position})"
        ))
    }

    pub(super) fn subscript_at(
        &mut self,
        op: &Builtin,
        read: impl FnOnce(&mut Self, Option<[usize; 4]>) -> String,
    ) -> CheckedValue {
        let Builtin::RegisteredSubscript(range, base_end) = op else {
            return CheckedValue::handle(read(self, None));
        };
        let read = |frame: &mut Self| {
            let span = frame.expression_range_span(*range, *base_end, b'[');
            read(frame, span)
        };
        // An injected exception belongs to the parked key. Once it returns,
        // the read itself can fail across several source lines.
        let value = if self.native_suspension
            && self.positions.line(range.start()) != self.positions.line(range.end())
        {
            self.with_source_range(*range, read)
        } else {
            read(self)
        };
        CheckedValue::handle(value)
    }
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
        self.take_matching_check();
        self.checked(&format!(
            "call i64 @imre_dyn_interpolation(ptr %ctx, i64 {value}, i64 {expression}, i64 {conversion}, i64 {spec})"
        ))
    }
}

impl Frame<'_, '_, '_> {
    /// The heap takes the reference even if adding its temporary root fails.
    fn move_integer(&mut self, slot: usize, word: &str) -> String {
        self.line(&format!("store i64 1, ptr %int_owner{slot}"));
        self.take_integer(word)
    }

    pub(super) fn take_integer(&mut self, word: &str) -> String {
        // An immediate has no reference to transfer. Use the same permanent
        // Python object as a literal when its value is in the cache.
        let raw = self.value(&format!("ashr i64 {word}, 1"));
        let minimum = permanent::SMALL_INT_MIN;
        let maximum_offset = permanent::SMALL_INT_MAX - minimum;
        let offset = self.value(&format!("sub i64 {raw}, {minimum}"));
        let fits = self.value(&format!("icmp ule i64 {offset}, {maximum_offset}"));
        let immediate = self.value(&format!("trunc i64 {word} to i1"));
        let hit = self.value(&format!("and i1 {immediate}, {fits}"));
        let first = permanent::FIRST_SMALL_INT;
        let id = self.value(&format!("add i64 {offset}, {first}"));
        self.cached_handle_or(&hit, &id, |frame| {
            frame.checked(&format!(
                "call i64 @imre_dyn_take_int(ptr %ctx, i64 {word})"
            ))
        })
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
        // Iterator cursors and other private storage occupy unnamed local slots.
        self.iteration.storage = checked.frame_slots.clone();
        self.iteration.first_slot = first;
        let end = first + self.iteration.storage.slots().len();
        self.private_slot_count = end;
        self.live_slots.extend(first..end);
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
        let physical = self
            .iteration
            .physical_slot(slot)
            .expect("checked private slot");
        self.load_frame_slot(physical)
    }

    fn load_frame_slot(&mut self, physical: usize) -> String {
        self.value(&format!("load i64, ptr %ir_slot{physical}"))
    }
    pub(super) fn owns_sync_cursor(&self, slots: &[usize]) -> bool {
        self.iteration.expression_mark.is_none()
            && slots
                .iter()
                .any(|slot| self.iteration.storage.cursor(slot) == Some(ir::IterNextKind::Generic))
    }

    pub(crate) fn clear_slots(&mut self, slots: &[usize]) {
        for &slot in slots {
            let Some(physical) = self.iteration.physical_slot(slot) else {
                continue;
            };
            if self.iteration.storage.cursor(&slot).is_some() {
                let old = self.load_frame_slot(physical);
                self.store_frame_slot(physical, "0");
                self.op("break_iter", &[&old]);
            } else {
                self.store_frame_slot(physical, "0");
            }
        }
        if self.owns_sync_cursor(slots) {
            self.release_target();
        }
    }
    pub(crate) fn store_private(&mut self, slot: usize, value: &str) {
        let physical = self
            .iteration
            .physical_slot(slot)
            .expect("checked private slot");
        self.store_frame_slot(physical, value);
    }

    fn store_frame_slot(&mut self, physical: usize, value: &str) {
        self.line(&format!("store i64 {value}, ptr %ir_slot{physical}"));
    }

    fn store_item_pair(&mut self, items: [usize; 2]) {
        let key = self.value("load i64, ptr %live");
        let slot = self.value("getelementptr i64, ptr %live, i64 1");
        let value = self.value(&format!("load i64, ptr {slot}"));
        self.store_private(items[0], &key);
        self.store_private(items[1], &value);
    }
}

#[cfg(test)]
pub(crate) mod tests;
