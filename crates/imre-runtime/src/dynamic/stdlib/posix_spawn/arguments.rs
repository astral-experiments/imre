use std::ffi::CString;

use crate::dynamic::stdlib::os;
use crate::dynamic::{Context, Error, Id, NONE, Result, Value, builtins};

/// Check the argument shapes before invoking path conversions, as py_posix_spawn does.
pub(super) fn arguments(
    ctx: &mut Context,
    name: &str,
    argv: Id,
    env: Id,
    scheduler: Option<Id>,
) -> Result<(Vec<CString>, Option<Vec<CString>>)> {
    if !ctx.builtin_slot_accepts(argv, "list") && !ctx.builtin_slot_accepts(argv, "tuple") {
        return Err(Error::new(
            "TypeError",
            format!("{name}: argv must be a tuple or list"),
        ));
    }
    let mark = ctx.heap.temporary_mark();
    let argc = match ctx.instance_len(argv) {
        Ok(Some(length)) => length,
        Ok(None) => ctx.sequence_at(argv, 0).map_or(0, |(length, _)| length),
        Err(_) => 0,
    };
    ctx.discard_truth(mark);
    if argc == 0 {
        return Err(Error::new(
            "ValueError",
            format!("{name}: argv must not be empty"),
        ));
    }
    if env != NONE && !ctx.has_mapping_subscription(env) {
        return Err(Error::new(
            "TypeError",
            format!("{name}: environment must be a mapping object or None"),
        ));
    }
    if scheduler.is_some_and(|value| value != NONE && !ctx.builtin_slot_accepts(value, "tuple")) {
        return Err(Error::new(
            "TypeError",
            format!("{name}: scheduler must be a tuple or None"),
        ));
    }
    let argv = os::fs_arguments(ctx, argv, argc)?;
    if argv[0].as_bytes().is_empty() {
        return Err(Error::new(
            "ValueError",
            format!("{name}: argv first element cannot be empty"),
        ));
    }
    let env = if env == NONE {
        None
    } else {
        let mark = ctx.heap.temporary_mark();
        let entries = environment(ctx, env)?;
        ctx.discard_truth(mark);
        Some(entries)
    };
    Ok((argv, env))
}

/// Keep an exact list returned by a mapping live. Other iterables are snapshotted before
/// calling the next mapping method or converting the first filesystem argument.
fn mapping_list(ctx: &mut Context, env: Id, method: &str) -> Result<Id> {
    let mark = ctx.heap.temporary_mark();
    let function = ctx.lookup_attr(env, method)?;
    let result = ctx.call(function, Vec::new(), Vec::new())?;
    if matches!(ctx.heap.get(result), Value::List(_)) {
        ctx.finish_call_operands(mark, result);
        return Ok(result);
    }
    let iterator = ctx.iterator(result).map_err(|error| {
        let ty = ctx.builtin("TypeError");
        if ctx.raised_error_matches(&error, ty) {
            Error::new(
                "TypeError",
                format!(
                    "{}.{method}() returned a non-iterable (type {})",
                    ctx.type_name(env),
                    ctx.type_name(result)
                ),
            )
        } else {
            error
        }
    })?;
    ctx.finish_call_operands(mark, iterator);
    let list = ctx.alloc(Value::List(Vec::new()));
    builtins::list_extend(ctx, list, iterator)?;
    ctx.finish_call_operands(mark, list);
    Ok(list)
}

fn environment(ctx: &mut Context, env: Id) -> Result<Vec<CString>> {
    let size = builtins::call(ctx, "len", &[env], &[])?;
    let count = ctx.as_integer(size)? as usize;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(count)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let keys = mapping_list(ctx, env, "keys")?;
    ctx.heap.keep(keys);
    let values = mapping_list(ctx, env, "values")?;
    ctx.heap.keep(values);
    for index in 0..count {
        let mark = ctx.heap.temporary_mark();
        // A key's __fspath__ can remove the value from the mapping's returned list.
        let at = |list| {
            ctx.sequence_at(list, index)
                .and_then(|(_, item)| item)
                .ok_or_else(|| Error::new("IndexError", "list index out of range"))
        };
        let key = at(keys)?;
        let value = at(values)?;
        ctx.heap.keep(key);
        ctx.heap.keep(value);
        let mut key = os::fs_argument(ctx, key)?.into_bytes();
        let value = os::fs_argument(ctx, value)?;
        if key.is_empty() || key[1..].contains(&b'=') {
            return Err(Error::new(
                "ValueError",
                "illegal environment variable name",
            ));
        }
        key.try_reserve(1 + value.as_bytes().len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        key.push(b'=');
        key.extend_from_slice(value.as_bytes());
        entries.push(CString::new(key).expect("both fields have already been checked for NUL"));
        ctx.discard_truth(mark);
    }
    Ok(entries)
}
