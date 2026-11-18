//! A deliberately closed source region in which one private result can be replaced.

use super::{RegisteredFunction, Statement};
use crate::ir::{self, Builtin, Expr, ExprKind};
use ruff_text_size::TextRange;

pub(super) const ABI: &str = concat!(
    "declare i32 @imre_dyn_replacement_pull_item(ptr, i64, ptr, i1, i64, ptr)\n",
    "declare i64 @imre_dyn_replacement_lookup(ptr, i64, ptr, i64, i1, ptr)\n",
    "declare i64 @imre_dyn_replacement_assign_target(ptr, i64, ptr, i64, i64, ptr, ptr)\n",
    "declare void @imre_dyn_replacement_guard_item_store(ptr, i64, ptr, i64, ptr)\n",
    "declare void @imre_dyn_replacement_guard_result_store(ptr, i64, ptr, i64, i64, ptr, ptr)\n",
    "declare void @imre_dyn_replacement_assignment_target_at(ptr, i64, ptr, i64, i64, ptr, ptr)\n",
    "declare void @imre_dyn_replacement_commit(ptr, i64, ptr, i64, i64, ptr, i64, ptr, ptr)\n",
    "declare i64 @imre_dyn_replacement_positional_call_at(ptr, i64, ptr, i64, i64, ptr, i64, ptr, ptr)\n",
    "declare i64 @imre_dyn_replacement_initial_call_at(ptr, i64, ptr, i64, i64, ptr, i64, ptr)\n",
    "declare void @imre_dyn_replacement_statement(ptr, i64, ptr, i64, ptr, i32, i64, ptr)\n",
);

/// The only statement whose RHS may establish a fresh-result witness.
pub(super) struct Replacement {
    pub(super) result: usize,
    pub(super) item: usize,
    pub(super) cursor: usize,
    pub(super) position: TextRange,
    pub(super) initial_position: TextRange,
}

fn direct_call(value: &Expr) -> Option<(&str, usize)> {
    let ExprKind::Invoke { callable, args } = &value.without_location().kind else {
        return None;
    };
    let site = args.site.python()?;
    if site.scope.is_some()
        || args.exhaustive_enum_match
        || site.call.checked_nullable_receiver
        || site.call.partition_field.is_some()
        || !matches!(site.call.arguments, ir::RegisteredArguments::Positional)
        || !matches!(
            site.call.target,
            ir::RegisteredCallTarget::Callable {
                direct_eval: false,
                ..
            }
        )
        || !args.order.is_empty()
    {
        return None;
    }
    let ExprKind::Builtin(Builtin::RegisteredName(name), operands) =
        &callable.without_location().kind
    else {
        return None;
    };
    let [argument] = args.values.as_slice() else {
        return None;
    };
    operands
        .is_empty()
        .then_some((name.as_str(), local(argument)?))
}

/// Source locations only change diagnostic position; no reads or conversions are admitted.
fn local(value: &Expr) -> Option<usize> {
    match value.without_location().kind {
        ExprKind::Local(slot) => Some(slot),
        _ => None,
    }
}

/// Admit only a complete, unsuspended checked function with a single private
/// result binding and one read of that binding, in its terminal return.
pub(super) fn replacement(
    checked: &RegisteredFunction,
    is_fast_local: impl Fn(&str) -> bool,
) -> Option<Replacement> {
    let [
        Statement::Position(initial_position, _),
        Statement::Assign(result, initial),
        Statement::Position(..),
        Statement::Scope { body, clear },
        Statement::Position(..),
        Statement::Return(returned),
    ] = checked.function.body.as_slice()
    else {
        return None;
    };
    if checked.function.params.contains(result) || local(returned) != Some(*result) {
        return None;
    }
    let binding = checked.local_names.get(result)?;
    if binding.namespace.is_some() || !is_fast_local(&binding.name) {
        return None;
    }
    let [
        Statement::Assign(cursor, acquire),
        Statement::While(test, loop_body, otherwise),
    ] = body.as_slice()
    else {
        return None;
    };
    if !matches!(test.kind, ExprKind::Bool(true)) || !otherwise.is_empty() {
        return None;
    }
    let [
        Statement::IterNext {
            kind: ir::IterNextKind::Generic,
            iterator,
            key,
            value: None,
            ready,
        },
        exhausted,
        Statement::Assign(item, item_value),
        Statement::Clear(cleared),
        Statement::Position(position, _),
        Statement::Assign(replaced, rhs),
    ] = loop_body.as_slice()
    else {
        return None;
    };
    if iterator != cursor
        || replaced != result
        || cleared != key
        || clear.as_slice() != [*cursor, *key, *ready]
        || local(item_value) != Some(*key)
        || !matches!(
            exhausted.branches_for_flag(*ready),
            Some(([], [Statement::Exhaust]))
        )
    {
        return None;
    }
    let item_binding = checked.local_names.get(item)?;
    if item_binding.namespace.is_some() || !is_fast_local(&item_binding.name) {
        return None;
    }
    let (name, argument) = direct_call(rhs)?;
    if name != "loads"
        || direct_call(initial)? != (name, argument)
        || [*item, argument, *cursor, *key, *ready].contains(result)
    {
        return None;
    }
    let ExprKind::Builtin(Builtin::RegisteredIter(_), operands) = &acquire.without_location().kind
    else {
        return None;
    };
    let [range] = operands.as_slice() else {
        return None;
    };
    let (range_name, count) = direct_call(range)?;
    if range_name != "range" || count == *result || count == *item {
        return None;
    }
    Some(Replacement {
        result: *result,
        item: *item,
        cursor: *cursor,
        position: *position,
        initial_position: *initial_position,
    })
}
