//! Construction and comparison for types.SimpleNamespace.

use super::value::{Error, Id, NONE, NOT_IMPLEMENTED, Result, Value};
use super::{Context, builtins};

pub(super) fn initialize(
    ctx: &mut Context,
    object: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Value::Instance { class, .. } = *ctx.heap.get(object) else {
        unreachable!("validated SimpleNamespace instance");
    };
    if args.len() > 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "{} expected at most 1 argument, got {}",
                ctx.class_name(class),
                args.len()
            ),
        ));
    }
    let dictionary = ctx.instance_dictionary(object, class);
    let input = if let Some(&source) = args.first() {
        // Convert and validate the whole positional argument before updating the namespace.
        let mapping = if matches!(ctx.heap.get(source), Value::Dict(_)) {
            source
        } else {
            let fresh = ctx.dict(Vec::new());
            builtins::update_dict(ctx, fresh, Some(source), &[])?;
            fresh
        };
        if ctx
            .dict_entries(mapping)
            .expect("exact dict")
            .iter()
            .any(|&(key, _)| ctx.string_storage(key).is_none())
        {
            return Err(Error::new("TypeError", "keywords must be strings"));
        }
        Some(mapping)
    } else {
        None
    };
    builtins::update_dict(ctx, dictionary, input, kwargs)?;
    Ok(NONE)
}

pub(super) fn equal(ctx: &mut Context, object: Id, other: Id, invert: bool) -> Result<Id> {
    let Value::Instance {
        class: other_class, ..
    } = *ctx.heap.get(other)
    else {
        return Ok(NOT_IMPLEMENTED);
    };
    if !ctx.instance_of(other, ctx.classes["types.SimpleNamespace"]) {
        return Ok(NOT_IMPLEMENTED);
    }
    let Value::Instance { class, .. } = *ctx.heap.get(object) else {
        unreachable!("validated SimpleNamespace instance");
    };
    let left = ctx.instance_dictionary(object, class);
    let right = ctx.instance_dictionary(other, other_class);
    ctx.binary(if invert { "!=" } else { "==" }, left, right)
}
