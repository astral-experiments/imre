//! Emit checked operations using the storage and ownership of each operand.

mod join;
pub(crate) use join::ValueJoin;

use super::records;
use super::{Binary, Builtin, Expr, ExprKind, Statement, Type};
use super::{Emitter, Value, none, small};
use crate::dynamic::CheckedFrame;

use super::{Operand, Ownership, body::FunctionBody};

/// Evaluate a positional call's callable first, then each argument exactly once.
pub(crate) fn call_arguments<C, E, V>(
    context: &mut C,
    callee: &E,
    args: &[E],
    mut evaluate: impl FnMut(&mut C, &E) -> V,
) -> (V, Vec<V>) {
    let callable = evaluate(context, callee);
    let values = args.iter().map(|arg| evaluate(context, arg)).collect();
    (callable, values)
}

/// A subscript assignment evaluates the RHS before its receiver and index.
pub(crate) fn assignment_operands<C, V>(
    context: &mut C,
    receiver: &Expr,
    index: &Expr,
    value: &Expr,
    mut evaluate: impl FnMut(&mut C, &Expr) -> V,
) -> (V, V, V) {
    let value = evaluate(context, value);
    let receiver = evaluate(context, receiver);
    let index = evaluate(context, index);
    (receiver, index, value)
}

/// A checked expression keeps the representation selected by its producer.
/// A guarded integer carries an owned native sum on the exact edge and the
/// original Python protocol result on the other edge. Zero is not an Int word.
pub(crate) enum CheckedValue {
    Native(Operand, Option<usize>),
    Handle {
        id: String,
        exact_word: Option<String>,
    },
    IntOrHandle {
        word: String,
        fallback: String,
        owner: usize,
    },
}

impl CheckedValue {
    pub(crate) fn handle(id: String) -> Self {
        Self::Handle {
            id,
            exact_word: None,
        }
    }
    pub(crate) fn into_handle(self) -> String {
        let Self::Handle { id, .. } = self else {
            unreachable!("handle-producing expression")
        };
        id
    }

    pub(crate) fn rooted_integer(id: String, n: i64) -> Self {
        Self::Handle {
            id,
            exact_word: exact_integer_literal(n).map(|value| value.operand),
        }
    }

    /// Keep an available native result through the next addition. Two rooted
    /// handles need either two known words or a checked integer whose word
    /// still needs a guard. One known word alone cannot justify probing its peer.
    fn use_integer_add(&self, left: Type, other: &Self, right: Type) -> bool {
        match (self, other) {
            (Self::Handle { exact_word: a, .. }, Self::Handle { exact_word: b, .. }) => {
                (a.is_some() && b.is_some())
                    || (a.is_none() && left == Type::Int)
                    || (b.is_none() && right == Type::Int)
            }
            _ => true,
        }
    }

    fn frame_integer(value: Value, frame: &mut CheckedFrame<'_, '_, '_>) -> Self {
        let owner = frame.own_word(&value.operand);
        Self::Native(
            Operand {
                value,
                ownership: Ownership::Owned,
            },
            Some(owner),
        )
    }

    pub(crate) fn frame_owner(&self) -> Option<usize> {
        match self {
            Self::Native(_, owner) => *owner,
            Self::IntOrHandle { owner, .. } => Some(*owner),
            Self::Handle { .. } => None,
        }
    }

    pub(super) fn native(operand: Operand) -> Self {
        Self::Native(operand, None)
    }
    pub(super) fn owned(value: super::Value) -> Self {
        Self::Native(
            Operand {
                value,
                ownership: Ownership::Owned,
            },
            None,
        )
    }
    pub(super) fn into_native(self) -> Operand {
        let Self::Native(operand, None) = self else {
            unreachable!("native ABI operand")
        };
        operand
    }
}

fn exact_integer_literal(value: i64) -> Option<Value> {
    (-(1_i64 << 62)..1_i64 << 62)
        .contains(&value)
        .then(|| small(value))
}

/// A checked body selects operations from each operand's representation.
/// Its entry convention supplies local storage, retained owners and exception exits.
pub(crate) struct CheckedBody<'f, 'a, 'p, 'module>(Entry<'f, 'a, 'p, 'module>);

enum Entry<'f, 'a, 'p, 'module> {
    Native(&'f mut Emitter<'a, 'module>),
    Registered(CheckedFrame<'f, 'a, 'p>),
}

impl<'f, 'a, 'p, 'module> CheckedBody<'f, 'a, 'p, 'module> {
    pub(super) fn native(emitter: &'f mut Emitter<'a, 'module>) -> Self {
        Self(Entry::Native(emitter))
    }

    pub(crate) fn registered(frame: CheckedFrame<'f, 'a, 'p>) -> Self {
        Self(Entry::Registered(frame))
    }

    fn python(&mut self) -> &mut CheckedFrame<'f, 'a, 'p> {
        let Entry::Registered(frame) = &mut self.0 else {
            unreachable!("checked Python operation requires a registered frame")
        };
        frame
    }

    fn fixed(&mut self) -> &mut Emitter<'a, 'module> {
        let Entry::Native(emitter) = &mut self.0 else {
            unreachable!("checked fixed-layout operation requires a native frame")
        };
        emitter
    }

    fn body(&mut self) -> &mut FunctionBody {
        match &mut self.0 {
            Entry::Native(emitter) => &mut emitter.body,
            Entry::Registered(frame) => frame.body(),
        }
    }

    fn operand(&mut self, expr: &Expr) -> CheckedValue {
        match &mut self.0 {
            Entry::Native(emitter) => CheckedValue::native(emitter.operand(expr)),
            Entry::Registered(_) => self.emit_value(expr),
        }
    }

    fn evaluate(&mut self, expr: &Expr) -> CheckedValue {
        match &mut self.0 {
            Entry::Native(emitter) => CheckedValue::owned(emitter.expr(expr)),
            Entry::Registered(_) => self.emit_value(expr),
        }
    }

    fn release_input(&mut self, value: CheckedValue) {
        match value {
            CheckedValue::Handle { .. } => {}
            CheckedValue::Native(operand, None) => self.fixed().release_operand(&operand),
            CheckedValue::Native(_, Some(owner)) | CheckedValue::IntOrHandle { owner, .. } => {
                self.python().release_word(owner);
            }
        }
    }

    fn declare_int_add(&mut self) {
        if let Entry::Registered(frame) = &mut self.0 {
            frame.declare_int_add();
        }
    }

    fn finish_native_add(&mut self, value: Value) -> CheckedValue {
        match &mut self.0 {
            Entry::Native(emitter) => CheckedValue::owned(emitter.owned(value)),
            Entry::Registered(frame) => CheckedValue::frame_integer(value, frame),
        }
    }
    fn registered_call(
        &mut self,
        operands: &[Expr],
        range: ruff_text_size::TextRange,
        callee_end: ruff_text_size::TextSize,
        callee_name: Option<ruff_text_size::TextRange>,
    ) -> CheckedValue {
        let mark = self.python().call_mark(callee_name);
        let (callee, args) = operands.split_first().expect("checked call has a callee");
        let (callable, values) = call_arguments(self, callee, args, Self::operand);
        let callable = self.python().materialize(&callable);
        let args = values
            .iter()
            .map(|value| self.python().materialize(value))
            .collect::<Vec<_>>();
        CheckedValue::handle(
            self.python()
                .call(&callable, &args, &mark, range, callee_end),
        )
    }
    fn integer_operand(&mut self, value: &CheckedValue) -> (String, bool) {
        match value {
            CheckedValue::Native(v, _) => (v.value.operand.clone(), false),
            CheckedValue::Handle { id, exact_word } => match exact_word {
                Some(word) => (word.clone(), false),
                None => (self.python().exact_word(id), true),
            },
            CheckedValue::IntOrHandle { word, .. } => (word.clone(), true),
        }
    }

    fn python_add(&mut self, left: &CheckedValue, right: &CheckedValue) -> CheckedValue {
        let frame = self.python();
        let lhs = frame.materialize(left);
        let rhs = frame.materialize(right);
        CheckedValue::handle(frame.protocol_add(&lhs, &rhs))
    }

    fn add_integers(&mut self, left: &CheckedValue, right: &CheckedValue) -> CheckedValue {
        self.declare_int_add();
        let (a, check_a) = self.integer_operand(left);
        let (b, check_b) = self.integer_operand(right);
        let out = self.body();
        let guard = match (check_a, check_b) {
            (false, false) => None,
            (true, false) => Some(out.instruction(format!("icmp ne i64 {a}, 0"))),
            (false, true) => Some(out.instruction(format!("icmp ne i64 {b}, 0"))),
            (true, true) => {
                let x = out.instruction(format!("icmp ne i64 {a}, 0"));
                let y = out.instruction(format!("icmp ne i64 {b}, 0"));
                Some(out.instruction(format!("and i1 {x}, {y}")))
            }
        };
        let exact = |code: &mut Self| {
            let sum = code.body().add_int(&a, &b);
            code.finish_native_add(sum)
        };
        let Some(guard) = guard else {
            return exact(self);
        };
        ValueJoin::select(
            self,
            Self::body,
            |code, yes, no| code.body().branch(&guard, yes, no),
            exact,
            |code| code.python_add(left, right),
        )
        .expect("guarded addition has a normal result")
    }

    fn binary_expression(
        &mut self,
        op: Binary,
        left: &Expr,
        right: &Expr,
        expr: &Expr,
    ) -> CheckedValue {
        if let Entry::Native(emitter) = &mut self.0
            && let Some(value) = emitter.specialized_binary(op, left, right, expr)
        {
            return value;
        }
        let (a, b) = (self.operand(left), self.operand(right));
        let result = if matches!(op, Binary::Add)
            && (expr.ty == Type::PythonObject || left.ty == Type::Int && right.ty == Type::Int)
        {
            if a.use_integer_add(left.ty, &b, right.ty) {
                self.add_integers(&a, &b)
            } else {
                self.python_add(&a, &b)
            }
        } else {
            let (CheckedValue::Native(x, _), CheckedValue::Native(y, _)) = (&a, &b) else {
                unreachable!("unsupported checked operator")
            };
            CheckedValue::owned(self.fixed().binary(op, &x.value, &y.value))
        };
        self.release_input(a);
        self.release_input(b);
        result
    }

    pub(crate) fn emit_value(&mut self, expr: &Expr) -> CheckedValue {
        match &expr.kind {
            ExprKind::Int(text) => {
                let integer = text.parse::<i64>().ok();
                match &mut self.0 {
                    Entry::Native(emitter) => CheckedValue::owned(
                        integer
                            .and_then(exact_integer_literal)
                            .unwrap_or_else(|| emitter.pooled_literal(text.as_bytes(), Type::Int)),
                    ),
                    Entry::Registered(frame) => frame.integer(text, integer),
                }
            }
            ExprKind::Str(text) => match &mut self.0 {
                Entry::Native(emitter) => CheckedValue::owned(Value {
                    ty: expr.ty,
                    ..emitter.pooled_literal(text.as_bytes(), Type::Str)
                }),
                Entry::Registered(frame) => frame.string(text),
            },
            ExprKind::Float(value) => match &mut self.0 {
                Entry::Native(_) => CheckedValue::owned(Value {
                    ty: Type::Float,
                    operand: format!("0x{:016X}", value.to_bits()),
                    small_integer: false,
                }),
                Entry::Registered(frame) => frame.float(*value),
            },
            ExprKind::Bytes(value) => match &mut self.0 {
                Entry::Native(emitter) => {
                    CheckedValue::owned(emitter.pooled_literal(value, Type::Bytes))
                }
                Entry::Registered(frame) => frame.bytes(value),
            },
            ExprKind::Bool(value) => match &mut self.0 {
                Entry::Native(_) => CheckedValue::owned(Value {
                    ty: Type::Bool,
                    operand: u8::from(*value).to_string(),
                    small_integer: false,
                }),
                Entry::Registered(frame) => frame.boolean(*value),
            },
            ExprKind::None => match &mut self.0 {
                Entry::Native(_) => CheckedValue::owned(none()),
                Entry::Registered(_) => CheckedValue::handle("0".into()),
            },
            ExprKind::Local(slot) => match &mut self.0 {
                Entry::Native(emitter) => {
                    CheckedValue::owned(emitter.load(*slot, emitter.ranges.integer(expr).is_some()))
                }
                Entry::Registered(frame) => CheckedValue::handle(frame.lookup_local(*slot)),
            },
            ExprKind::Conditional(test, yes, no) => match &mut self.0 {
                Entry::Native(emitter) => CheckedValue::owned(emitter.conditional(test, yes, no)),
                Entry::Registered(frame) => frame.select(test, yes, no),
            },
            ExprKind::Binary(op, left, right) => self.binary_expression(*op, left, right, expr),
            ExprKind::Builtin(Builtin::RegisteredCall(range, callee_end, callee_name), args) => {
                self.registered_call(args, *range, *callee_end, *callee_name)
            }
            ExprKind::Builtin(op, args) => match &mut self.0 {
                Entry::Native(emitter) => CheckedValue::owned(emitter.builtin(op, args, expr.ty)),
                Entry::Registered(frame) => frame.builtin(op, args),
            },
            _ => CheckedValue::owned(self.fixed().other_expression(expr)),
        }
    }

    pub(super) fn emit_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign(slot, expr) => {
                let previous_small = match &mut self.0 {
                    Entry::Native(emitter) => {
                        let Some(small) = emitter.prepare_assignment(*slot, expr) else {
                            return;
                        };
                        small
                    }
                    Entry::Registered(_) => false,
                };
                let value = self.evaluate(expr);
                match &mut self.0 {
                    Entry::Native(emitter) => emitter.assign_native(*slot, value, previous_small),
                    Entry::Registered(frame) => frame.assign(*slot, value),
                }
            }
            Statement::SetItem(receiver, index, value) => match &mut self.0 {
                Entry::Native(emitter) => emitter.assign_item(receiver, index, value),
                Entry::Registered(frame) => frame.assign_item(receiver, index, value),
            },
            Statement::Expr(expr) => {
                let consumed = match &mut self.0 {
                    Entry::Native(emitter) => emitter.consume_expression(expr),
                    Entry::Registered(frame) => frame.consume_expression(expr),
                };
                if !consumed {
                    let value = self.evaluate(expr);
                    if let Entry::Registered(frame) = &mut self.0 {
                        frame.discard_expression(&value);
                    }
                    self.release_input(value);
                }
            }
            Statement::Return(expr) => {
                let value = self.evaluate(expr);
                match &mut self.0 {
                    Entry::Native(emitter) => emitter.return_result(value),
                    Entry::Registered(frame) => frame.return_result(value),
                }
            }
            _ => self.fixed().other_statement(stmt),
        }
    }

    /// Emit the function through one traversal, installing fixed list views only
    /// after the last operation that could invalidate their backing storage.
    pub(crate) fn emit_function(&mut self, function: &crate::ir::Function) {
        let mut statements = function.body.as_slice();
        if let Entry::Native(code) = &self.0
            && let Some((start, slots)) = code.int_buffer_region(function)
        {
            self.emit_block(&statements[..start]);
            let code = self.fixed();
            if code.body.terminated {
                return;
            }
            code.enter_int_buffer_region(slots);
            statements = &statements[start..];
        }
        self.emit_block(statements);
    }

    pub(super) fn emit_block(&mut self, mut remaining: &[Statement]) {
        let incoming = match &self.0 {
            Entry::Native(emitter) => Some(emitter.temporaries.clone()),
            Entry::Registered(_) => None,
        };
        while let Some((stmt, tail)) = remaining.split_first() {
            let fused = match &mut self.0 {
                Entry::Native(emitter) if !emitter.body.terminated => {
                    emitter.fused_statements(remaining)
                }
                Entry::Registered(frame) if !frame.terminated() => 0,
                _ => break,
            };
            if fused == 0 {
                self.emit_statement(stmt);
                remaining = tail;
            } else {
                remaining = &remaining[fused..];
            }
            if let (Entry::Native(emitter), Some(incoming)) = (&self.0, &incoming) {
                debug_assert!(
                    emitter
                        .temporaries
                        .iter()
                        .map(|value| (&value.operand, value.ty))
                        .eq(incoming.iter().map(|value| (&value.operand, value.ty))),
                    "statement changed its enclosing expression owners"
                );
            }
        }
    }
}

impl Emitter<'_, '_> {
    fn assign_item(&mut self, list: &Expr, index: &Expr, expression: &Expr) {
        if self.copy_item(list, index, expression) {
            return;
        }
        let nonnegative = self.nonnegative_index(index);
        let fixed_slot = match list.kind {
            ExprKind::Local(slot)
                if self.fixed_int_lists[slot] && self.int_buffers.contains_key(&slot) =>
            {
                Some(slot)
            }
            _ => None,
        };
        let (list, index, value) =
            assignment_operands(self, list, index, expression, Self::operand);
        if let Some(slot) = fixed_slot {
            self.fixed_int_set(slot, &index.value, &value.value);
        } else {
            self.set_item(&list.value, &index.value, &value.value, nonnegative);
        }
        if self.module.types.mapping(list.value.ty).is_some()
            && self
                .module
                .types
                .may_own_file(value.value.ty, &self.module.class_fields, true)
        {
            // The assignment's stack keeps the RHS alive while the
            // temporary dictionary releases its remaining entries.
            self.release_operand(&list);
            self.release_operand(&index);
            self.release_operand(&value);
        } else {
            self.release_operand(&value);
            self.release_operand(&index);
            self.release_operand(&list);
        }
    }

    fn prepare_assignment(&mut self, slot: usize, expression: &Expr) -> Option<bool> {
        if let Some(cursor) = self.forward_cursors.get(&slot).cloned() {
            self.assign_forward_cursor(slot, expression, cursor);
            return None;
        }
        if self.line_slots[slot] {
            self.assign_line(slot, expression);
            return None;
        }
        if let ExprKind::Builtin(Builtin::SplitIntSource(mask), args) = &expression.kind {
            let value = self.expr(&args[0]);
            self.line(format!("call void @imre_str_prepare_int_fields(ptr {}, ptr %split_fields{slot}, i32 {mask})", value.operand));
            self.store(slot, &value);
            return None;
        }
        if let ExprKind::Builtin(Builtin::SplitTextSource(fields), args) = &expression.kind {
            let value = self.expr(&args[0]);
            self.line(format!(
                "call void @imre_str_field_spans(ptr {}, ptr %text_fields{slot}, i32 {fields})",
                value.operand
            ));
            self.store(slot, &value);
            return None;
        }
        if self.locals[slot] == Type::ByteOutput
            && matches!(&expression.kind, ExprKind::List(values) if values.is_empty())
        {
            let value = self.call("imre_byte_output_new", Type::ByteOutput, &[]);
            if self.byte_output_owners.get(slot) == Some(&Some(slot)) {
                self.line(format!(
                    "store {{ ptr, i64, i64 }} zeroinitializer, ptr %byte_window{slot}"
                ));
            }
            self.store(slot, &value);
            return None;
        }
        if self.struct_locals[slot] {
            let value = self
                .struct_snapshot(expression)
                .expect("checked scalar struct source");
            self.store_borrowed(slot, &value);
            return None;
        }
        if self.borrowed_locals[slot] {
            let value = self.borrow_projection(expression);
            self.store_borrowed(slot, &value);
            return None;
        }
        if self.concat_assign(slot, expression) {
            return None;
        }
        if !self.loops.is_empty()
            && let Some(fields) = records::replacement_fields(self.module, expression)
        {
            self.assign_loop_record(slot, fields);
            return None;
        }
        Some(self.assignment_replaces_small(slot, expression))
    }

    fn assign_native(&mut self, slot: usize, value: CheckedValue, previous_small: bool) {
        let value = value.into_native().value;
        self.store_with_previous_small(slot, &value, previous_small);
        if self.fixed_int_lists[slot] {
            self.remember_int_buffer(slot, &value);
        }
    }

    fn consume_expression(&mut self, expression: &Expr) -> bool {
        // A discarded scalar literal has no effects. In particular, a function
        // docstring must not allocate on every call.
        if matches!(
            expression.kind,
            ExprKind::Int(_)
                | ExprKind::Float(_)
                | ExprKind::Bool(_)
                | ExprKind::Str(_)
                | ExprKind::Bytes(_)
                | ExprKind::None
        ) {
            true
        } else if let ExprKind::Builtin(Builtin::TextWrite, args) = &expression.kind {
            self.write_discard(args);
            true
        } else {
            false
        }
    }

    fn return_result(&mut self, value: CheckedValue) {
        let value = value.into_native().value;
        self.forget(&value);
        self.route(super::Completion::Return(value));
    }

    fn specialized_binary(
        &mut self,
        op: Binary,
        left: &Expr,
        right: &Expr,
        expression: &Expr,
    ) -> Option<CheckedValue> {
        if matches!(op, Binary::Concat) && expression.ty == Type::Str {
            return Some(CheckedValue::owned(self.concat(expression)));
        }
        if matches!(op, Binary::Add)
            && matches!(&left.kind, ExprKind::Builtin(Builtin::IntBounded { .. }, _))
            && let Some(slot) = self.active_subtotal().map(str::to_owned)
        {
            let subtotal = self.instruction(format!("load i64, ptr {slot}"));
            let term = self.operand(right);
            let decoded = self.instruction(format!("ashr i64 {}, 1", term.value.operand));
            let updated = self.instruction(format!("add nsw i64 {subtotal}, {decoded}"));
            self.release_operand(&term);
            self.line(format!("store i64 {updated}, ptr {slot}"));
            return Some(CheckedValue::owned(self.tag_small(&updated)));
        }
        let adjust_floor = self.small_binary_proof(expression, left, right, op)?;
        let (left_value, right_value) = (self.operand(left), self.operand(right));
        let bounds = self.ranges.integer(left).zip(self.ranges.integer(right));
        let result = self.small_binary(
            op,
            &left_value.value,
            &right_value.value,
            adjust_floor,
            bounds,
        );
        self.release_operand(&left_value);
        self.release_operand(&right_value);
        Some(CheckedValue::owned(result))
    }

    fn fused_statements(&mut self, remaining: &[Statement]) -> usize {
        let grouped = self.integer_assignments(remaining);
        if grouped == 0
            && let [statement, next, ..] = remaining
            && (self.move_cleared_local(statement, next) || self.append_pair(statement, next))
        {
            2
        } else {
            grouped
        }
    }
}
