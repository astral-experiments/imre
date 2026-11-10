//! Checked operations inside a Python function with a registered frame and object arguments.

mod abi;
pub(crate) mod context;
mod frame;
pub(crate) mod handlers;
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
pub(crate) use frame::{ArgumentPack, FrameEmitter, Place, RegisteredFrame, SourcePositions};
use sites::CheckedSites;

use crate::llvm::operations::{CheckedBody, CheckedValue, LoopEdges};
use context::Region;
use std::collections::{BTreeSet, HashMap};

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
                    Statement::With { body, entered, .. } => {
                        uses_python_protocol = true;
                        entered.as_ref().is_none_or(|slot| private.contains(slot))
                            && !suspends(body)
                    }
                    Statement::Scope { body, .. } => !suspends(body),
                    Statement::SetItem(receiver, _, _)
                    | Statement::AugmentItem(receiver, _, ir::ItemUpdate::Protocol(_), _) => {
                        uses_python_protocol |= receiver.ty == Type::PythonObject;
                        matches!(receiver.ty, Type::Dict(_) | Type::PythonObject)
                    }
                    Statement::Clear(slot) => private.contains(slot),
                    Statement::If(..) => true,
                    Statement::Match(_, cases) => cases.iter().flat_map(|case| &case.captures).all(has_slot),
                    Statement::IterNext {
                        kind: ir::IterNextKind::Generic,
                        iterator,
                        key,
                        value: None,
                        ready,
                    } => {
                        checked.function.locals[*iterator] == Type::PythonObject
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
                uses_python_protocol |= matches!(iteration, Ok(ir::IterationOperands::Iter(_)));
                iteration.is_ok()
            }
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
            ExprKind::Builtin(Builtin::RegisteredCompare(ops), values) => {
                !ops.is_empty() && values.len() == ops.len() + 1
            }
            ExprKind::Builtin(Builtin::RegisteredContainsLiteral(_), values) => !values.is_empty(),
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
            ExprKind::Builtin(Builtin::RegisteredStatement(..), _) => true,
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
            ExprKind::Builtin(
                Builtin::RegisteredInterpolation { .. } | Builtin::RegisteredFormat,
                args,
            ) => args.len() == 2,
            ExprKind::Builtin(Builtin::RegisteredFormatConversion(_), args) => args.len() == 1,
            ExprKind::Builtin(Builtin::RegisteredFormatJoin, _) => true,
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
    cursors: BTreeSet<usize>,
    scopes: Vec<Region>,
    loops: Vec<(LoopEdges, usize)>,
    in_expression: bool,
}

/// The registered binder owns the scope, argument identities and frame.
pub(crate) struct Frame<'f, 'a, 'p> {
    code: &'f mut RegisteredFrame<'a, 'p>,
    sites: &'f CheckedSites,
    names: Option<&'f HashMap<usize, LocalBinding>>,
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
    pub(crate) fn raise_exception(&mut self, value: Option<&Expr>) {
        let (exception, cause) = match value.and_then(Expr::explicit_cause) {
            Some((exception, cause)) => (Some(exception), Some(cause)),
            None => (value, None),
        };
        let exception = exception.map(|value| self.evaluate_handle(value));
        if self.begin_raise(exception) {
            let cause = cause.map(|value| self.evaluate_handle(value));
            self.finish_raise(cause.as_deref());
        }
    }

    pub(super) fn for_expression(
        code: &'f mut RegisteredFrame<'a, 'p>,
        sites: &'f CheckedSites,
    ) -> Self {
        Self {
            code,
            sites,
            names: None,
        }
    }

    pub(super) fn reborrow(&mut self) -> Frame<'_, 'a, 'p> {
        Frame {
            code: self.code,
            sites: self.sites,
            names: self.names,
        }
    }
}

impl Frame<'_, '_, '_> {
    fn evaluate(&mut self, expr: &Expr) -> CheckedValue {
        CheckedBody::registered(self.reborrow()).emit_value(expr)
    }

    pub(crate) fn evaluate_handle(&mut self, expr: &Expr) -> String {
        let value = self.evaluate(expr);
        self.materialize(&value)
    }

    pub(crate) fn evaluate_test(&mut self, expr: &Expr) -> String {
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
        CheckedBody::registered(self.reborrow()).test_operand(expr)
    }

    pub(crate) fn sequence(&mut self, values: &[Expr], display: DisplayKind) -> CheckedValue {
        CheckedValue::handle(self.checked_sequence(values, display, |code, operand| {
            code.evaluate_handle(operand)
        }))
    }

    pub(crate) fn lookup_local(&mut self, slot: usize) -> String {
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
        self.code
            .assign_expression_name(&scope, &name, handle, mark);
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
    pub(crate) fn start_expression(&mut self) -> (String, bool) {
        (
            self.temporary_mark(),
            std::mem::replace(&mut self.code.iteration.in_expression, true),
        )
    }
    pub(crate) fn finish_expression(&mut self, mark: &str, outer: bool, result: &CheckedValue) {
        self.code.iteration.in_expression = outer;
        let handle = match result {
            CheckedValue::Handle { id, .. } => id.as_str(),
            CheckedValue::IntOrHandle { fallback, .. } => fallback.as_str(),
            CheckedValue::Native(..) => "0",
        };
        self.code.keep_operands(mark, handle, "0");
    }
    fn physical_slot(&self, slot: usize) -> usize {
        let frame = &self.code.iteration;
        frame.first_slot
            + frame
                .private_slots
                .binary_search(&slot)
                .expect("checked private slot")
    }

    fn load_private(&mut self, slot: usize) -> String {
        let slot = self.physical_slot(slot);
        self.code.value(&format!("load i64, ptr %ir_slot{slot}"))
    }
    fn store_private(&mut self, slot: usize, value: &str) {
        let slot = self.physical_slot(slot);
        self.code
            .line(&format!("store i64 {value}, ptr %ir_slot{slot}"));
    }
    pub(crate) fn enter_scope(&mut self, clear: &[usize]) {
        self.code
            .iteration
            .scopes
            .push(Region::Slots(clear.to_vec()));
    }
    pub(crate) fn leave_scope(&mut self) {
        self.code.iteration.scopes.pop();
    }
    pub(crate) fn clear_slots(&mut self, slots: &[usize]) {
        for &slot in slots {
            if self.names.is_some_and(|n| n.contains_key(&slot)) {
                continue;
            }
            if self.code.iteration.cursors.contains(&slot) {
                let old = self.load_private(slot);
                self.store_private(slot, "0");
                self.code.op("break_iter", &[&old]);
            } else {
                self.store_private(slot, "0");
            }
        }
    }
    pub(crate) fn enter_loop(&mut self, edges: &LoopEdges) {
        let depth = self.code.iteration.scopes.len();
        self.code.iteration.loops.push((edges.clone(), depth));
    }
    pub(crate) fn leave_loop(&mut self) {
        self.code.iteration.loops.pop();
    }
    pub(crate) fn transfer_loop(&mut self, stmt: &Statement) {
        let (edges, depth) = self.code.iteration.loops.last().expect("loop transfer");
        let dest = match stmt {
            Statement::Break => &edges.done,
            Statement::Continue => &edges.step,
            _ => &edges.exhausted,
        }
        .clone();
        self.exit_regions(*depth, None);
        self.code.jump(&dest);
    }
    pub(crate) fn begin_step(&mut self, mark: &str) {
        self.code.line(&format!(
            "call void @imre_dyn_comprehension_step(ptr %ctx, i64 {mark})"
        ));
    }
    pub(crate) fn pull_ready(&mut self, iterator: usize, ready: usize) -> (String, String) {
        let cursor = self.load_private(iterator);
        let has_else = self
            .code
            .iteration
            .loops
            .last()
            .is_some_and(|(edges, _)| edges.exhausted != edges.done);
        let more = self
            .code
            .op(if has_else { "more_else" } else { "more" }, &[&cursor]);
        self.store_private(ready, &more);
        let available = self
            .code
            .value(&format!("icmp eq i64 {more}, {}", permanent::TRUE));
        (cursor, available)
    }
    pub(crate) fn pull_item(&mut self, cursor: &str, key: usize) {
        let item = self.code.op("next", &[cursor]);
        self.store_private(key, &item);
    }
    pub(crate) fn declare_integer_arithmetic(&mut self, op: Binary) {
        match op {
            Binary::Add => self.code.output.declarations.checked_integer_add = true,
            Binary::Subtract => self.code.output.declarations.checked_integer_subtract = true,
            Binary::Multiply => self.code.output.declarations.checked_integer_multiply = true,
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
        } else if let ExprKind::Builtin(Builtin::RegisteredStatement(range, simple), _) = &expr.kind
        {
            self.code.current_range = *range;
            self.code.checkpoint(*range);
            self.code.simple_call = *simple;
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
        self.code.live.push(value.clone());
        self.exit_regions(0, Some(&value));
        self.code.live.pop();
        if self.code.body.terminated {
            return;
        }
        if self.code.cleanup_returns.is_some() {
            let label = self.code.name("return_cleanup");
            self.code.jump(&label);
            self.code.cleanup_returns.as_mut().unwrap().push((
                label,
                value,
                self.code.current_range,
            ));
        } else {
            self.code.return_value(&value);
        }
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
            (op, args) if let Some(iteration) = op.iteration_operands(args) => {
                use ir::IterationOperands::*;
                match iteration.expect("checked iteration operands") {
                    Iter(value) => {
                        let value = self.evaluate_handle(value);
                        self.code.op("iter", &[&value])
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
                        let item = self.evaluate_handle(item);
                        self.code.op("append", &[&out, &item])
                    }
                    Unpack(value, count, star) => {
                        let value = self.evaluate_handle(value);
                        self.unpack(&value, count, star)
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
                let strings = self.code.emit_string_tuple(segments.clone());
                let parts = self.code.op("list", &[]);
                for field in fields {
                    let part = self.evaluate_handle(field);
                    self.code.op("append", &[&parts, &part]);
                }
                self.code.op("build_template", &[&strings, &parts])
            }
            (
                Builtin::RegisteredInterpolation {
                    expression,
                    conversion,
                },
                [value, spec],
            ) => {
                let value = self.evaluate_handle(value);
                let spec = self.evaluate_handle(spec);
                self.code
                    .interpolation(&value, expression, *conversion, &spec)
            }
            (Builtin::RegisteredFormatConversion(conversion), [value]) => {
                let value = self.evaluate_handle(value);
                self.code.convert_format_value(&value, *conversion)
            }
            (Builtin::RegisteredFormat, [value, spec]) => {
                let value = self.evaluate_handle(value);
                let spec = self.evaluate_handle(spec);
                self.code.format_value(&value, &spec)
            }
            (Builtin::RegisteredFormatJoin, parts) => {
                let parts = parts
                    .iter()
                    .map(|part| self.evaluate_handle(part))
                    .collect();
                self.code.join_format_parts(parts)
            }
            (Builtin::RegisteredLocation(range, position), [value]) => {
                let previous_range = std::mem::replace(&mut self.code.current_range, *range);
                let previous_line = self.code.enter_expression(*position);
                let value = self.evaluate(value);
                self.code.leave_expression(previous_line);
                self.code.current_range = previous_range;
                return value;
            }
            (Builtin::RegisteredLambda(site, scoped), operands) => {
                let mut operands = operands.iter();
                let scope = if *scoped {
                    self.evaluate_handle(operands.next().expect("explicit lambda scope"))
                } else {
                    self.code.scope.clone()
                };
                let defaults = operands
                    .map(|value| self.evaluate_handle(value))
                    .collect::<Vec<_>>();
                self.sites.bind_lambda(self.code, *site, &scope, &defaults)
            }
            (Builtin::RegisteredDefinition(site), header) => {
                let (prefix, arguments) = match site {
                    ir::DefinitionSite::Class { arguments, .. } => {
                        let count = arguments.starred.len() + arguments.keywords.len();
                        let (decorators, operands) = header.split_at(header.len() - count);
                        (decorators, Some((arguments, operands)))
                    }
                    ir::DefinitionSite::Function(_) => (header, None),
                };
                let header = prefix
                    .iter()
                    .map(|value| self.evaluate_handle(value))
                    .collect::<Vec<_>>();
                let class_arguments = arguments.map(|(layout, operands)| {
                    let mut pack = ArgumentPack::new(self.code, layout, None);
                    for (index, operand) in operands.iter().enumerate() {
                        let slot = pack.prepare(self.code, index);
                        let value = self.evaluate_handle(operand);
                        pack.insert(self.code, slot, &value);
                    }
                    pack.finish(self.code)
                });
                self.sites
                    .definition(self.code, site, &header, class_arguments)
            }
            (Builtin::RegisteredTypeAlias(site), []) => {
                self.sites.aliases[site].instantiate(self.code)
            }
            (Builtin::RegisteredImport(import), []) => {
                self.code.namespace_import(import);
                "0".to_owned()
            }
            (Builtin::RegisteredAnnotationFormat, [format]) => {
                let format = self.evaluate_handle(format);
                self.code.op("annotation_format", &[&format])
            }
            (Builtin::RegisteredAnnotationStart, []) => {
                self.code.begin_annotations();
                "0".to_owned()
            }
            (Builtin::RegisteredAnnotationInsert(occurrence), [value]) => {
                let (name, range) = &self.sites.annotation_fields[&occurrence.site];
                let target = self.code.annotation_target(occurrence.index, name, *range);
                let value = self.evaluate_handle(value);
                self.code.store_annotation(target, &value);
                "0".to_owned()
            }
            (Builtin::BindAnnotationExpression(site), []) => {
                self.sites.annotation_string(self.code, *site)
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
                self.code
                    .op("annotate", &[&self.code.scope.clone(), &key, &value]);
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
            (Builtin::RegisteredDeleteItem, [base, key]) => {
                let base = self.evaluate_handle(base);
                let key = self.evaluate_handle(key);
                self.code.delete_item(&base, &key)
            }
            (Builtin::RegisteredSubscript(range, base_end), [base, key]) => {
                let base = self.evaluate_handle(base);
                let key = self.read_key(key);
                self.read_item(&base, key, *range, *base_end)
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

    pub(crate) fn augment_item(&mut self, receiver: &Expr, key: &Expr, op: &str, rhs: &Expr) {
        let receiver = self.evaluate_handle(receiver);
        let key = self.evaluate_handle(key);
        self.update_item(receiver, key, op, |code| code.evaluate_handle(rhs));
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
    pub(crate) fn interpolation(
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

    pub(crate) fn emit_string_tuple(&mut self, segments: Vec<Vec<u8>>) -> String {
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

    pub(crate) fn convert_format_value(&mut self, value: &str, conversion: char) -> String {
        let conversion = self.text(&conversion.to_string());
        self.op("format_conversion", &[value, &conversion])
    }

    pub(crate) fn format_value(&mut self, value: &str, spec: &str) -> String {
        self.op("format", &[value, "0", spec])
    }

    /// Preserve a lone formatted value and bypass string-subclass addition when joining fields.
    pub(crate) fn join_format_parts(&mut self, parts: Vec<String>) -> String {
        parts
            .into_iter()
            .reduce(|left, right| self.op("format_concat", &[&left, &right]))
            .unwrap_or_else(|| self.text(""))
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
            if let ir::Node::Statement(Statement::IterNext { iterator, .. }) = node {
                self.iteration.cursors.insert(*iterator);
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

    /// Emit a registered expression whose names are resolved through this live frame.
    fn with_checked_frame<R>(
        &mut self,
        sites: &CheckedSites,
        names: Option<&HashMap<usize, LocalBinding>>,
        emit: impl FnOnce(Frame<'_, '_, '_>) -> R,
    ) -> R {
        let first = self.integer_owners;
        let outer = self.expression_integer_start.replace(first);
        let result = emit(Frame {
            code: self,
            names,
            sites,
        });
        self.expression_integer_start = outer;
        result
    }

    pub(crate) fn checked_statement(&mut self, stmt: &Statement, sites: &CheckedSites) {
        self.with_checked_frame(sites, None, |frame| {
            CheckedBody::registered(frame).emit_statement(stmt)
        })
    }

    pub(crate) fn unary_handle(&mut self, op: ir::ObjectUnary, operand: String) -> CheckedValue {
        self.with_checked_frame(&CheckedSites::default(), None, |frame| {
            CheckedBody::registered(frame).unary_value(op, CheckedValue::handle(operand), false)
        })
    }

    pub(crate) fn registered_branch(
        &mut self,
        test: &Expr,
        sites: &CheckedSites,
        yes: &str,
        no: &str,
    ) {
        self.with_checked_frame(sites, None, |frame| {
            CheckedBody::registered(frame).branch_condition(test, yes, no)
        })
    }

    pub(crate) fn logical_operand(
        &mut self,
        plan: &mut super::operations::branches::Logical,
        step: usize,
        operand: super::operations::branches::LogicalOperand,
    ) {
        self.with_checked_frame(&CheckedSites::default(), None, |frame| {
            plan.accept(&mut CheckedBody::registered(frame), step, operand)
        });
    }

    pub(crate) fn registered_value(&mut self, expression: &Expr, sites: &CheckedSites) -> String {
        self.registered_expression(expression, sites, false)
    }

    pub(crate) fn registered_expression(
        &mut self,
        value: &Expr,
        sites: &CheckedSites,
        condition: bool,
    ) -> String {
        self.registered_operand(value, sites, condition, None)
    }

    fn registered_operand(
        &mut self,
        value: &Expr,
        sites: &CheckedSites,
        condition: bool,
        names: Option<&HashMap<usize, LocalBinding>>,
    ) -> String {
        self.with_checked_frame(sites, names, |mut frame| {
            if condition {
                frame.evaluate_test(value)
            } else {
                frame.evaluate_handle(value)
            }
        })
    }

    pub(crate) fn prepared_operand(
        &mut self,
        operand: &RegisteredOperand,
        sites: &CheckedSites,
        condition: bool,
    ) -> String {
        let Some(enclosing) = self.enter_checked_slots(operand.region()) else {
            return "0".to_owned();
        };
        let result =
            self.registered_operand(&operand.value, sites, condition, Some(&operand.local_names));
        self.restore_checked_slots(enclosing);
        result
    }

    pub(crate) fn checked_block(
        &mut self,
        statements: &[Statement],
        checked: &RegisteredFunction,
        sites: &CheckedSites,
    ) {
        CheckedBody::registered(Frame {
            code: self,
            names: Some(&checked.local_names),
            sites,
        })
        .emit_block(statements);
    }

    pub(crate) fn checked_body(&mut self, checked: &RegisteredFunction, sites: &CheckedSites) {
        let Some(enclosing) = self.enter_checked_slots(checked.region()) else {
            return;
        };
        CheckedBody::registered(Frame {
            code: self,
            names: Some(&checked.local_names),
            sites,
        })
        .emit_function(&checked.function);
        self.restore_checked_slots(enclosing);
    }
}

#[cfg(test)]
mod tests;
