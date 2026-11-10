//! Emit checked operations using the storage and ownership of each operand.

mod comparisons;
pub(crate) use comparisons::ComparisonChain;
mod iteration;
pub(crate) use iteration::{LoopEdges, loop_region};
pub(crate) mod branches;
mod join;
pub(crate) mod sequences;
use crate::ir::{DisplayKind, ObjectUnary};
pub(crate) use join::{ValueJoin, choose};

use super::records;
use super::{Binary, Builtin, CallArgs, Expr, ExprKind, Statement, Type};
use super::{Emitter, Value, none, small};
use crate::llvm::registered::{Frame as CheckedFrame, FrameEmitter};

use super::{Operand, Ownership, body::FunctionBody};

/// A checked expression keeps the representation selected by its producer.
/// A guarded integer carries an owned integer result on the exact edge and the
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

    /// Keep an available native result through addition or subtraction. Two rooted
    /// handles need either two known words or a checked integer whose word
    /// still needs a guard. One known word alone cannot justify probing its peer.
    fn can_use_integer_arithmetic(&self, left: Type, other: &Self, right: Type) -> bool {
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

impl CallArgs {
    /// Parameter order applies at the ABI boundary, after source-order evaluation.
    pub(super) fn capture<T>(&self, operand: impl FnMut(&Expr) -> T) -> Vec<T> {
        self.values.iter().map(operand).collect()
    }

    fn capture_invoke<T>(
        &self,
        callable: &Expr,
        mut operand: impl FnMut(&Expr) -> T,
    ) -> (T, Vec<T>) {
        let callable = operand(callable);
        (callable, self.capture(operand))
    }
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

    pub(crate) fn python(&mut self) -> &mut CheckedFrame<'f, 'a, 'p> {
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

    /// Capture the callable before evaluating each argument. A native operand keeps
    /// its checked borrow or owner; a Python argument must already have its rooted
    /// handle before the next argument can call back into the frame.
    fn invoke(
        &mut self,
        callable: &Expr,
        args: &crate::ir::CallArgs,
        returns: Type,
    ) -> CheckedValue {
        let Some(site) = &args.python_site else {
            let native = self.fixed();
            let (target, values) = args.capture_invoke(callable, |expr| native.operand(expr));
            let target = super::callables::NativeCallTarget::Callable(target);
            return CheckedValue::owned(native.native_call(target, args, values, returns));
        };

        let frame = self.python();
        let mark = frame.temporary_mark();
        let result = frame.unwind_stack_operands(&mark, |frame| {
            if frame.native_suspension
                && let Some(name) = site.suspended_name
            {
                frame.call_position(name);
            }
            // Each evaluation leaves just its result above the previous operands.
            // Release intermediate owners before the next argument can run Python.
            let mut next = mark.clone();
            let enclosing_scope = frame.scope.clone();
            let mut evaluate = |expr: &Expr| {
                let value = frame.evaluate_handle(expr);
                next = frame.keep_operands(&next, &value, "0");
                value
            };
            let scope = site
                .scope
                .as_deref()
                .map(&mut evaluate)
                .unwrap_or(enclosing_scope);
            let (target, values) = args.capture_invoke(callable, evaluate);
            let (array, count) = frame.handle_array(&values);
            let positional = super::registered::CallArguments::Positional {
                array,
                count,
                receiver_slot: false,
            };
            frame.positioned_call(
                &target,
                positional,
                &mark,
                site.range,
                site.callee_end,
                &scope,
            )
        });
        CheckedValue::handle(result)
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

    fn finish_integer(&mut self, value: Value) -> CheckedValue {
        match &mut self.0 {
            Entry::Native(emitter) => CheckedValue::owned(emitter.owned(value)),
            Entry::Registered(frame) => CheckedValue::frame_integer(value, frame),
        }
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

    fn python_binary(
        &mut self,
        op: Binary,
        left: &CheckedValue,
        right: &CheckedValue,
    ) -> CheckedValue {
        let frame = self.python();
        let lhs = frame.materialize(left);
        let rhs = frame.materialize(right);
        CheckedValue::handle(frame.binary_handles(op, &lhs, &rhs))
    }

    fn integer_arithmetic(
        &mut self,
        op: Binary,
        left: &CheckedValue,
        right: &CheckedValue,
    ) -> CheckedValue {
        if let Entry::Registered(frame) = &mut self.0 {
            frame.declare_integer_arithmetic(op);
        }
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
            let value = code.body().integer_arithmetic(op, &a, &b);
            code.finish_integer(value)
        };
        let Some(guard) = guard else {
            return exact(self);
        };
        ValueJoin::select(
            self,
            Self::body,
            |code, yes, no| code.body().branch(&guard, yes, no),
            exact,
            |code| code.python_binary(op, left, right),
        )
        .expect("guarded integer arithmetic has a normal result")
    }

    fn unary_expression(&mut self, op: ObjectUnary, operand: &Expr, expr: &Expr) -> CheckedValue {
        if let Entry::Registered(frame) = &mut self.0 {
            let scope = frame.begin_unary();
            let value = self.evaluate(operand);
            let result = self.unary_value(op, value, false);
            return self.python().finish_unary(scope, op, result);
        }
        let direct = self.fixed().ranges.integer(expr).is_some()
            && self.fixed().ranges.integer(operand).is_some();
        let value = self.operand(operand);
        self.unary_value(op, value, direct)
    }

    pub(crate) fn unary_value(
        &mut self,
        op: ObjectUnary,
        value: CheckedValue,
        direct: bool,
    ) -> CheckedValue {
        match value {
            CheckedValue::Native(input, None) => {
                let emitter = self.fixed();
                let scalar = &input.value;
                let result = match op {
                    ObjectUnary::Not => Value {
                        ty: Type::Bool,
                        operand: emitter.instruction(format!("xor i1 {}, true", scalar.operand)),
                        small_integer: false,
                    },
                    ObjectUnary::Negative if scalar.ty == Type::Float => Value {
                        ty: Type::Float,
                        operand: emitter.instruction(format!("fneg double {}", scalar.operand)),
                        small_integer: false,
                    },
                    ObjectUnary::Negative if direct => {
                        emitter.small_binary(Binary::Subtract, &small(0), scalar, false, None)
                    }
                    ObjectUnary::Negative => emitter.binary(Binary::Subtract, &small(0), scalar),
                    _ => unreachable!("fixed checker lowers unary plus and invert"),
                };
                emitter.release_operand(&input);
                CheckedValue::owned(result)
            }
            CheckedValue::Native(_, Some(_)) if op == ObjectUnary::Positive => value,
            _ => {
                let frame = self.python();
                let operand = frame.materialize(&value);
                let name = match op {
                    ObjectUnary::Positive => "pos",
                    ObjectUnary::Negative => "neg",
                    ObjectUnary::Invert => "~",
                    ObjectUnary::Not => "not",
                };
                CheckedValue::handle(frame.op(name, &[&operand]))
            }
        }
    }

    fn binary_expression(
        &mut self,
        op: Binary,
        left: &Expr,
        right: &Expr,
        expr: &Expr,
        fixed_site: bool,
    ) -> CheckedValue {
        if let Entry::Native(emitter) = &mut self.0
            && let Some(value) = emitter.specialized_binary(op, left, right, expr)
        {
            return value;
        }
        // A body placed from a native definition now receives Python handles.
        let op = match (fixed_site, op) {
            (true, Binary::Concat) => Binary::Add,
            (true, Binary::Repeat) => Binary::Multiply,
            _ => op,
        };
        let (a, b) = (self.operand(left), self.operand(right));
        let result = if matches!(op, Binary::Add | Binary::Subtract | Binary::Multiply)
            && (fixed_site
                || expr.ty == Type::PythonObject
                || left.ty == Type::Int && right.ty == Type::Int)
        {
            if matches!(op, Binary::Multiply) || a.can_use_integer_arithmetic(left.ty, &b, right.ty)
            {
                self.integer_arithmetic(op, &a, &b)
            } else {
                self.python_binary(op, &a, &b)
            }
        } else if let (CheckedValue::Native(x, None), CheckedValue::Native(y, None)) = (&a, &b) {
            CheckedValue::owned(self.fixed().binary(op, &x.value, &y.value))
        } else {
            self.python_binary(op, &a, &b)
        };
        self.release_input(a);
        self.release_input(b);
        result
    }

    pub(crate) fn emit_value(&mut self, expr: &Expr) -> CheckedValue {
        if let Entry::Registered(frame) = &mut self.0
            && let Some(range) = frame.next_fixed_site()
        {
            if let ExprKind::Field(receiver, _) = &expr.kind {
                return frame.load_fixed_field(receiver, range);
            }
            return frame.located_value(range, range.start(), |frame| {
                CheckedBody::registered(frame.reborrow()).emit_operation(expr, true)
            });
        }
        self.emit_operation(expr, false)
    }

    fn emit_operation(&mut self, expr: &Expr, fixed_site: bool) -> CheckedValue {
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
            ExprKind::Bind(slot, rhs) => match &mut self.0 {
                Entry::Native(emitter) => {
                    CheckedValue::owned(emitter.bind_value(*slot, rhs, Ownership::Owned))
                }
                Entry::Registered(frame) => {
                    let mark = frame.temporary_mark();
                    let handle = frame.evaluate_handle(rhs);
                    frame.bind_expression(*slot, &mark, &handle);
                    CheckedValue::handle(handle)
                }
            },
            ExprKind::List(items) | ExprKind::Tuple(items) => {
                let tuple = matches!(expr.kind, ExprKind::Tuple(_));
                match &mut self.0 {
                    Entry::Native(emitter) if tuple => {
                        CheckedValue::owned(emitter.tuple(expr.ty, items, None))
                    }
                    Entry::Native(emitter) => {
                        CheckedValue::owned(emitter.native_sequence(expr.ty, items, false))
                    }
                    Entry::Registered(frame) => frame.sequence(
                        items,
                        if tuple {
                            DisplayKind::Tuple
                        } else {
                            DisplayKind::List
                        },
                    ),
                }
            }
            ExprKind::Set { values, .. } => match &mut self.0 {
                Entry::Native(emitter) => {
                    CheckedValue::owned(emitter.native_sequence(expr.ty, values, false))
                }
                Entry::Registered(frame) => frame.sequence(values, DisplayKind::Set),
            },
            ExprKind::Compare(values, ops) => {
                CheckedValue::owned(self.fixed().comparisons(values, ops))
            }
            ExprKind::Builtin(Builtin::RegisteredCompare(ops), values) => {
                self.registered_comparisons(values, ops, false)
            }
            ExprKind::Conditional(test, yes, no) => self.conditional_value(test, yes, no),
            ExprKind::Boolean(..) | ExprKind::BooleanValue(..) => self.boolean_expression(expr),
            ExprKind::Builtin(op @ Builtin::Bool, args) => match &mut self.0 {
                Entry::Native(emitter) => CheckedValue::owned(emitter.builtin(op, args, expr.ty)),
                Entry::Registered(_) => {
                    let Some(Ok(crate::ir::IterationOperands::Truth(value))) =
                        op.iteration_operands(args)
                    else {
                        unreachable!("checked truth operand");
                    };
                    self.predicate(value, false)
                }
            },
            ExprKind::Binary(op, left, right) => {
                self.binary_expression(*op, left, right, expr, fixed_site)
            }
            ExprKind::Builtin(Builtin::FormatJoin, parts) => match &mut self.0 {
                Entry::Native(emitter) => CheckedValue::owned(emitter.concat(expr)),
                Entry::Registered(frame) => frame.builtin(&Builtin::FormatJoin, parts),
            },
            ExprKind::Invoke { callable, args } => self.invoke(callable, args, expr.ty),
            ExprKind::Builtin(
                op @ (Builtin::RegisteredCall(_)
                | Builtin::RegisteredAttribute { .. }
                | Builtin::RegisteredDeleteItem),
                args,
            ) => self.python().object_operation(op, args),
            ExprKind::Builtin(Builtin::RegisteredUnary(op), args) => {
                self.unary_expression(*op, &args[0], expr)
            }
            ExprKind::Negate(value) => self.unary_expression(ObjectUnary::Negative, value, expr),
            ExprKind::Not(value) => self.unary_expression(ObjectUnary::Not, value, expr),
            ExprKind::Scope { body, value, clear } => self.scoped_expression(body, value, clear),
            ExprKind::Dict { .. } if matches!(self.0, Entry::Registered(_)) => {
                CheckedValue::handle(self.python().op("dict", &[]))
            }
            ExprKind::Builtin(op, args) => match &mut self.0 {
                Entry::Native(emitter) => CheckedValue::owned(emitter.builtin(op, args, expr.ty)),
                Entry::Registered(frame) => frame.builtin(op, args),
            },
            _ => CheckedValue::owned(self.fixed().other_expression(expr)),
        }
    }

    pub(crate) fn emit_statement(&mut self, stmt: &Statement) {
        if let Entry::Registered(frame) = &mut self.0
            && let Some(range) = frame.next_fixed_site()
        {
            frame.statement_site(range, None);
        }
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
            Statement::AugmentItem(receiver, key, crate::ir::ItemUpdate::Protocol(op), rhs) => {
                self.python().augment_item(receiver, key, op, rhs);
            }
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
            Statement::Match(subject, cases) => self.emit_prepared_match(subject, cases),
            Statement::Return(expr) => {
                let value = self.evaluate(expr);
                match &mut self.0 {
                    Entry::Native(emitter) => emitter.return_result(value),
                    Entry::Registered(frame) => frame.return_result(value),
                }
            }
            Statement::Raise(exception) => match &mut self.0 {
                Entry::Native(emitter) => emitter.other_statement(stmt),
                Entry::Registered(frame) => frame.raise_exception(exception.as_ref()),
            },
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } if matches!(self.0, Entry::Registered(_)) => {
                self.python().try_region(body, handlers, orelse, finalbody);
            }
            Statement::With {
                manager,
                site,
                binding,
                body,
            } => {
                self.python()
                    .with_manager(manager, *site, binding.as_ref(), body);
            }
            Statement::Clear(slot) => self.clear_iteration_slots(&[*slot]),
            Statement::Scope { body, clear } => self.scoped_suite(body, clear),
            Statement::While(test, body, otherwise) => {
                self.while_suite(stmt, test, body, otherwise)
            }
            Statement::If(test, yes, no) => self.conditional_suite(stmt, test, yes, no),
            Statement::SetItem(table, key, value) => self.store_item(table, key, value),
            Statement::IterNext {
                iterator,
                key,
                value,
                ready,
                kind,
            } => self.pull(*iterator, *key, *value, *ready, *kind),
            Statement::Break | Statement::Continue | Statement::Exhaust => self.loop_transfer(stmt),
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

    pub(crate) fn emit_block(&mut self, mut remaining: &[Statement]) {
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
    /// An immediate consumer may borrow the new binding. Other consumers own the RHS
    /// independently of any conversion or subsequent replacement of the stored value.
    pub(super) fn bind_value(&mut self, slot: usize, rhs: &Expr, ownership: Ownership) -> Value {
        let value = self.expr(rhs);
        if matches!(ownership, Ownership::Owned) {
            self.retain(&value);
        }
        let storage = self.locals[slot];
        let stored = if storage == value.ty {
            value.clone()
        } else if value.ty == Type::None {
            Value {
                ty: storage,
                operand: "0".into(),
                small_integer: false,
            }
        } else if matches!(
            self.module.types.payload(storage).unwrap_or(storage),
            Type::Union(_)
        ) && matches!(
            self.module.types.payload(value.ty).unwrap_or(value.ty),
            Type::Str
                | Type::Bytes
                | Type::List(_)
                | Type::Class(_)
                | Type::Exception
                | Type::NamedException(_)
        ) {
            self.union_inject(value.clone(), storage)
        } else {
            self.present(value.clone(), storage)
        };
        self.store(slot, &stored);
        value
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
