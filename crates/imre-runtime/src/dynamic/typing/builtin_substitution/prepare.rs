//! Expand fixed-length unpacked tuples and run each parameter's preparation hook.

use crate::dynamic::value::ELLIPSIS;
use crate::dynamic::{Context, Error, Id, NONE, Result, Value, builtins};

fn unpack_args(ctx: &mut Context, item: Id) -> Result<Id> {
    let items = ctx.tuple_items(item)?.unwrap_or_else(|| vec![item]);
    let mut buffer = Vec::new();
    buffer
        .try_reserve(items.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    let expanded = ctx.alloc(Value::List(buffer));
    for arg in items {
        if !ctx.is_type_object(arg)
            && let Some(metadata) = ctx.optional_attr(arg, "__typing_unpacked_tuple_args__")?
            && ctx.builtin_slot_accepts(metadata, "tuple")
        {
            let (size, _) = ctx.sequence_at(metadata, 0).expect("tuple storage");
            if size == 0 || ctx.sequence_at(metadata, size - 1).unwrap().1 != Some(ELLIPSIS) {
                // PyList_SetSlice reads an exact tuple directly, but copies a subclass's iterator.
                ctx.heap.reserve_temporaries(1)?;
                ctx.heap.keep(metadata);
                let source = if matches!(ctx.heap.get(metadata), Value::Tuple(_)) {
                    metadata
                } else {
                    ctx.iterator(metadata)?
                };
                builtins::list_extend(ctx, expanded, source)?;
                continue;
            }
        }
        let Value::List(items) = ctx.heap.get_mut(expanded) else {
            unreachable!()
        };
        items
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        items.push(arg);
    }
    let Value::List(items) = ctx.heap.get_mut(expanded) else {
        unreachable!()
    };
    let result = std::mem::take(items);
    Ok(ctx.alloc(Value::Tuple(result)))
}

// Native TypeVar does not expose its preparation descriptor yet.
fn prepare_typevar(
    ctx: &mut Context,
    alias: Id,
    parameters: &[Id],
    param: Id,
    args: Id,
    default: Id,
) -> Result<Id> {
    // PySequence_Index compares earlier parameters before reaching this TypeVar.
    for (index, &candidate) in parameters.iter().enumerate() {
        if candidate != param && !ctx.checked_equal(candidate, param)? {
            continue;
        }
        let actual = ctx.sequence_len(args)?;
        if index < actual {
            return Ok(args);
        }
        if index == actual && !matches!(ctx.heap.get(default), Value::NoDefault) {
            let extra = ctx.alloc(Value::Tuple(vec![default]));
            return ctx.binary("+", args, extra);
        }
        let name = ctx.checked_format(alias, false)?;
        return Err(Error::new(
            "TypeError",
            format!(
                "Too few arguments for {name}; actual {actual}, expected at least {}",
                index + 1
            ),
        ));
    }
    Err(Error::new(
        "ValueError",
        "sequence.index(x): x not in sequence",
    ))
}

/// Run the hooks in parameter order, preserving a non-tuple result as one argument.
pub(super) fn prepared(ctx: &mut Context, alias: Id, parameters: &[Id], item: Id) -> Result<Id> {
    let mut args = unpack_args(ctx, item)?;
    for &param in parameters {
        let hook = ctx.optional_attr(param, "__typing_prepare_subst__")?;
        if hook == Some(NONE)
            || (hook.is_none()
                && !matches!(
                    ctx.heap.get(param),
                    Value::TypeVar(_) | Value::TypeVarTuple { .. }
                ))
        {
            continue;
        }
        if let Some(hook) = hook {
            ctx.heap.reserve_temporaries(1)?;
            ctx.heap.keep(hook);
        }
        if !ctx.builtin_slot_accepts(args, "tuple") {
            args = ctx.alloc(Value::Tuple(vec![args]));
        }
        args = match (hook, ctx.heap.get(param)) {
            (Some(hook), _) => ctx.call(hook, vec![alias, args], Vec::new())?,
            (None, Value::TypeVar(var)) => {
                let default = var.default;
                prepare_typevar(ctx, alias, parameters, param, args, default)?
            }
            (None, Value::TypeVarTuple { .. }) => {
                ctx.module("typing")?;
                ctx.call_typing_helper("_prepare_typevartuple", vec![param, alias, args])?
            }
            _ => unreachable!(),
        };
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(args);
    }
    Ok(args)
}
