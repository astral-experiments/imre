//! Construction, replacement and comparison for types.SimpleNamespace.

use super::value::{Error, Id, NONE, NOT_IMPLEMENTED, Result, Value};
use super::{Context, builtins};

/// Allocate bare namespace storage. The class call runs the initializer separately.
pub(super) fn allocate(ctx: &mut Context, args: &[Id]) -> Result<Id> {
    let owner = super::namespace::CLASS;
    let base = ctx.classes[owner];
    let Some(&class) = args.first() else {
        return Err(Error::new(
            "TypeError",
            format!("{owner}.__new__(): not enough arguments"),
        ));
    };
    if !ctx.direct_subclass(class, base) {
        return Err(ctx.invalid_new_subtype(owner, class));
    }
    if let Some(layout) = ctx.allocation_base(class)
        && layout != base
    {
        return Err(Error::new(
            "TypeError",
            format!(
                "{owner}.__new__({}) is not safe, use {}.__new__()",
                ctx.class_name(class),
                ctx.class_name(layout)
            ),
        ));
    }
    Ok(ctx.instance(class, None))
}

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

/// Invoke the actual class with no arguments, then update its namespace storage in place.
pub(super) fn replace(
    ctx: &mut Context,
    object: Id,
    class: Id,
    changes: &[(String, Id)],
) -> Result<Id> {
    let replacement = ctx.call(class, vec![], vec![])?;
    let Some(target_class) = super::namespace::instance_class(ctx, replacement) else {
        let returned = ctx.exact_type(replacement)?;
        let constructor = super::stdlib::qualified_type(ctx, class)?;
        let returned = super::stdlib::qualified_type(ctx, returned)?;
        return Err(Error::new(
            "TypeError",
            format!(
                "expect types.SimpleNamespace type, but {}() returned '{}' object",
                ctx.str_value(constructor)?,
                ctx.str_value(returned)?,
            ),
        ));
    };
    let source = ctx.instance_dictionary(object, class);
    let target = ctx.instance_dictionary(replacement, target_class);
    builtins::update_dict(ctx, target, Some(source), changes)?;
    Ok(replacement)
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
