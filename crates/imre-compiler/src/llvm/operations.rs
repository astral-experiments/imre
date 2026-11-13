//! Emit checked operations using the storage and ownership of each operand.

mod comparisons;
mod dictionaries;
mod handlers;
mod iteration;
pub(crate) use iteration::LoopEdges;
pub(crate) mod branches;
mod join;
pub(crate) mod sequences;
use crate::ir::{self, DisplayKind};
pub(crate) use join::ValueJoin;
use ruff_python_ast::UnaryOp;

use super::records;
use super::{Binary, Builtin, CallArgs, Expr, ExprKind, Statement, Type};
use super::{Emitter, Value, none, small};
use crate::llvm::registered::Frame as CheckedFrame;
use ruff_text_size::TextRange;

use super::{Operand, Ownership, body::FunctionBody};

/// A checked expression keeps the representation selected by its producer.
/// A guarded integer carries an owned integer result on the exact edge and the
/// original Python protocol result on the other edge. Zero is not an Int word.
pub(crate) enum CheckedValue {
    Native(Operand, Option<usize>),
    /// A terminal integer operand only needs a Python object on the protocol edge.
    IntegerLiteral(i64),
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ValueUse<'a> {
    General,
    PythonHandle,
    CallOperand(&'a str, &'a std::cell::OnceCell<String>),
    IntegerOperand,
    FinalIntegerOperand,
}

impl ValueUse<'_> {
    fn needs_integer(self) -> bool {
        matches!(self, Self::IntegerOperand | Self::FinalIntegerOperand)
    }
}

/// Only the checked operation that owns a physical location may pass it to a
/// failing protocol call. Synthesized operations have no site of their own.
#[derive(Clone, Copy)]
enum OperationSite {
    Placed(TextRange),
    Registered(TextRange),
    Unlocated,
}

impl CheckedValue {
    pub(crate) fn handle(id: String) -> Self {
        Self::Handle {
            id,
            exact_word: None,
        }
    }
    /// An integer consumer can use the result without boxing it. In that case
    /// one available word is enough to try projecting the other operand.
    fn can_use_integer_arithmetic(
        &self,
        left: Type,
        other: &Self,
        right: Type,
        usage: ValueUse,
    ) -> bool {
        match (self, other) {
            (Self::Handle { exact_word: a, .. }, Self::Handle { exact_word: b, .. }) => {
                match (a, b) {
                    (Some(_), Some(_)) => true,
                    (Some(_), None) => right == Type::Int || usage.needs_integer(),
                    (None, Some(_)) => left == Type::Int || usage.needs_integer(),
                    (None, None) => left == Type::Int || right == Type::Int,
                }
            }
            _ => true,
        }
    }

    fn frame_integer(value: Value, frame: &mut CheckedFrame<'_, '_, '_>, usage: ValueUse) -> Self {
        if matches!(usage, ValueUse::PythonHandle | ValueUse::CallOperand(..)) {
            return Self::handle(frame.take_integer(&value.operand));
        }
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
            Self::Handle { .. } | Self::IntegerLiteral(_) => None,
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
    fn fixed_value(&self) -> &Value {
        let Self::Native(operand, None) = self else {
            unreachable!("fixed-layout operand")
        };
        &operand.value
    }

    fn rooted_handle(&self) -> &str {
        let Self::Handle { id, .. } = self else {
            unreachable!("materialized Python operand")
        };
        id
    }

    pub(super) fn into_native(self) -> Operand {
        let Self::Native(operand, None) = self else {
            unreachable!("native ABI operand")
        };
        operand
    }
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

    pub(super) fn fixed(&mut self) -> &mut Emitter<'a, 'module> {
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

    fn block_name(&mut self) -> String {
        match &mut self.0 {
            Entry::Native(e) => e.label(),
            Entry::Registered(f) => f.block_name(),
        }
    }
    fn jump_open(&mut self, dest: &str) {
        let body = self.body();
        if !body.terminated {
            body.jump(dest);
        }
    }

    /// Emit a conditional suite and continue on the false edge when the true
    /// suite transfers control elsewhere.
    fn when(&mut self, test: &str, otherwise: Option<String>, yes: impl FnOnce(&mut Self)) {
        let take = self.block_name();
        let end = otherwise.unwrap_or_else(|| self.block_name());
        let body = self.body();
        if !body.terminated {
            body.branch(test, &take, &end);
        }
        self.body().start(&take);
        yes(self);
        self.jump_open(&end);
        self.body().start(&end);
    }

    fn operand(&mut self, expr: &Expr) -> CheckedValue {
        self.operand_for(expr, ValueUse::General)
    }

    fn operand_for(&mut self, expr: &Expr, usage: ValueUse) -> CheckedValue {
        match &mut self.0 {
            Entry::Native(emitter) => CheckedValue::native(emitter.operand(expr)),
            Entry::Registered(_) => self.value_for(expr, usage),
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
        let Some(site) = args.site.python() else {
            let native = self.fixed();
            let (target, values) = args.capture_invoke(callable, |expr| native.operand(expr));
            let target = super::callables::NativeCallTarget::Callable(target);
            return CheckedValue::owned(native.native_call(target, args, values, returns));
        };

        self.python_invoke(site, &args.values, |frame, next| {
            frame.kept_call_value(callable, next)
        })
    }

    fn python_invoke(
        &mut self,
        site: &ir::InvokeSite,
        args: &[Expr],
        callable: impl FnOnce(&mut CheckedFrame<'_, '_, '_>, &mut String) -> String,
    ) -> CheckedValue {
        let frame = self.python();
        let exec = frame.exec_literal_at(site.range);
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
            let scope = site
                .scope
                .as_deref()
                .map(|expr| frame.kept_call_value(expr, &mut next));
            let target = callable(frame, &mut next);
            let (array, count) = frame.kept_call_arguments(&target, args, next, exec);
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
                scope.as_deref(),
            )
        });
        CheckedValue::handle(result)
    }

    fn evaluate(&mut self, expr: &Expr) -> CheckedValue {
        self.evaluate_for(expr, ValueUse::General)
    }

    fn evaluate_for(&mut self, expr: &Expr, usage: ValueUse) -> CheckedValue {
        match &mut self.0 {
            Entry::Native(emitter) => CheckedValue::owned(emitter.expr(expr)),
            Entry::Registered(_) => self.value_for(expr, usage),
        }
    }

    /// Evaluate the raised object before its optional cause. A suspended Python cause
    /// saves the first handle in the frame. Fixed owned operands already unwind with
    /// the emitter's other temporaries.
    fn raise_statement(&mut self, raised: Option<&crate::ir::RaisedException>) {
        let Some(raised) = raised else {
            match &mut self.0 {
                Entry::Native(emitter) => {
                    emitter.void_call("imre_exception_reraise", &[]);
                    emitter.body.end("unreachable");
                }
                Entry::Registered(frame) => {
                    let active = frame.caught.last().cloned().unwrap_or_else(|| "0".into());
                    frame.raise(&active, None, true);
                }
            }
            return;
        };
        let statement = match &self.0 {
            Entry::Registered(frame) => Some(frame.current_range),
            Entry::Native(_) => None,
        };
        let exception = self.operand(&raised.exception);
        if let Entry::Registered(frame) = &mut self.0 {
            let handle = frame.materialize(&exception);
            frame.live.push(handle);
        }
        let cause = raised.cause.as_deref().map(|cause| self.operand(cause));
        match &mut self.0 {
            Entry::Native(emitter) => {
                let mut exception = exception.into_native();
                if let Some(cause) = cause {
                    let cause = cause.into_native();
                    let ptr =
                        emitter.instruction(format!("inttoptr i64 {} to ptr", cause.value.operand));
                    let call = format!(
                        "call ptr @imre_exception_with_cause(ptr {}, ptr {ptr})",
                        exception.value.operand
                    );
                    exception = emitter.attach_exception(exception, call);
                    emitter.release_operand(&cause);
                }
                if let Some(origin) = &raised.origin {
                    let call = emitter.exception_frame(&exception.value.operand, origin);
                    exception = emitter.attach_exception(exception, call);
                }
                emitter.void_call("imre_exception_raise", &[&exception.value]);
                emitter.release_operand(&exception);
                emitter.body.end("unreachable");
            }
            Entry::Registered(frame) => {
                let cause = cause.map(|value| frame.materialize_owned(value));
                let exception = frame.live.pop().expect("pending raised exception");
                frame.raise_operand_at(
                    &exception,
                    cause.as_deref(),
                    statement.expect("registered raise site"),
                );
            }
        }
    }

    fn release_input(&mut self, value: CheckedValue) {
        match value {
            CheckedValue::Handle { .. } | CheckedValue::IntegerLiteral(_) => {}
            CheckedValue::Native(operand, None) => self.fixed().release_operand(&operand),
            CheckedValue::Native(_, Some(owner)) | CheckedValue::IntOrHandle { owner, .. } => {
                self.python().release_word(owner);
            }
        }
    }

    fn integer_operand(&mut self, value: &CheckedValue) -> (String, bool) {
        match value {
            CheckedValue::Native(v, _) => (v.value.operand.clone(), false),
            CheckedValue::IntegerLiteral(n) => (small(*n).operand, false),
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
        span: Option<[usize; 4]>,
    ) -> CheckedValue {
        let frame = self.python();
        let lhs = frame.materialize(left);
        let rhs = frame.materialize(right);
        CheckedValue::handle(frame.op_at(op.protocol(), &[&lhs, &rhs], span))
    }

    /// A positive power-of-two remainder fits in the permanent Python integer
    /// cache when its mask does. Both edges can then return an ordinary handle.
    fn cached_remainder(
        &mut self,
        left: &CheckedValue,
        right: &CheckedValue,
        span: Option<[usize; 4]>,
        divisor: &Expr,
    ) -> Option<CheckedValue> {
        use super::registered::permanent;
        let literal = match &divisor.kind {
            ExprKind::Builtin(Builtin::RegisteredLocation(..), operands) => &operands[0],
            _ => divisor,
        };
        let divisor = literal.tagged_integer_literal()?;
        let mask = super::remainder_mask(divisor)?;
        let divisor_id = permanent::small_integer_id(divisor)?;
        if let CheckedValue::Handle {
            id,
            exact_word: None,
        } = left
        {
            let result = self.python().masked_remainder(id, divisor_id);
            return Some(CheckedValue::handle(result));
        }
        // A preceding integer operation already produced a tagged word.
        // Keep it native until an actual protocol call needs the Python object.
        let zero = permanent::small_integer_id(0).expect("zero is a permanent integer");
        let (word, _) = self.integer_operand(left);
        // Both missing projections and heap integers have a clear low bit.
        let immediate = self.body().instruction(format!("trunc i64 {word} to i1"));
        ValueJoin::select(
            self,
            Self::body,
            |code, yes, no| code.body().branch(&immediate, yes, no),
            |code| {
                let raw = code.body().instruction(format!("ashr i64 {word}, 1"));
                let remainder = code.body().masked_remainder(&raw, mask);
                let id = code
                    .body()
                    .instruction(format!("add i64 {remainder}, {zero}"));
                CheckedValue::handle(id)
            },
            |code| code.python_binary(Binary::Modulo, left, right, span),
        )
    }

    fn integer_arithmetic(
        &mut self,
        op: Binary,
        left: &CheckedValue,
        right: &CheckedValue,
        span: Option<[usize; 4]>,
        usage: ValueUse,
    ) -> CheckedValue {
        let (a, check_a) = self.integer_operand(left);
        let (b, check_b) = self.integer_operand(right);
        if let Entry::Native(emitter) = &mut self.0 {
            return CheckedValue::owned(emitter.integer_arithmetic(op, &a, &b));
        }
        let out = self.body();
        // A declined projection is zero. Integer zero is the tagged word one.
        // The Python frame owns the exception when a divisor is zero.
        let right_guard = |out: &mut FunctionBody| match op {
            Binary::FloorDivide | Binary::Modulo => out.instruction(format!("icmp ugt i64 {b}, 1")),
            Binary::RightShift => {
                // A nonnegative immediate count cannot raise. Let Python handle heap counts
                // (of either sign), negative counts and overridden operators.
                let immediate = out.instruction(format!("trunc i64 {b} to i1"));
                let nonnegative = out.instruction(format!("icmp sge i64 {b}, 1"));
                out.instruction(format!("and i1 {immediate}, {nonnegative}"))
            }
            _ => out.instruction(format!("icmp ne i64 {b}, 0")),
        };
        let guard = match (
            check_a,
            check_b
                || matches!(
                    op,
                    Binary::FloorDivide | Binary::Modulo | Binary::RightShift
                ),
        ) {
            (false, false) => None,
            (true, false) => Some(out.instruction(format!("icmp ne i64 {a}, 0"))),
            (false, true) => Some(right_guard(out)),
            (true, true) => {
                let x = out.instruction(format!("icmp ne i64 {a}, 0"));
                let y = right_guard(out);
                Some(out.instruction(format!("and i1 {x}, {y}")))
            }
        };
        let exact = |code: &mut Self| {
            let value = code.body().integer_arithmetic(op, &a, &b, false);
            CheckedValue::frame_integer(value, code.python(), usage)
        };
        let Some(guard) = guard else {
            return exact(self);
        };
        ValueJoin::select(
            self,
            Self::body,
            |code, yes, no| code.body().branch(&guard, yes, no),
            exact,
            |code| code.python_binary(op, left, right, span),
        )
        .expect("guarded integer arithmetic has a normal result")
    }

    /// Root a Python handle before evaluating another operand that could replace its owner.
    fn rooted_operand(&mut self, expr: &Expr) -> CheckedValue {
        let value = self.operand_for(expr, ValueUse::PythonHandle);
        match &mut self.0 {
            Entry::Registered(frame) if !matches!(value, CheckedValue::Handle { .. }) => {
                CheckedValue::handle(frame.materialize_owned(value))
            }
            _ => value,
        }
    }

    /// Capture the container before the item. A packed record can write its fields directly.
    fn insert(&mut self, op: &Builtin, exprs: &[Expr]) -> CheckedValue {
        let Some(Ok(crate::ir::IterationOperands::Insert([target, item]))) =
            op.iteration_operands(exprs)
        else {
            unreachable!("checked collection operands");
        };
        let container = self.rooted_operand(target);
        if let Entry::Native(emitter) = &mut self.0
            && matches!(op, Builtin::Append)
            && let Some(row) = emitter.packed_constructor(item)
        {
            emitter.append_packed(container.fixed_value(), row);
            emitter.release_operand(&container.into_native());
            return CheckedValue::owned(none());
        }
        let spread = item.spread_operand();
        let value = self.rooted_operand(spread.unwrap_or(item));
        match &mut self.0 {
            Entry::Native(emitter) => {
                let container = container.into_native();
                let value = value.into_native();
                if matches!(op, Builtin::SetAdd) {
                    emitter.set_member(&container.value, &value.value, true);
                    emitter.release_operand(&container);
                    emitter.release_operand(&value);
                } else {
                    if let Some(window) = emitter.byte_output_window(target) {
                        emitter.append_private_bytes(window, &container.value, &value.value, None);
                        emitter.release_operand(&value);
                    } else {
                        emitter.append(&container.value, &value);
                    }
                    emitter.release_operand(&container);
                }
                CheckedValue::owned(none())
            }
            Entry::Registered(frame) => {
                let operation = if spread.is_some() { "extend" } else { "append" };
                CheckedValue::handle(frame.op(
                    operation,
                    &[container.rooted_handle(), value.rooted_handle()],
                ))
            }
        }
    }

    /// Keep the receiver alive across key evaluation and release both operands after deletion.
    fn delete_item(&mut self, receiver: &Expr, key: &Expr) -> CheckedValue {
        let roots = match &mut self.0 {
            Entry::Registered(frame) => Some((frame.temporary_mark(), frame.begin_operand_scope())),
            Entry::Native(_) => None,
        };
        let mut next_mark = None;
        let [receiver, key] = [receiver, key].map(|expr| {
            let value = self.rooted_operand(expr);
            if let Some((mark, _)) = &roots {
                next_mark = Some(self.python().keep_operands(
                    next_mark.as_deref().unwrap_or(mark),
                    value.rooted_handle(),
                    "0",
                ));
            }
            value
        });
        if let Some((mark, failure)) = roots {
            let frame = self.python();
            let result = frame.op("delitem", &[receiver.rooted_handle(), key.rooted_handle()]);
            frame.finish_unwind(failure, |frame| frame.release_owned_operands(&mark));
            frame.release_owned_operands(&mark);
            return CheckedValue::handle(result);
        }
        let receiver = receiver.into_native();
        let key = key.into_native();
        self.fixed()
            .at_builtin_origin(&Builtin::DeleteItem, |code| {
                let word = code.key_word(&key.value);
                code.checked_line(format!(
                    "call void @imre_dict_delete(ptr {}, i64 {word})",
                    receiver.value.operand
                ));
                code.release_operand(&receiver);
                code.release_operand(&key);
            });
        CheckedValue::owned(none())
    }

    fn subscript(&mut self, op: &Builtin, operands: &[Expr], returns: Type) -> CheckedValue {
        let [base_expr, key_expr] = operands else {
            unreachable!("checked subscript operands")
        };
        if let Entry::Native(emitter) = &mut self.0
            && let Some(value) = emitter.selected_split(op, operands)
        {
            return CheckedValue::owned(value);
        }
        // A receiver that came from Python must already be rooted before the key can call back.
        let base = self.rooted_operand(base_expr);
        if let (Entry::Registered(frame), Builtin::RegisteredSubscript(..)) = (&mut self.0, op)
            && let Some(value) = frame.slice_subscript(op, base.rooted_handle(), key_expr)
        {
            return value;
        }
        let key = self.operand(key_expr);
        match &mut self.0 {
            Entry::Native(emitter) => CheckedValue::owned(emitter.at_builtin_origin(op, |e| {
                let base = base.into_native();
                let key = key.into_native();
                let result = if let Some(slot) = e.fixed_int_buffer_slot(base_expr) {
                    e.fixed_int_get(slot, &key.value)
                } else {
                    let nonnegative = matches!(op, Builtin::ListGet(_) | Builtin::BytesGet)
                        && e.nonnegative_index(key_expr);
                    e.get_item(&base.value, &key.value, returns, nonnegative)
                };
                e.release_operand(&base);
                e.release_operand(&key);
                result
            })),
            Entry::Registered(frame) => frame.subscript_at(op, |frame, span| {
                frame.read_index(base.rooted_handle(), &key, span)
            }),
        }
    }

    fn unary_expression(&mut self, op: UnaryOp, operand: &Expr, expr: &Expr) -> CheckedValue {
        let (scope, direct) = match &mut self.0 {
            Entry::Native(emitter) => (
                None,
                emitter.ranges.integer(expr).is_some() && emitter.ranges.integer(operand).is_some(),
            ),
            Entry::Registered(frame) => (Some(frame.begin_unary()), false),
        };
        let value = self.operand(operand);
        let result = self.unary_value(op, value, direct);
        if let Some(scope) = scope {
            self.python().finish_unary(scope, op, result)
        } else {
            result
        }
    }

    fn unary_value(&mut self, op: UnaryOp, value: CheckedValue, direct: bool) -> CheckedValue {
        match value {
            CheckedValue::Native(input, None) => {
                let emitter = self.fixed();
                let scalar = &input.value;
                let result = match op {
                    UnaryOp::Not => Value {
                        ty: Type::Bool,
                        operand: emitter.instruction(format!("xor i1 {}, true", scalar.operand)),
                        small_integer: false,
                    },
                    UnaryOp::USub if scalar.ty == Type::Float => Value {
                        ty: Type::Float,
                        operand: emitter.instruction(format!("fneg double {}", scalar.operand)),
                        small_integer: false,
                    },
                    UnaryOp::USub if direct => {
                        emitter.small_binary(Binary::Subtract, &small(0), scalar, false, None)
                    }
                    UnaryOp::USub => emitter.binary(Binary::Subtract, &small(0), scalar),
                    UnaryOp::Invert if direct => {
                        emitter.small_binary(Binary::BitXor, scalar, &small(-1), false, None)
                    }
                    UnaryOp::Invert => emitter.binary(Binary::BitXor, scalar, &small(-1)),
                    UnaryOp::UAdd => return CheckedValue::Native(input, None),
                };
                emitter.release_operand(&input);
                CheckedValue::owned(result)
            }
            _ => {
                let frame = self.python();
                let operand = frame.materialize_owned(value);
                let name = match op {
                    UnaryOp::UAdd => "pos",
                    UnaryOp::USub => "neg",
                    UnaryOp::Invert => "~",
                    UnaryOp::Not => "not",
                };
                CheckedValue::handle(frame.op_at_current_range(name, &[&operand]))
            }
        }
    }

    fn binary_expression(
        &mut self,
        op: Binary,
        left: &Expr,
        right: &Expr,
        expr: &Expr,
        site: OperationSite,
        usage: ValueUse,
    ) -> CheckedValue {
        if let Entry::Native(emitter) = &mut self.0
            && let Some(value) = emitter.specialized_binary(op, left, right, expr)
        {
            return value;
        }
        let checked_ints =
            left.checked_operand_type() == Type::Int && right.checked_operand_type() == Type::Int;
        // Refining a Python operand for arithmetic must preserve its original error site.
        let positioned = matches!(op, Binary::Add | Binary::Subtract | Binary::Multiply)
            || op == Binary::Modulo
                && [left, right].into_iter().all(|operand| {
                    operand.ty == Type::Int
                        || matches!(operand.without_location().kind, ExprKind::Int(_))
                });
        let arithmetic = matches!(
            op,
            Binary::FloorDivide | Binary::Modulo | Binary::RightShift
        ) && checked_ints
            || positioned
                && (matches!(site, OperationSite::Placed(..))
                    || expr.ty == Type::PythonObject
                    || checked_ints);
        let (left_use, right_use) =
            if arithmetic && (op == Binary::Multiply || checked_ints || usage.needs_integer()) {
                // The left operand must be ready before the right can call Python.
                (ValueUse::IntegerOperand, ValueUse::FinalIntegerOperand)
            } else {
                (ValueUse::General, ValueUse::General)
            };
        // On a placed body the cursor follows execution's preorder walk. Read
        // each operand's range before evaluating it, including an entire nested lhs.
        let mut operand = |expr, usage| {
            let range = match (&self.0, site) {
                (
                    Entry::Registered(frame),
                    OperationSite::Placed(_) | OperationSite::Registered(_),
                ) => frame.operand_range(expr),
                _ => None,
            };
            (self.operand_for(expr, usage), range)
        };
        let (a, lhs) = operand(left, left_use);
        let (b, rhs) = operand(right, right_use);
        let span = match (&self.0, site, lhs, rhs) {
            (
                Entry::Registered(frame),
                OperationSite::Placed(range) | OperationSite::Registered(range),
                Some(lhs),
                Some(rhs),
            ) => frame.binary_span(range, lhs, rhs, op),
            _ => None,
        };
        let result = if let Entry::Native(emitter) = &mut self.0
            && let Some(value) =
                emitter.bounded_binary(op, expr, (left, a.fixed_value()), (right, b.fixed_value()))
        {
            CheckedValue::owned(value)
        } else if op == Binary::Modulo
            && checked_ints
            && matches!(self.0, Entry::Registered(_))
            && let Some(value) = self.cached_remainder(&a, &b, span, right)
        {
            value
        } else if arithmetic {
            if op == Binary::Multiply
                || checked_ints
                || a.can_use_integer_arithmetic(left.ty, &b, right.ty, usage)
            {
                self.integer_arithmetic(op, &a, &b, span, usage)
            } else {
                self.python_binary(op, &a, &b, span)
            }
        } else if let (CheckedValue::Native(x, None), CheckedValue::Native(y, None)) = (&a, &b) {
            CheckedValue::owned(self.fixed().binary(op, &x.value, &y.value))
        } else {
            self.python_binary(op, &a, &b, span)
        };
        self.release_input(a);
        self.release_input(b);
        result
    }

    pub(crate) fn emit_value(&mut self, expr: &Expr) -> CheckedValue {
        self.value_for(expr, ValueUse::General)
    }

    pub(super) fn value_for(&mut self, expr: &Expr, usage: ValueUse) -> CheckedValue {
        if let Entry::Registered(frame) = &mut self.0
            && let Some(range) = frame.next_fixed_site()
        {
            let attribute = frame.fixed_attribute(range);
            use crate::frontend::FixedAttribute;
            let position = match attribute {
                Some(FixedAttribute::Operation { position, .. }) => *position,
                _ => range.start(),
            };
            return frame.at_expression(range, position, |frame| {
                if let Some(FixedAttribute::Call(call)) = attribute {
                    let (target, site) = call.as_ref();
                    let args = expr
                        .named_call_arguments()
                        .expect("placed call has its written operands");
                    return CheckedBody::registered(frame.reborrow()).python_invoke(
                        site,
                        args,
                        |frame, next| frame.source_callee(target, next),
                    );
                }
                if let Some(FixedAttribute::Operation { operation, .. }) = attribute {
                    let operands = match &expr.kind {
                        ExprKind::Field(receiver, _) => std::slice::from_ref(receiver.as_ref()),
                        ExprKind::Builtin(_, args) => args,
                        _ => unreachable!("placed attribute has its checked operands"),
                    };
                    return frame.builtin(operation, operands, usage);
                }
                CheckedBody::registered(frame.reborrow()).emit_operation(
                    expr,
                    OperationSite::Placed(range),
                    usage,
                )
            });
        }
        self.emit_operation(expr, OperationSite::Unlocated, usage)
    }

    fn emit_operation(
        &mut self,
        expr: &Expr,
        site: OperationSite,
        usage: ValueUse,
    ) -> CheckedValue {
        match &expr.kind {
            ExprKind::Builtin(Builtin::RegisteredLocation(range, position, ..), operands) => {
                self.python().at_expression(*range, *position, |frame| {
                    CheckedBody::registered(frame.reborrow()).emit_operation(
                        &operands[0],
                        OperationSite::Registered(*range),
                        usage,
                    )
                })
            }
            ExprKind::Int(text) => {
                let integer = text.parse::<i64>().ok();
                let tagged = integer.and_then(crate::ir::tagged_integer);
                match &mut self.0 {
                    Entry::Native(emitter) => CheckedValue::owned(
                        tagged
                            .map(small)
                            .unwrap_or_else(|| emitter.pooled_literal(text.as_bytes(), Type::Int)),
                    ),
                    Entry::Registered(frame) => match integer {
                        Some(n)
                            if usage == ValueUse::FinalIntegerOperand
                                && tagged.is_some()
                                && super::registered::permanent::small_integer_id(n).is_none() =>
                        {
                            CheckedValue::IntegerLiteral(n)
                        }
                        Some(n) => CheckedValue::Handle {
                            id: frame.integer(n),
                            exact_word: tagged.map(|value| small(value).operand),
                        },
                        None => CheckedValue::handle(frame.lit(0, text.as_bytes())),
                    },
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
                    Entry::Registered(frame) => CheckedValue::handle(frame.sequence(
                        items,
                        if tuple {
                            DisplayKind::Tuple
                        } else {
                            DisplayKind::List
                        },
                    )),
                }
            }
            ExprKind::Set { values, .. } => match &mut self.0 {
                Entry::Native(emitter) => {
                    CheckedValue::owned(emitter.native_sequence(expr.ty, values, false))
                }
                Entry::Registered(frame) => {
                    CheckedValue::handle(frame.sequence(values, DisplayKind::Set))
                }
            },
            ExprKind::Compare(values, ops) => {
                let truth = matches!(self.0, Entry::Native(_));
                self.comparisons(values, ops, truth)
            }
            ExprKind::Conditional(test, yes, no) => self.conditional_value(test, yes, no),
            ExprKind::Boolean(..) | ExprKind::BooleanValue(..) => self.boolean_expression(expr),
            ExprKind::Builtin(
                op @ (Builtin::ListGet(_)
                | Builtin::TupleGet
                | Builtin::TupleIndex
                | Builtin::StringGet
                | Builtin::BytesGet
                | Builtin::DictGet
                | Builtin::RegisteredSubscript(..)),
                args,
            ) => self.subscript(op, args, expr.ty),
            ExprKind::Builtin(Builtin::DeleteItem, args) => {
                let [receiver, key] = args.as_slice() else {
                    unreachable!("checked deletion operands")
                };
                self.delete_item(receiver, key)
            }
            ExprKind::Builtin(op @ (Builtin::Append | Builtin::SetAdd), args) => {
                self.insert(op, args)
            }
            ExprKind::Binary(op, left, right) => {
                self.binary_expression(*op, left, right, expr, site, usage)
            }
            ExprKind::Builtin(Builtin::FormatJoin, parts) => match &mut self.0 {
                Entry::Native(emitter) => {
                    CheckedValue::owned(emitter.concat(parts.iter().collect()))
                }
                Entry::Registered(frame) => {
                    let mark = frame.temporary_mark();
                    CheckedValue::handle(frame.unwind_stack_operands(&mark, |frame| {
                        let (values, count) = frame.evaluate_handle_array(parts);
                        frame.checked(&format!(
                            "call i64 @imre_dyn_format_parts(ptr %ctx, i64 {mark}, ptr {values}, i64 {count})"
                        ))
                    }))
                }
            },
            ExprKind::Invoke { callable, args } => self.invoke(callable, args, expr.ty),
            ExprKind::Unary(op, value) => self.unary_expression(*op, value, expr),
            ExprKind::Scope { body, value, clear } => self.scoped_expression(body, value, clear),
            ExprKind::Dict(entries) => self.dictionary(expr.ty, entries),
            ExprKind::Builtin(op, args) => match &mut self.0 {
                Entry::Native(emitter) => CheckedValue::owned(emitter.builtin(op, args, expr.ty)),
                Entry::Registered(frame) => frame.builtin(op, args, usage),
            },
            // Python handles already represent both the payload and None. Evaluating
            // the original operand also preserves effects of a None-valued call.
            ExprKind::Present(value) | ExprKind::Absent(value) | ExprKind::Unwrap(value)
                if matches!(self.0, Entry::Registered(_)) =>
            {
                self.operand_for(value, usage)
            }
            ExprKind::Present(value) => {
                let emitter = self.fixed();
                let value = emitter.operand(value);
                let value = emitter.own_operand(value);
                CheckedValue::owned(emitter.present(value, expr.ty))
            }
            ExprKind::Absent(value) => {
                let emitter = self.fixed();
                let value = emitter.expr(value);
                emitter.release(&value);
                CheckedValue::owned(Value {
                    ty: expr.ty,
                    ..none()
                })
            }
            ExprKind::Unwrap(value) => {
                CheckedValue::owned(self.fixed().unwrap_optional(value, expr.ty))
            }
            ExprKind::Call(id, args) => {
                CheckedValue::owned(self.fixed().direct_call(*id, args, expr.ty))
            }
            ExprKind::Callable { function, receiver } => CheckedValue::owned(
                self.fixed()
                    .callable_value(expr.ty, *function, receiver.as_deref()),
            ),
            ExprKind::Construct { initializer, args } => {
                CheckedValue::owned(self.fixed().construct_record(expr.ty, *initializer, args))
            }
            ExprKind::Exception { kind, message } => {
                CheckedValue::owned(self.fixed().exception_value(*kind, message.as_deref()))
            }
            ExprKind::Field(receiver, index) => {
                let emitter = self.fixed();
                let value = if let Some(snapshot) = emitter.struct_snapshot(receiver) {
                    let ptr = emitter.field_pointer(&snapshot, *index);
                    emitter.field_value(&ptr, expr.ty)
                } else {
                    let receiver = emitter.immediate_receiver(receiver);
                    let value = emitter.get_field(&receiver.value, *index, expr.ty);
                    emitter.release_operand(&receiver);
                    value
                };
                CheckedValue::owned(value)
            }
        }
    }

    pub(crate) fn emit_statement(&mut self, stmt: &Statement) {
        if let Entry::Registered(frame) = &mut self.0 {
            match (stmt, frame.next_fixed_site()) {
                (Statement::Position(range, simple), _) => frame.statement_site(*range, *simple),
                (Statement::Break(Some(range)) | Statement::Continue(range), _) => {
                    frame.statement_site(*range, None)
                }
                (Statement::SetField(receiver, _, value), Some(site)) => {
                    frame.statement_site(site, None);
                    let Some(crate::frontend::FixedAttribute::Operation { operation, .. }) =
                        frame.fixed_attribute(site)
                    else {
                        unreachable!("placed field store retains its attribute operation");
                    };
                    frame.attribute_operation(
                        operation,
                        [value, receiver].into_iter(),
                        ValueUse::General,
                    );
                    return;
                }
                (_, Some(range)) => frame.statement_site(range, None),
                (_, None) => {}
            }
        }
        match stmt {
            Statement::Position(..) => {}
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
                let value = self.evaluate_for(expr, ValueUse::PythonHandle);
                match &mut self.0 {
                    Entry::Native(emitter) => emitter.assign_native(*slot, value, previous_small),
                    Entry::Registered(frame) => frame.assign(*slot, value),
                }
            }
            Statement::AugmentItem(receiver, key, op, rhs) => {
                self.augment_item(receiver, key, *op, rhs)
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
                let value = self.evaluate_for(expr, ValueUse::PythonHandle);
                match &mut self.0 {
                    Entry::Native(emitter) => emitter.return_result(value),
                    Entry::Registered(frame) => frame.return_result(value),
                }
            }
            Statement::Raise(raised) => self.raise_statement(raised.as_ref()),
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => self.try_statement(body, handlers, orelse, finalbody),
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
            Statement::SetItem(order, table, key, value) => {
                self.store_item(*order, table, key, value)
            }
            Statement::IterNext {
                iterator,
                key,
                value,
                ready,
                kind,
            } => self.pull(*iterator, *key, *value, *ready, *kind),
            Statement::Break(_) | Statement::Continue(_) | Statement::Exhaust => {
                self.loop_transfer(stmt)
            }
            Statement::Move(target, source) => self.fixed().move_local(*target, *source),
            Statement::SetField(receiver, field, rhs) => {
                let code = self.fixed();
                let value = code.operand(rhs);
                let receiver = code.operand(receiver);
                code.set_field(&receiver.value, *field, &value.value);
                code.release_operand(&receiver);
                code.release_operand(&value);
            }
            Statement::AugmentField(receiver, field, op, rhs) => {
                let code = self.fixed();
                let receiver = code.operand(receiver);
                let old = code.field_update_operand(&receiver.value, *field, rhs);
                let right = code.operand(rhs);
                let value = code.binary(*op, &old.value, &right.value);
                code.forget(&value);
                code.replace_field(&receiver.value, *field, &value);
                code.release_operand(&right);
                code.release_operand(&old);
                code.release_operand(&receiver);
            }
            Statement::For {
                target,
                cursor,
                stop,
                step,
                args,
                body,
                orelse,
            } => {
                let code = self.fixed();
                // The proof covers the hidden cursor and its final increment,
                // independently of reassignment of the visible target.
                let small_cursor = code.ranges.range_cursor(stmt).is_some();
                code.range_loop(
                    small_cursor,
                    [*target, *cursor, *stop, *step],
                    args,
                    body,
                    orelse,
                );
            }
        }
    }

    /// Install fixed list views after the last operation that can invalidate their backing storage.
    pub(crate) fn emit_function(&mut self, function: &crate::ir::Function) {
        let mut statements = function.body.as_slice();
        let list_views = match &mut self.0 {
            Entry::Native(code) => code.int_buffer_region(function),
            Entry::Registered(_) => None,
        };
        if let Some((start, slots)) = list_views {
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
                Entry::Registered(frame) if !frame.terminated() => {
                    frame.pull_and_exhaust(remaining)
                }
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
    /// Both fixed exception annotations return a retained alias of the original.
    fn attach_exception(&mut self, previous: Operand, call: String) -> Operand {
        let operand = self.instruction(call);
        self.release_operand(&previous);
        Operand {
            value: self.owned(Value {
                ty: Type::Exception,
                operand,
                small_integer: false,
            }),
            ownership: Ownership::Owned,
        }
    }

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
        if expression.string_addends().is_some() {
            let parts = super::concat_leaves(self, expression);
            return Some(CheckedValue::owned(self.concat(parts)));
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
        None
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
