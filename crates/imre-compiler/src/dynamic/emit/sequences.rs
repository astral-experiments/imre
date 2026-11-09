//! Container displays in registered Python frames.
use super::{Constant, Function, permanent};
use crate::ir::DisplayKind;
use crate::ir::{Builtin, Expr, ExprKind};
use std::fmt::Write as _;

fn literal(value: &Expr) -> &ExprKind {
    match &value.kind {
        ExprKind::Builtin(Builtin::RegisteredLocation(..), args) => literal(&args[0]),
        kind => kind,
    }
}

fn spread(item: &Expr) -> Option<&Expr> {
    match &item.kind {
        ExprKind::Builtin(Builtin::RegisteredSpread, operands) => Some(&operands[0]),
        _ => None,
    }
}

fn permanent_handle(value: &Expr) -> Option<u16> {
    let handle = match literal(value) {
        ExprKind::None => permanent::NONE,
        ExprKind::Bool(true) => permanent::TRUE,
        ExprKind::Bool(false) => permanent::FALSE,
        ExprKind::Int(n) => permanent::small_integer_id(n.parse().ok()?)?,
        ExprKind::Builtin(Builtin::RegisteredEllipsis, _) => permanent::ELLIPSIS,
        ExprKind::Tuple(values) if values.is_empty() => permanent::EMPTY_TUPLE,
        _ => return None,
    };
    u16::try_from(handle).ok()
}

impl Function<'_, '_> {
    /// Equal literals share a slot within a source unit. The address remains distinct after linking
    /// independently compiled providers, and the runtime holds its value in the current heap.
    pub(super) fn constant_slot(&mut self, key: Constant) -> String {
        let output = &mut self.frame.output;
        let next = output.constant_slots.len();
        let index = *output
            .constant_slots
            .entry((self.source.unit, key))
            .or_insert(next);
        if index == next {
            writeln!(
                output.constants,
                "@const_slot{index} = private constant i8 0"
            )
            .unwrap();
        }
        format!("@const_slot{index}")
    }

    pub(super) fn build_sequence(
        &mut self,
        values: &[String],
        tuple: bool,
        slot: Option<&str>,
    ) -> String {
        let (array, count) = self.handle_array(values);
        let slot = slot.unwrap_or("null");
        self.checked(&format!("call i64 @imre_dyn_build_sequence(ptr %ctx, ptr {array}, i64 {count}, i1 false, i1 {tuple}, ptr {slot})"))
    }

    pub(super) fn checked_sequence(
        &mut self,
        values: &[Expr],
        display: DisplayKind,
        evaluate: impl FnMut(&mut Self, &Expr) -> String,
    ) -> String {
        let tuple = display == DisplayKind::Tuple;
        if display == DisplayKind::Set {
            return self.sequence_display(values, display, None, spread, evaluate);
        }
        if values.is_empty() {
            return if tuple {
                permanent::EMPTY_TUPLE.to_string()
            } else {
                self.op("list", &[])
            };
        }
        let slot = tuple
            .then(|| {
                values
                    .iter()
                    .map(Constant::from_expr)
                    .collect::<Option<Vec<_>>>()
            })
            .flatten()
            .map(|keys| self.constant_slot(Constant::Tuple(keys)));
        if let Some(handles) = values
            .iter()
            .map(permanent_handle)
            .collect::<Option<Vec<_>>>()
        {
            let data = self.output.data(
                &handles
                    .iter()
                    .flat_map(|id| id.to_le_bytes())
                    .collect::<Vec<_>>(),
            );
            let slot = slot.as_deref().unwrap_or("null");
            let count = handles.len();
            return self.checked(&format!("call i64 @imre_dyn_build_sequence(ptr %ctx, ptr {data}, i64 {count}, i1 true, i1 {tuple}, ptr {slot})"));
        }
        let build = |code: &mut Self| {
            code.sequence_display(values, display, slot.as_deref(), spread, evaluate)
        };
        let Some(slot) = slot.as_deref() else {
            return build(self);
        };
        self.cached_tuple(slot, build)
    }

    pub(super) fn cached_tuple(
        &mut self,
        slot: &str,
        build: impl FnOnce(&mut Self) -> String,
    ) -> String {
        // A loaded tuple is a constant of the code unit. Reuse it before evaluating
        // any literal loaders or allocating a temporary container.
        let cached = self.value(&format!(
            "call i64 @imre_dyn_cached_handle(ptr %ctx, ptr {slot})"
        ));
        let hit = self.value(&format!("icmp ne i64 {cached}, 0"));
        let previous = self.current_block().to_owned();
        let load = self.name("b");
        let done = self.name("b");
        self.branch(&hit, &done, &load);
        self.label(&load);
        let result = build(self);
        let loaded = self.current_block().to_owned();
        self.jump(&done);
        self.label(&done);
        self.value(&format!(
            "phi i64 [ {cached}, %{previous} ], [ {result}, %{loaded} ]"
        ))
    }

    /// Evaluate small prefixes before allocating. Once built, the list owns every consumed operand.
    pub(super) fn sequence_display<T>(
        &mut self,
        items: &[T],
        display: DisplayKind,
        slot: Option<&str>,
        spread: impl Fn(&T) -> Option<&T>,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        let tuple = display == DisplayKind::Tuple;
        let mark = self.temporary_mark();
        let unwind = |code: &mut Self| {
            code.line(&format!(
                "call void @imre_dyn_unwind_display(ptr %ctx, i64 {mark})"
            ));
        };
        self.unwind_with(unwind, |code| {
            let initial = |code: &mut Self, values: Vec<String>, complete| {
                let slot = if complete { slot } else { None };
                let container = if display == DisplayKind::Set {
                    let (data, count) = code.handle_array(&values);
                    code.checked(&format!("call i64 @imre_dyn_build_set(ptr %ctx, ptr {data}, i64 {count}, i64 {mark})"))
                } else {
                    code.build_sequence(&values, tuple && complete, slot)
                };
                if !complete {
                    code.keep_operands(&mark, &container, "0");
                }
                container
            };
            let append = |code: &mut Self, list: &String, _, value: String, extend| {
                if extend && code.native_suspension {
                    code.frame.call_position(code.current_range);
                }
                code.take_matching_check();
                code.checked(&format!("call i64 @imre_dyn_display_item(ptr %ctx, i64 {list}, i64 {value}, i64 {mark}, i1 {extend})"));
            };
            let (mut result, complete) = crate::llvm::operations::sequences::build(
                code, items, spread, &mut evaluate, initial, append,
            );
            if !complete && tuple {
                let operation = if slot.is_some() { "constant_tuple" } else { "tuple" };
                result = code.op(operation, &[&result]);
                if let Some(slot) = slot {
                    result = code.checked(&format!("call i64 @imre_dyn_linked_value(ptr %ctx, ptr {slot}, i64 {result})"));
                }
            }
            code.keep_operands(&mark, &result, "0");
            result
        })
    }
}
