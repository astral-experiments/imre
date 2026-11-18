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

use super::callables::NativeCallTarget;
use super::records;
use super::{Binary, Builtin, CallArgs, Expr, ExprKind, Statement, Type};
use super::{Emitter, Value, none, small};
use crate::llvm::registered::Frame as CheckedFrame;
use ruff_text_size::TextRange;
use std::borrow::Cow;

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
    ComparisonTruth,
    PythonHandle,
    RootedOperand(&'a str, Option<&'a std::cell::OnceCell<String>>),
    IntegerOperand,
    FinalIntegerOperand,
}

/// Source callees own their traceback positions independently of the enclosing fixed sites.
#[derive(Clone, Copy)]
enum CalleeLoader {
    Ordinary,
    Source,
}

impl CalleeLoader {
    fn fixed(self, frame: &mut CheckedFrame<'_, '_, '_>, expr: &Expr, next: &mut String) -> String {
        match self {
            Self::Ordinary => frame.kept_call_value(expr, next),
            Self::Source => frame.source_callee(expr, next),
        }
    }

    fn packed(self, frame: &mut CheckedFrame<'_, '_, '_>, expr: &Expr) -> String {
        match self {
            Self::Ordinary => frame.evaluate_handle(expr),
            Self::Source => frame.source_callee_handle(expr),
        }
    }
}

impl<'a> ValueUse<'a> {
    /// An enclosing operation can supply the boundary above its preceding operands.
    pub(super) fn operand_mark(self, frame: &mut CheckedFrame<'_, '_, '_>) -> Cow<'a, str> {
        match self {
            Self::RootedOperand(mark, _) => Cow::Borrowed(mark),
            _ => Cow::Owned(frame.temporary_mark()),
        }
    }

    /// Tell the enclosing call where the producer kept its completed operand.
    pub(super) fn retained_operand(self, next: String) {
        if let Self::RootedOperand(_, Some(retained)) = self {
            retained.set(next).expect("one result per call operand");
        }
    }

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

    /// On a failed word projection the producer already supplied the Python result.
    fn missing_word(&self, known_missing: bool) -> Option<Self> {
        match self {
            Self::IntOrHandle { fallback, .. } if known_missing => {
                Some(Self::handle(fallback.clone()))
            }
            _ => None,
        }
    }

    fn permanent_integer(body: &mut FunctionBody, raw: &str) -> Self {
        let zero = super::registered::permanent::small_integer_id(0).expect("zero is permanent");
        Self::handle(body.instruction(format!("add i64 {raw}, {zero}")))
    }

    fn frame_integer(
        value: Value,
        frame: &mut CheckedFrame<'_, '_, '_>,
        usage: ValueUse,
        permanent: bool,
    ) -> Self {
        if matches!(usage, ValueUse::PythonHandle | ValueUse::RootedOperand(..)) {
            if permanent {
                let body = frame.body();
                let raw = body.instruction(format!("ashr i64 {}, 1", value.operand));
                return Self::permanent_integer(body, &raw);
            }
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

/// Capture each written operand once from left to right. A Python method may
/// supply its already-bound receiver in the first ABI slot.
pub(super) fn capture_call_operands<T>(
    args: &[Expr],
    receiver: Option<T>,
    operand: impl FnMut(&Expr) -> T,
) -> Vec<T> {
    receiver
        .into_iter()
        .chain(args.iter().map(operand))
        .collect()
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
    fn when(&mut self, test: &str, yes: impl FnOnce(&mut Self)) {
        let take = self.block_name();
        let end = self.block_name();
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
        args: &CallArgs,
        returns: Type,
        usage: ValueUse<'_>,
    ) -> CheckedValue {
        let Some(site) = args.site.python() else {
            let native = self.fixed();
            let target = NativeCallTarget::Callable(native.operand(callable));
            return CheckedValue::owned(native.native_call(target, args, returns));
        };

        let emit = |frame: &mut CheckedFrame<'f, 'a, 'p>| {
            CheckedBody::registered(frame.reborrow()).python_call(
                &site.call,
                site.scope.as_deref(),
                callable,
                &args.values,
                usage,
                CalleeLoader::Ordinary,
            )
        };
        let frame = self.python();
        match site.call.position {
            Some((anchor, _)) => frame.at_expression(site.call.range, anchor, emit),
            None => emit(frame),
        }
    }

    /// Resolve the callable and any bound receiver before capturing the written arguments.
    fn python_call(
        &mut self,
        call: &ir::RegisteredCall,
        scope: Option<&Expr>,
        target: &Expr,
        args: &[Expr],
        usage: ValueUse<'_>,
        load_callable: CalleeLoader,
    ) -> CheckedValue {
        use super::registered::CallArguments;
        use ir::{RegisteredArguments, RegisteredCallTarget};

        let frame = self.python();
        let expression = matches!(
            call.target,
            RegisteredCallTarget::Callable {
                direct_eval: true,
                ..
            }
        )
        .then(|| args.first())
        .flatten()
        .and_then(|value| match &value.without_location().kind {
            ExprKind::Str(text) => Some(text.as_str()),
            _ => None,
        });
        let exec = frame.exec_literal_at(call.range, expression);
        let mark = usage.operand_mark(frame);
        let edge = frame.begin_operand_scope();
        if frame.native_suspension
            && let Some(name) = call.target.suspended_name()
        {
            frame.call_position(name);
        }
        let mut next = mark.to_string();
        let forced_pack = frame.output.link_interposition.scoped_call
            && !matches!(call.arguments, RegisteredArguments::Packed(_));
        let guarded_positional = forced_pack
            && matches!(call.arguments, RegisteredArguments::Positional)
            && call.partition_field.is_none();
        let packed = forced_pack || matches!(call.arguments, RegisteredArguments::Packed(_));
        let scope = scope.map(|scope| {
            if packed && !guarded_positional {
                frame.evaluate_handle(scope)
            } else {
                frame.kept_call_value(scope, &mut next)
            }
        });
        let target = if !matches!(call.target, RegisteredCallTarget::Method { .. })
            && (!packed || guarded_positional)
        {
            load_callable.fixed(frame, target, &mut next)
        } else {
            load_callable.packed(frame, target)
        };
        let (callable, receiver, range, end) = match &call.target {
            RegisteredCallTarget::Callable { .. } | RegisteredCallTarget::FixedExec { .. } => {
                (target, None, Some(call.range), call.target.end())
            }
            RegisteredCallTarget::Method {
                name,
                attribute,
                name_start,
            } => {
                let direct = (!packed || guarded_positional)
                    && matches!(call.arguments, RegisteredArguments::Positional);
                let (lookup, range) = frame
                    .positions
                    .method_ranges(*attribute, *name_start, name, call.range)
                    .unzip();
                let (callable, bound) =
                    frame.call_method_target(&target, name, lookup, attribute.end(), direct);
                next = frame.keep_operands(&next, &callable, bound.as_deref().unwrap_or("0"));
                if bound.is_some() && !args.is_empty() {
                    // keep_operands returns the boundary above the callable. A direct
                    // lookup may have placed its receiver above that boundary as well.
                    next = frame.temporary_mark();
                }
                (callable, bound, range, attribute.end())
            }
        };
        if guarded_positional {
            let receiver_slot = receiver.is_some();
            let prepared_receiver = receiver.as_deref().unwrap_or("0").to_owned();
            let (array, count) = frame.kept_call_arguments(&callable, receiver, args, next, exec);
            let method_name = match &call.target {
                RegisteredCallTarget::Method { name, .. } => frame.output.data(name.as_bytes()),
                _ => "null".into(),
            };
            // Arguments may replace the function's code, so inspect the live callable only
            // after its operands have run and before constructing any canonical argument list.
            let positional = frame.value(&format!(
                "call i1 @imre_dyn_positional_call_abi(ptr %ctx, i64 {callable}, i64 {prepared_receiver}, ptr {method_name})"
            ));
            let source = frame.name("b");
            let dynamic = frame.name("b");
            let done = frame.name("b");
            frame.branch(&positional, &source, &dynamic);
            frame.label(&source);
            let span = frame.position_for_call(range, end, call.range.end());
            let source_result = frame.call_at(
                &callable,
                CallArguments::Positional {
                    array,
                    count,
                    receiver_slot,
                },
                &mark,
                span,
                scope.as_deref(),
            );
            let source_block = frame.current_block().to_owned();
            frame.jump(&done);
            frame.label(&dynamic);
            let args = frame.pack_evaluated_positional(array, count, receiver_slot);
            frame.finish_unwind(edge, |frame| frame.release_owned_operands(&mark));
            let span = frame.position_for_call(range, end, call.range.end());
            let dynamic_result = frame.call_at(
                &callable,
                CallArguments::Packed {
                    args,
                    kwargs: "0".into(),
                },
                &mark,
                span,
                scope.as_deref(),
            );
            let dynamic_block = frame.current_block().to_owned();
            frame.jump(&done);
            frame.label(&done);
            return CheckedValue::handle(frame.value(&format!(
                "phi i64 [ {source_result}, %{source_block} ], [ {dynamic_result}, %{dynamic_block} ]"
            )));
        }
        let operands = match &call.arguments {
            layout if forced_pack => {
                let (positional, keywords) = match layout {
                    RegisteredArguments::Positional => (args.len(), Vec::new()),
                    RegisteredArguments::Keywords { positional, names } => (
                        *positional,
                        names
                            .iter()
                            .map(|name| Some(name.as_str().to_owned()))
                            .collect(),
                    ),
                    RegisteredArguments::Packed(_) => unreachable!("already packed"),
                };
                let pack = ir::PackedArguments {
                    starred: vec![false; positional],
                    keywords,
                };
                let (args, kwargs) =
                    frame.pack_arguments_with_exec(&pack, args, Some(&callable), exec);
                CallArguments::Packed { args, kwargs }
            }
            RegisteredArguments::Packed(pack) => {
                let (args, kwargs) = frame.pack_arguments(pack, args, Some(&callable));
                CallArguments::Packed { args, kwargs }
            }
            layout => {
                let receiver_slot = receiver.is_some();
                let (array, count) =
                    frame.kept_call_arguments(&callable, receiver, args, next, exec);
                if let RegisteredArguments::Keywords { positional, names } = layout {
                    CallArguments::FixedKeywords {
                        array,
                        positional: *positional,
                        keywords: names.len(),
                        names: frame.output.data(names.join("\0").as_bytes()),
                    }
                } else if let Some((index, range)) = call.partition_field {
                    let span = frame.expression_range_span(range, call.range.end(), b'[');
                    let get_position = frame.output.position(span);
                    debug_assert!(receiver_slot && count == 2);
                    CallArguments::PartitionField {
                        array,
                        index,
                        get_position,
                    }
                } else {
                    CallArguments::Positional {
                        array,
                        count,
                        receiver_slot,
                    }
                }
            }
        };
        let invoke = |frame: &mut CheckedFrame<'_, '_, '_>| {
            let span = frame.position_for_call(range, end, call.range.end());
            frame.call_at(&callable, operands, &mark, span, scope.as_deref())
        };
        let result = if packed {
            frame.finish_unwind(edge, |frame| frame.release_owned_operands(&mark));
            invoke(frame)
        } else {
            let result = invoke(frame);
            frame.finish_unwind(edge, |frame| frame.release_owned_operands(&mark));
            result
        };
        let result = if forced_pack && let Some((index, outer)) = call.partition_field {
            let span = frame.expression_range_span(outer, call.range.end(), b'[');
            let key = frame.integer(index);
            frame.op_at("get", &[&result, &key], span)
        } else {
            result
        };
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
        divisor: i64,
    ) -> Option<CheckedValue> {
        use super::registered::permanent;
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
                CheckedValue::permanent_integer(code.body(), &remainder)
            },
            |code| code.python_binary(Binary::Modulo, left, right, span),
        )
    }

    /// Emit the exact operation for native operands or a Python frame's guarded words.
    pub(super) fn integer_word(&mut self, op: Binary, left: &str, right: &str) -> Value {
        let value = self.body().integer_arithmetic(op, left, right, false);
        if let Entry::Native(emitter) = &mut self.0 {
            if !op.is_plain_arithmetic() {
                emitter.check_error();
            }
            return emitter.owned(value);
        }
        value
    }

    fn integer_arithmetic(
        &mut self,
        op: Binary,
        left: &CheckedValue,
        right: &CheckedValue,
        span: Option<[usize; 4]>,
        usage: ValueUse,
        permanent: bool,
    ) -> CheckedValue {
        let (a, check_a) = self.integer_operand(left);
        let (b, check_b) = self.integer_operand(right);
        if matches!(self.0, Entry::Native(_)) {
            return CheckedValue::owned(self.integer_word(op, &a, &b));
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
        let checks_right_value = !op.is_plain_arithmetic();
        let check_right = check_b || checks_right_value;
        let guard = match (check_a, check_right) {
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
            let value = code.integer_word(op, &a, &b);
            CheckedValue::frame_integer(value, code.python(), usage, permanent)
        };
        let Some(guard) = guard else {
            return exact(self);
        };
        ValueJoin::select(
            self,
            Self::body,
            |code, yes, no| code.body().branch(&guard, yes, no),
            exact,
            |code| {
                // A sole projection guard identifies the exact missing word. A zero
                // divisor or an invalid shift can fail with both words still present.
                let lhs = left.missing_word(check_a && !check_right);
                let rhs = right.missing_word(check_b && !check_a && !checks_right_value);
                code.python_binary(
                    op,
                    lhs.as_ref().unwrap_or(left),
                    rhs.as_ref().unwrap_or(right),
                    span,
                )
            },
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
                    UnaryOp::UAdd => return CheckedValue::owned(emitter.own_operand(input)),
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
        let types = (left.checked_operand_type(), right.checked_operand_type());
        let checked_ints = types == (Type::Int, Type::Int);
        let plain_arithmetic = op.is_plain_arithmetic();
        let float_arithmetic = plain_arithmetic
            && matches!(self.0, Entry::Registered(_))
            && types == (Type::Float, Type::Float);
        // Refining a Python operand for arithmetic must preserve its original error site.
        let positioned = plain_arithmetic
            || op == Binary::Modulo
                && [left, right].into_iter().all(|operand| {
                    operand.ty == Type::Int
                        || matches!(operand.without_location().kind, ExprKind::Int(_))
                });
        let integer_arithmetic = (matches!(
            op,
            Binary::FloorDivide | Binary::Modulo | Binary::RightShift
        ) && checked_ints
            || op == Binary::FloorDivide
                && usage.needs_integer()
                && !matches!(site, OperationSite::Unlocated)
            || positioned
                && (matches!(site, OperationSite::Placed(..))
                    || expr.ty == Type::PythonObject
                    || checked_ints))
            && !float_arithmetic
            && [left, right].into_iter().all(|operand| {
                matches!(
                    operand.without_location().kind.scalar_literal_type(),
                    None | Some(Type::Int)
                )
            });
        let (left_use, right_use) = if integer_arithmetic
            && (op == Binary::Multiply || checked_ints || usage.needs_integer())
        {
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
        let divisor =
            if op == Binary::Modulo && checked_ints && matches!(self.0, Entry::Registered(_)) {
                right.without_location().tagged_integer_literal()
            } else {
                None
            };
        let result = if let Entry::Native(emitter) = &mut self.0
            && let Some(value) =
                emitter.bounded_binary(op, expr, (left, a.fixed_value()), (right, b.fixed_value()))
        {
            CheckedValue::owned(value)
        } else if float_arithmetic {
            self.python().float_arithmetic(op, &a, &b, span)
        } else if let Some(divisor) = divisor
            && let Some(value) = self.cached_remainder(&a, &b, span, divisor)
        {
            value
        } else if integer_arithmetic
            && (op == Binary::Multiply
                || checked_ints
                || a.can_use_integer_arithmetic(left.ty, &b, right.ty, usage))
        {
            // Python's exact remainder is in 0..divisor, including for a heap dividend.
            let permanent = divisor.is_some_and(|d| {
                (1..=super::registered::permanent::SMALL_INT_MAX + 1).contains(&d)
            });
            self.integer_arithmetic(op, &a, &b, span, usage, permanent)
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
            && let Some(range) = frame.next_fixed_site(ir::Node::Expression(expr))
        {
            let attribute = frame.fixed_attribute(range);
            use crate::frontend::FixedAttribute;
            let call = attribute.and_then(FixedAttribute::call_binding);
            let position = match attribute {
                Some(FixedAttribute::Load(name)) => name.range.start(),
                _ => call
                    .and_then(|(_, site)| site.method_name_start())
                    .unwrap_or(range.start()),
            };
            return frame.at_expression(range, position, |frame| {
                if let Some((attr, (target, site))) = attribute.zip(call) {
                    let (callee, args) = attr
                        .call_operands(range, frame.operand_range(expr), expr)
                        .expect("placed call operands");
                    let load = if target.is_some() {
                        CalleeLoader::Source
                    } else {
                        CalleeLoader::Ordinary
                    };
                    return CheckedBody::registered(frame.reborrow())
                        .python_call(site, None, callee, args, usage, load);
                }
                if let Some(FixedAttribute::Load(name)) = attribute {
                    let operands = expr
                        .attribute_operands()
                        .expect("placed attribute operands");
                    return CheckedBody::registered(frame.reborrow())
                        .load_attribute(operands, name, range, usage);
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

    /// A fixed module slot and its placed Python spelling describe the same source read.
    /// Only the native representation can retain the value directly from that slot.
    fn read_name(&mut self, binding: &Builtin, ty: Type, site: OperationSite) -> CheckedValue {
        let name = match binding {
            Builtin::GlobalGet(slot) => match &mut self.0 {
                Entry::Native(emitter) => {
                    let value = emitter.borrow_global(*slot, ty);
                    emitter.retain(&value);
                    return CheckedValue::owned(value);
                }
                Entry::Registered(frame) => {
                    let OperationSite::Placed(range) = site else {
                        unreachable!("registered module slot requires a lexical binding")
                    };
                    let Some(crate::frontend::FixedAttribute::ModuleName(name)) =
                        frame.fixed_attribute(range)
                    else {
                        unreachable!("placed module slot keeps its spelling")
                    };
                    name.as_str()
                }
            },
            Builtin::RegisteredName(name) => name,
            _ => unreachable!("checked name read"),
        };
        CheckedValue::handle(self.python().lookup(name))
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
                    // The located operand is emitted directly, with its enclosing source position.
                    frame.next_fixed_site(ir::Node::Expression(&operands[0]));
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
            ExprKind::Builtin(
                binding @ (Builtin::GlobalGet(_) | Builtin::RegisteredName(_)),
                args,
            ) if args.is_empty() => self.read_name(binding, expr.ty, site),
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
            ExprKind::List(items)
            | ExprKind::Tuple(items)
            | ExprKind::Set { values: items, .. } => {
                let display = match &expr.kind {
                    ExprKind::Tuple(_) => DisplayKind::Tuple,
                    ExprKind::Set { .. } => DisplayKind::Set,
                    _ => DisplayKind::List,
                };
                match &mut self.0 {
                    Entry::Native(emitter) if display == DisplayKind::Tuple => {
                        CheckedValue::owned(emitter.tuple(expr.ty, items, None))
                    }
                    Entry::Native(emitter) => {
                        CheckedValue::owned(emitter.native_sequence(expr.ty, items, false))
                    }
                    Entry::Registered(frame) => {
                        CheckedValue::handle(frame.sequence(items, display, usage))
                    }
                }
            }
            ExprKind::Compare(values, ops) => {
                let truth =
                    matches!(self.0, Entry::Native(_)) || usage == ValueUse::ComparisonTruth;
                self.comparisons(values, ops, truth)
            }
            ExprKind::Conditional(test, yes, no) => self.conditional_value(test, yes, no),
            ExprKind::Boolean(..) | ExprKind::BooleanValue(..) => self.boolean_expression(expr),
            ExprKind::Binary(op, left, right) => {
                self.binary_expression(*op, left, right, expr, site, usage)
            }
            ExprKind::Invoke { callable, args } => self.invoke(callable, args, expr.ty, usage),
            ExprKind::Unary(op, value) => self.unary_expression(*op, value, expr),
            ExprKind::Scope { body, value, clear } => self.scoped_expression(body, value, clear),
            ExprKind::Dict(entries) => self.dictionary(expr.ty, entries),
            ExprKind::Builtin(op, args) => self.builtin(op, args, expr.ty, usage),
            // Python handles already represent both the payload and None. Evaluating
            // the original operand also preserves effects of a None-valued call.
            ExprKind::Present(value) | ExprKind::Absent(value) | ExprKind::Unwrap(value)
                if matches!(self.0, Entry::Registered(_)) =>
            {
                self.operand_for(value, usage)
            }
            // A narrowed float already carries the box required by the result.
            ExprKind::Present(value)
                if value.ty == Type::Float
                    && let ExprKind::Unwrap(owner) = &value.kind
                    && owner.ty == expr.ty =>
            {
                self.evaluate_for(owner, usage)
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
            ExprKind::Call(id, args) => CheckedValue::owned(self.fixed().native_call(
                NativeCallTarget::Function(*id),
                args,
                expr.ty,
            )),
            ExprKind::Callable { function, receiver } => CheckedValue::owned(
                self.fixed()
                    .callable_value(expr.ty, *function, receiver.as_deref()),
            ),
            ExprKind::Construct { initializer, args } => {
                CheckedValue::owned(self.fixed().construct_record(expr.ty, *initializer, args))
            }
            ExprKind::Exception { kind, message } => {
                if matches!(self.0, Entry::Native(_)) {
                    CheckedValue::owned(self.fixed().exception_value(*kind, message.as_deref()))
                } else {
                    let message = message.as_deref().map(|message| self.operand(message));
                    let frame = self.python();
                    let name = frame.text(
                        imre_ast::exceptions::Kind::try_from_raw(*kind)
                            .unwrap()
                            .name(),
                    );
                    let class = frame.op("builtin", &[&name]);
                    let value = if let Some(message) = message {
                        let message = frame.materialize(&message);
                        frame.invoke(&class, &[&message])
                    } else {
                        frame.invoke(&class, &[])
                    };
                    CheckedValue::handle(value)
                }
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

    fn load_attribute(
        &mut self,
        operands: &[Expr],
        name: &ruff_python_ast::Identifier,
        range: TextRange,
        usage: ValueUse,
    ) -> CheckedValue {
        let [receiver] = operands else {
            unreachable!("checked attribute receiver")
        };
        let mark = usage.operand_mark(self.python());
        let unwind = self.python().begin_operand_scope();
        let receiver = self.rooted_operand(receiver);
        let frame = self.python();
        let value = frame.attribute_value(receiver.rooted_handle(), name, range);
        usage.retained_operand(frame.keep_operands(&mark, &value, "0"));
        frame.finish_operand_scope(&mark, unwind);
        CheckedValue::handle(value)
    }

    /// Placed operations and registered expressions evaluate the same checked operands.
    fn builtin(
        &mut self,
        op: &Builtin,
        args: &[Expr],
        returns: Type,
        usage: ValueUse,
    ) -> CheckedValue {
        match op {
            Builtin::LiteralStringView if matches!(self.0, Entry::Registered(_)) => {
                self.operand_for(&args[0], usage)
            }
            Builtin::RegisteredAttribute {
                action: ir::AttributeAction::Load,
                name,
                range,
                ..
            } => self
                .python()
                .at_expression(*range, name.range.start(), |frame| {
                    CheckedBody::registered(frame.reborrow())
                        .load_attribute(args, name, *range, usage)
                }),
            Builtin::ListGet(_)
            | Builtin::TupleGet
            | Builtin::TupleIndex
            | Builtin::StringGet
            | Builtin::BytesGet
            | Builtin::DictGet
            | Builtin::RegisteredSubscript(..) => self.subscript(op, args, returns),
            Builtin::DeleteItem => {
                let [receiver, key] = args else {
                    unreachable!("checked deletion operands")
                };
                self.delete_item(receiver, key)
            }
            Builtin::Append | Builtin::SetAdd => self.insert(op, args),
            Builtin::FormatJoin => match &mut self.0 {
                Entry::Native(emitter) => {
                    CheckedValue::owned(emitter.concat(args.iter().collect()))
                }
                Entry::Registered(frame) => {
                    let mark = frame.temporary_mark();
                    CheckedValue::handle(frame.unwind_stack_operands(&mark, |frame| {
                        let (values, count) = frame.evaluate_handle_array(args);
                        frame.checked(&format!(
                            "call i64 @imre_dyn_format_parts(ptr %ctx, i64 {mark}, ptr {values}, i64 {count})"
                        ))
                    }))
                }
            },
            _ => match &mut self.0 {
                Entry::Native(emitter) => CheckedValue::owned(emitter.builtin(op, args, returns)),
                Entry::Registered(frame) => frame.builtin(op, args),
            },
        }
    }

    pub(crate) fn emit_statement(&mut self, stmt: &Statement) {
        self.statement_with_tail(stmt, &mut [].as_slice());
    }

    fn statement_with_tail(&mut self, stmt: &Statement, tail: &mut &[Statement]) {
        if let Entry::Registered(frame) = &mut self.0 {
            match (stmt, frame.next_fixed_site(ir::Node::Statement(stmt))) {
                (Statement::Position(range, simple), _) => frame.statement_site(*range, *simple),
                (Statement::Break(Some(range)) | Statement::Continue(range), _) => {
                    frame.statement_site(*range, None)
                }
                (
                    Statement::SetField(receiver, ir::StoreField::Written(field, name), value),
                    Some(site),
                ) => {
                    frame.statement_site(site, None);
                    frame.mutate_attribute(
                        ir::AttributeAction::Store,
                        name,
                        field.target,
                        [value, receiver].into_iter(),
                    );
                    return;
                }
                (
                    Statement::AugmentField(
                        receiver,
                        ir::StoreField::Written(field, name),
                        op,
                        rhs,
                    ),
                    Some(site),
                ) => {
                    frame.statement_site(site, None);
                    frame.mutate_attribute(
                        ir::AttributeAction::InPlace(op.protocol()),
                        name,
                        field.target,
                        [receiver, rhs].into_iter(),
                    );
                    return;
                }
                (_, Some(range)) => {
                    let simple = match frame.fixed_attribute(range) {
                        Some(crate::frontend::FixedAttribute::UnhighlightedCall(call)) => {
                            Some(*call)
                        }
                        _ => None,
                    };
                    frame.statement_site(range, simple);
                }
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
            Statement::ContainerFor(input) => self.container_suite(stmt, input),
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
            } => self.pull(*iterator, *key, *value, *ready, *kind, tail),
            Statement::Break(_) | Statement::Continue(_) | Statement::Exhaust => {
                self.loop_transfer(stmt)
            }
            Statement::Move(target, source) => self.fixed().move_local(*target, *source),
            Statement::SetField(receiver, field, rhs) => {
                let code = self.fixed();
                let value = code.operand(rhs);
                let receiver = code.operand(receiver);
                code.set_field(&receiver.value, field.slot(), &value.value);
                code.release_operand(&receiver);
                code.release_operand(&value);
            }
            Statement::AugmentField(receiver, field, op, rhs) => {
                let code = self.fixed();
                let receiver = code.operand(receiver);
                let old = code.field_update_operand(&receiver.value, field.slot(), rhs);
                let right = code.operand(rhs);
                let value = code.binary(*op, &old.value, &right.value);
                code.forget(&value);
                code.replace_field(&receiver.value, field.slot(), &value);
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
            Entry::Registered(frame) => {
                statements = frame.explicit_prefix(statements);
                None
            }
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
        while let Some((stmt, mut tail)) = remaining.split_first() {
            let fused = match &mut self.0 {
                Entry::Native(emitter) if !emitter.body.terminated => {
                    emitter.fused_statements(remaining)
                }
                Entry::Registered(frame) if !frame.terminated() => 0,
                _ => break,
            };
            if fused != 0 {
                remaining = &remaining[fused..];
            } else {
                self.statement_with_tail(stmt, &mut tail);
                remaining = tail;
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
