use super::*;

/// Construct a boxed binary64 using Python's numeric slots or decimal text grammar.
pub(super) fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error("float() takes no keyword arguments"));
    }
    let value = match args {
        [] => return Ok(ctx.alloc(Value::Float(0.0))),
        [value] => *value,
        _ => {
            return Err(type_error(format!(
                "float expected at most 1 argument, got {}",
                args.len()
            )));
        }
    };
    if let Some(number) = from_numeric(ctx, value)? {
        return Ok(number);
    }
    let parsed = if let Ok(text) = ctx.str_value(value) {
        crate::floats::parse(text)
    } else if let Value::Bytes(bytes) | Value::ByteArray(bytes) = ctx.heap.get(value) {
        crate::floats::parse_bytes(bytes)
    } else {
        return Err(type_error(format!(
            "float() argument must be a string or a real number, not '{}'",
            ctx.type_name(value)
        )));
    };
    let number = parsed.ok_or_else(|| {
        Error::new(
            "ValueError",
            format!("could not convert string to float: {}", ctx.repr(value)),
        )
    })?;
    Ok(ctx.alloc(Value::Float(number)))
}

/// Return the float supplied by the numeric protocol, preserving callback results and errors.
pub(in crate::dynamic) fn from_numeric(ctx: &mut Context, value: Id) -> Result<Option<Id>> {
    if matches!(ctx.heap.get(value), Value::Float(_)) {
        return Ok(Some(value));
    }
    if let Some(method) = ctx.bound_slot(value, "__float__")? {
        let result = ctx.call(method, Vec::new(), Vec::new())?;
        if matches!(ctx.heap.get(result), Value::Float(_)) {
            return Ok(Some(result));
        }
        return Err(type_error(format!(
            "{}.__float__ returned non-float (type {})",
            ctx.type_name(value),
            ctx.type_name(result),
        )));
    }
    if let Some(integer) = ctx.index_value(value)? {
        let number = crate::floats::int_to_float(&integer)?;
        return Ok(Some(ctx.alloc(Value::Float(number))));
    }
    Ok(None)
}
