//! Size queries use the receiver's type, including a weak proxy's own layout.
use super::super::value::Heap;
use super::{Context, Error, Id, Result, Value, builtins};

pub(super) fn getsizeof(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(object), default] =
        builtins::bind_builtin_arguments("getsizeof", args, kwargs, ["object", "default"], 1)?
    else {
        unreachable!("required object argument")
    };
    match (size_of(ctx, object), default) {
        (Err(error), Some(fallback)) if ctx.is_builtin_error(&error, "TypeError") => Ok(fallback),
        (result, _) => result,
    }
}

fn size_of(ctx: &mut Context, object: Id) -> Result<Id> {
    let class = ctx.exact_type(object)?;
    let slot = ctx.class_attr(class, "__sizeof__").ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "Type {:.100} doesn't define __sizeof__",
                ctx.type_name(object)
            ),
        )
    })?;
    let size = if object == super::NONE {
        // Descriptor access uses None as the sentinel for an unbound method.
        ctx.call(slot, vec![object], Vec::new())?
    } else {
        let method = ctx.bind_attribute(slot, object, class)?;
        ctx.call(method, Vec::new(), Vec::new())?
    };
    let integer = ctx
        .as_int(size)
        .map_err(|_| Error::new("TypeError", "an integer is required"))?;
    let value = integer
        .to_i64()
        .and_then(|n| isize::try_from(n).ok())
        .ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?;
    let bytes = usize::try_from(value)
        .map_err(|_| Error::new("ValueError", "__sizeof__() should return >= 0"))?;
    Ok(ctx.alloc(Value::Int(crate::Int::from_usize(
        bytes + Heap::size_overhead(),
    ))))
}
