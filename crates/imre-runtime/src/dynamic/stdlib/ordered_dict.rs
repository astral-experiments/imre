//! Storage operations for the Python OrderedDict subclass.

use super::super::value::{DictViewKind, NOT_IMPLEMENTED};
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

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
        ("sizeof", []) => {
            let bytes = ctx.dictionary_size(owner, storage, true)?;
            Ok(ctx.alloc(Value::Int(crate::Int::from_usize(bytes))))
        }
        ("setitem", [key, value]) => {
            ctx.set_ordered_dictionary_key(storage, *key, *value)?;
            Ok(NONE)
        }
        ("delitem", [key]) => {
            let removed = remove_key(ctx, storage, *key, None)?
                .ok_or_else(|| ctx.lookup_error("KeyError", *key))?;
            ctx.finalize_removed_dict_value(removed);
            Ok(NONE)
        }
        ("clear", []) => {
            ctx.clear_ordered_dict(storage);
            Ok(NONE)
        }
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
        ("move_to_end", [key, last]) => {
            let last = ctx.truth(*last)?;
            ctx.move_dictionary_key(storage, *key, last)?;
            Ok(NONE)
        }
        ("setdefault", [key, default]) => {
            let (_, found) = ctx.find_dictionary_key(storage, *key, None)?;
            if let Some(index) = found {
                return Ok(ctx.stored_entries(storage)[index].1);
            }
            ctx.set_ordered_dictionary_key(storage, *key, *default)?;
            Ok(*default)
        }
        ("pop", [key] | [key, _]) => pop_key(ctx, storage, *key, None, args.get(1).copied()),
        ("popitem", [last]) => {
            let last = ctx.truth(*last)?;
            let entries = ctx.stored_entries(storage);
            let Some((_, key, hash)) = entries.order_entry(entries.order_start(last), last) else {
                let message = ctx.text("dictionary is empty");
                return Err(ctx.lookup_error("KeyError", message));
            };
            let value = pop_key(ctx, storage, key, Some(hash), None)?;
            Ok(ctx.alloc(Value::Tuple(vec![key, value])))
        }
        ("repr", []) => {
            if ctx.repr_ancestors.contains(&owner) {
                return Ok(ctx.text("..."));
            }
            let items = if ctx.stored_entries(storage).is_empty() {
                String::new()
            } else {
                ctx.repr_ancestors
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                ctx.repr_ancestors.push(owner);
                let copy = super::super::builtins::copy_dict(ctx, owner, storage);
                ctx.repr_ancestors.pop();
                ctx.checked_container_slot_repr(owner, copy?)?
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

fn pop_key(
    ctx: &mut Context,
    storage: Id,
    key: Id,
    cached: Option<i64>,
    default: Option<Id>,
) -> Result<Id> {
    let (hash, found) = ctx.find_dictionary_key(storage, key, cached)?;
    let entries = ctx.stored_entries(storage);
    if found.and_then(|i| entries.order_node(i)).is_none() {
        return default.ok_or_else(|| ctx.lookup_error("KeyError", key));
    }
    remove_key(ctx, storage, key, Some(hash))?
        .or(default)
        .ok_or_else(|| ctx.lookup_error("KeyError", key))
}

/// Resolve the node before deleting the backing entry: each lookup can call Python.
fn remove_key(ctx: &mut Context, storage: Id, key: Id, cached: Option<i64>) -> Result<Option<Id>> {
    let hash = if ctx.stored_entries(storage).order_len() == 0 {
        cached
    } else {
        let (hash, dense) = ctx.find_dictionary_key(storage, key, cached)?;
        if let Some(dense) = dense
            && let Some(node) = ctx.stored_entries(storage).order_node(dense)
        {
            ctx.unlink_ordered_node(storage, dense, node);
        }
        Some(hash)
    };
    super::super::builtins::remove_dict_item_hashed(ctx, storage, key, hash)
}

/// Stop if comparing two keys changes the order of either dictionary.
fn keys_equal(ctx: &mut Context, left: Id, right: Id) -> Result<bool> {
    let state = |ctx: &Context, id| {
        let entries = ctx.stored_entries(id);
        (entries.order_len(), entries.order_state())
    };
    let original = (state(ctx, left), state(ctx, right));
    let ((left_len, _), (right_len, _)) = original;
    // Mapping equality may have changed a length. Compare the common prefix before testing the ends.
    let mut left_cursor = ctx.stored_entries(left).order_start(false);
    let mut right_cursor = ctx.stored_entries(right).order_start(false);
    for _ in 0..left_len.min(right_len) {
        let (next_left, a, _) = ctx
            .stored_entries(left)
            .order_entry(left_cursor, false)
            .expect("live ordered key");
        let (next_right, b, _) = ctx
            .stored_entries(right)
            .order_entry(right_cursor, false)
            .expect("live ordered key");
        left_cursor = next_left;
        right_cursor = next_right;
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
    Ok(left_len == right_len)
}
