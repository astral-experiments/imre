use super::{option, tuple_text};
use crate::dynamic::{
    Context,
    value::{Error, Id, Result, Value},
};

pub(super) const ASSIGNED: &[&str] = &[
    "__module__",
    "__name__",
    "__qualname__",
    "__doc__",
    "__annotate__",
    "__type_params__",
];
pub(super) const UPDATED: &[&str] = &["__dict__"];

pub(super) fn default_cache(ctx: &mut Context, function: Id) -> Result<Id> {
    if !ctx.callable(function) {
        return Err(Error::new(
            "TypeError",
            "the first argument must be callable",
        ));
    }
    let wrapper = ctx.cached_function(function);
    let mark = ctx.heap.temporary_mark();
    ctx.heap.keep(wrapper);
    let result = (|| {
        let parameters = ctx.alloc(Value::CacheParameters);
        ctx.set_attr(wrapper, "cache_parameters", parameters)?;
        update_wrapper(ctx, &[wrapper, function], &[])
    })();
    ctx.heap.release_temporaries_since(mark);
    result
}

pub(super) fn update_wrapper(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let wrapper = option(args, 0, kwargs, "wrapper")?;
    let wrapped = option(args, 1, kwargs, "wrapped")?;
    let (wrapper, wrapped) = match (wrapper, wrapped) {
        (Some(wrapper), Some(wrapped)) => (wrapper, wrapped),
        (None, None) => {
            return Err(Error::new(
                "TypeError",
                "update_wrapper() missing 2 required positional arguments: 'wrapper' and 'wrapped'",
            ));
        }
        (only, _) => {
            let name = if only.is_none() { "wrapper" } else { "wrapped" };
            return Err(Error::new(
                "TypeError",
                format!("update_wrapper() missing 1 required positional argument: '{name}'"),
            ));
        }
    };
    let assigned =
        option(args, 2, kwargs, "assigned")?.unwrap_or_else(|| tuple_text(ctx, ASSIGNED));
    let updated = option(args, 3, kwargs, "updated")?.unwrap_or_else(|| tuple_text(ctx, UPDATED));
    for key in ctx.items(assigned)? {
        let name = ctx.str_value(key)?.to_owned();
        match ctx.lookup_attr(wrapped, &name) {
            Ok(value) => ctx.set_attr(wrapper, &name, value)?,
            Err(error) if ctx.is_attribute_error(&error) => {}
            Err(error) => return Err(error),
        }
    }
    for key in ctx.items(updated)? {
        let name = ctx.str_value(key)?.to_owned();
        let target = ctx.lookup_attr(wrapper, &name)?;
        let update = ctx.lookup_attr(target, "update")?;
        let source = match ctx.lookup_attr(wrapped, &name) {
            Ok(value) => value,
            Err(error) if ctx.is_attribute_error(&error) => ctx.dict(Vec::new()),
            Err(error) => return Err(error),
        };
        ctx.call(update, vec![source], Vec::new())?;
    }
    // A wrapped function's dictionary can itself contain __wrapped__.
    ctx.set_attr(wrapper, "__wrapped__", wrapped)?;
    Ok(wrapper)
}
