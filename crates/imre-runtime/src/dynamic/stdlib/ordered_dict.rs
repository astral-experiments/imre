//! Storage operations for the Python OrderedDict subclass.

use super::super::value::{DictViewKind, NOT_IMPLEMENTED};
use super::{Context, Error, FALSE, Id, Result, TRUE, Value};

/// Use the existing dictionary entries and recursion guard for OrderedDict operations.
pub(super) fn call(
    ctx: &mut Context,
    op: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&owner, args)) = args.split_first() else {
        return Err(Error::new("TypeError", "expected a dictionary"));
    };
    let storage = ctx
        .dict_storage(owner)
        .filter(|_| kwargs.is_empty())
        .ok_or_else(|| Error::new("TypeError", "expected a dictionary"))?;
    match (op, args) {
        ("view", [kind]) => {
            let kind = match ctx.str_value(*kind)? {
                "keys" => DictViewKind::Keys,
                "items" => DictViewKind::Items,
                "values" => DictViewKind::Values,
                _ => return Err(Error::new("ValueError", "unknown dictionary view")),
            };
            Ok(ctx.alloc(Value::DictView {
                dict: owner,
                kind,
                ordered: true,
            }))
        }
        ("repr", []) => {
            if ctx.repr_ancestors.contains(&owner) {
                return Ok(ctx.text("..."));
            }
            let items = if ctx.stored_entries(storage).is_empty() {
                String::new()
            } else {
                ctx.checked_container_slot_repr(owner, storage)?
            };
            let class = ctx.exact_type(owner)?;
            Ok(ctx.text(format!("{}({items})", ctx.class_name(class))))
        }
        ("equal", [other, ordered]) => {
            let Some(right) = ctx.dict_storage(*other) else {
                return Ok(NOT_IMPLEMENTED);
            };
            let equal = ctx.checked_equal(storage, right)?
                && (*ordered != TRUE || keys_equal(ctx, storage, right)?);
            Ok(if equal { TRUE } else { FALSE })
        }
        _ => Err(Error::new(
            "TypeError",
            "invalid ordered dictionary operation",
        )),
    }
}

/// Stop if comparing two keys changes the order of either dictionary.
fn keys_equal(ctx: &mut Context, left: Id, right: Id) -> Result<bool> {
    let state = |ctx: &Context, id| {
        let entries = ctx.stored_entries(id);
        (entries.len(), entries.order_state())
    };
    let original = (state(ctx, left), state(ctx, right));
    let len = ctx.stored_entries(left).len();
    if len != ctx.stored_entries(right).len() {
        return Ok(false);
    }
    for index in 0..len {
        let a = ctx.stored_entries(left)[index].0;
        let b = ctx.stored_entries(right)[index].0;
        let equal = a == b || ctx.checked_equal(a, b)?;
        if (state(ctx, left), state(ctx, right)) != original {
            return Err(Error::new(
                "RuntimeError",
                "OrderedDict mutated during iteration",
            ));
        }
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}
