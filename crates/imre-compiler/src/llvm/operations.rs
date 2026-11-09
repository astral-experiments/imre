//! Evaluation order for checked IR. Each storage convention supplies its own value operations.

use super::records;
use super::{Binary, Builtin, Expr, ExprKind, Statement, Type};

pub(crate) trait Operations: Sized {
    type Value;
    type Operand;

    fn operand(&mut self, expr: &Expr) -> Self::Operand;

    fn binary_operands(&mut self, left: &Expr, right: &Expr) -> (Self::Operand, Self::Operand) {
        (self.operand(left), self.operand(right))
    }

    fn call_arguments(
        &mut self,
        callee: &Expr,
        args: &[Expr],
    ) -> (Self::Operand, Vec<Self::Operand>) {
        let callable = self.operand(callee);
        let values = args.iter().map(|arg| self.operand(arg)).collect();
        (callable, values)
    }

    fn evaluate(&mut self, expr: &Expr) -> Self::Value;
    fn integer(&mut self, text: &str) -> Self::Value;
    fn boolean(&mut self, value: bool) -> Self::Value;
    fn none(&mut self) -> Self::Value;
    fn local(&mut self, slot: usize, expr: &Expr) -> Self::Value;
    fn binary_expression(
        &mut self,
        op: Binary,
        left: &Expr,
        right: &Expr,
        expr: &Expr,
    ) -> Self::Value;
    fn builtin(&mut self, op: &Builtin, operands: &[Expr], ty: Type) -> Self::Value;
    fn other_expression(&mut self, expr: &Expr) -> Self::Value;

    fn emit_value(&mut self, expr: &Expr) -> Self::Value {
        match &expr.kind {
            ExprKind::Int(n) => self.integer(n),
            ExprKind::Bool(b) => self.boolean(*b),
            ExprKind::None => self.none(),
            ExprKind::Local(slot) => self.local(*slot, expr),
            ExprKind::Binary(op, left, right) => self.binary_expression(*op, left, right, expr),
            ExprKind::Builtin(op, args) => self.builtin(op, args, expr.ty),
            _ => self.other_expression(expr),
        }
    }

    type Assignment;
    fn prepare_assignment(&mut self, slot: usize, expr: &Expr) -> Option<Self::Assignment>;
    fn assign(&mut self, slot: usize, value: Self::Value, plan: Self::Assignment);
    fn consume_expression(&mut self, expr: &Expr) -> bool;
    fn discard(&mut self, value: Self::Value);
    fn return_result(&mut self, value: Self::Value);
    fn other_statement(&mut self, stmt: &Statement);

    fn emit_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign(slot, expr) => {
                if let Some(plan) = self.prepare_assignment(*slot, expr) {
                    let value = self.evaluate(expr);
                    self.assign(*slot, value, plan);
                }
            }
            Statement::Expr(expr) => {
                if !self.consume_expression(expr) {
                    let value = self.evaluate(expr);
                    self.discard(value);
                }
            }
            Statement::Return(expr) => {
                let value = self.evaluate(expr);
                self.return_result(value);
            }
            _ => self.other_statement(stmt),
        }
    }

    fn terminated(&self) -> bool;
    /// A fixed layout can fuse adjacent statements before ordinary lowering.
    fn fused_statements(&mut self, _remaining: &[Statement]) -> usize {
        0
    }

    fn emit_block(&mut self, mut remaining: &[Statement], after: impl Fn(&Self)) {
        while let Some((stmt, tail)) = remaining.split_first() {
            if self.terminated() {
                break;
            }
            let fused = self.fused_statements(remaining);
            if fused == 0 {
                self.emit_statement(stmt);
                remaining = tail;
            } else {
                remaining = &remaining[fused..];
            }
            after(self);
        }
    }
}

use super::{Emitter, Value, none, small};

impl Operations for Emitter<'_, '_> {
    type Value = Value;
    type Operand = super::Operand;

    fn operand(&mut self, expr: &Expr) -> Self::Operand {
        self.operand(expr)
    }
    type Assignment = bool;

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

    fn assign(&mut self, slot: usize, value: Value, previous_small: bool) {
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

    fn discard(&mut self, value: Value) {
        self.release(&value);
    }

    fn return_result(&mut self, value: Value) {
        self.forget(&value);
        self.route(super::Completion::Return(value));
    }

    fn other_statement(&mut self, statement: &Statement) {
        self.other_statement(statement);
    }

    fn integer(&mut self, value: &str) -> Value {
        if let Ok(value) = value.parse::<i64>()
            && (-(1_i64 << 62)..(1_i64 << 62)).contains(&value)
        {
            small(value)
        } else {
            self.pooled_literal(value.as_bytes(), Type::Int)
        }
    }

    fn boolean(&mut self, value: bool) -> Value {
        Value {
            ty: Type::Bool,
            operand: u8::from(value).to_string(),
            small_integer: false,
        }
    }

    fn local(&mut self, slot: usize, expression: &Expr) -> Value {
        let mut value = self.borrow_slot(slot);
        value.small_integer |= self.ranges.integer(expression).is_some();
        self.retain(&value);
        value
    }

    fn none(&mut self) -> Value {
        none()
    }

    fn binary_expression(
        &mut self,
        op: Binary,
        left: &Expr,
        right: &Expr,
        expression: &Expr,
    ) -> Value {
        if matches!(op, Binary::Concat) && expression.ty == Type::Str {
            return self.concat(expression);
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
            return self.tag_small(&updated);
        }
        let direct = self.small_binary_proof(expression, left, right, op);
        let (left_value, right_value) = self.binary_operands(left, right);
        let result = if let Some(adjust_floor) = direct {
            let bounds = self.ranges.integer(left).zip(self.ranges.integer(right));
            self.small_binary(
                op,
                &left_value.value,
                &right_value.value,
                adjust_floor,
                bounds,
            )
        } else {
            self.binary(op, &left_value.value, &right_value.value)
        };
        self.release_operand(&left_value);
        self.release_operand(&right_value);
        result
    }

    fn builtin(&mut self, op: &Builtin, args: &[Expr], ty: Type) -> Value {
        self.builtin(op, args, ty)
    }

    fn evaluate(&mut self, expression: &Expr) -> Value {
        self.expr(expression)
    }

    fn other_expression(&mut self, expression: &Expr) -> Value {
        self.other_expression(expression)
    }

    fn terminated(&self) -> bool {
        self.terminated
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
