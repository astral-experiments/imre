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
        let scalar = ctx.scalar_value(result);
        if matches!(ctx.heap.get(scalar), Value::Float(_)) {
            return Ok(Some(scalar));
        }
        return Err(type_error(format!(
            "{}.__float__ returned non-float (type {})",
            ctx.type_name(value),
            ctx.type_name(result),
        )));
    }
    let scalar = ctx.scalar_value(value);
    if matches!(ctx.heap.get(scalar), Value::Float(_)) {
        return Ok(Some(scalar));
    }
    if let Some(integer) = ctx.index_value(value)? {
        let number = crate::floats::int_to_float(&integer)?;
        return Ok(Some(ctx.alloc(Value::Float(number))));
    }
    Ok(None)
}

/// Sum a boxed iterable, retaining low-order bits while adding real numbers to a float.
pub(super) fn sum(ctx: &mut Context, source: Id, start: Option<Id>) -> Result<Id> {
    let iterator = ctx.iterator(source)?;
    let mut total = start.unwrap_or_else(|| ctx.int(0));
    let unsuitable = match ctx.heap.get(ctx.scalar_value(total)) {
        Value::Str(_) => Some(("strings", "''")),
        Value::Bytes(_) => Some(("bytes", "b''")),
        Value::ByteArray(_) => Some(("bytearray", "b''")),
        _ => None,
    };
    if let Some((kind, empty)) = unsuitable {
        return Err(type_error(format!(
            "sum() can't sum {kind} [use {empty}.join(seq) instead]"
        )));
    }
    let mark = ctx.heap.temporary_mark();
    let Some(mut item) = ctx.next_item(iterator)? else {
        return Ok(total);
    };

    if let Value::Int(initial) = ctx.heap.get(total)
        && let Some(mut integer) = initial.to_i64()
    {
        loop {
            let value = match ctx.heap.get(item) {
                Value::Int(value) => value.to_i64(),
                Value::Bool(value) => Some(i64::from(*value)),
                _ => None,
            };
            let Some(next) = value.and_then(|value| integer.checked_add(value)) else {
                let accumulated = ctx.int(integer);
                total = ctx.binary("+", accumulated, item)?;
                break;
            };
            integer = next;
            ctx.heap.release_temporaries_since(mark);
            let Some(next) = ctx.next_item(iterator)? else {
                return Ok(ctx.int(integer));
            };
            item = next;
        }
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(total);
        let Some(next) = ctx.next_item(iterator)? else {
            return Ok(total);
        };
        item = next;
    }

    if let Value::Float(mut high) = *ctx.heap.get(total) {
        let mut low = 0.0_f64;
        loop {
            let value = match ctx.heap.get(item) {
                Value::Float(value) => Some(*value),
                _ => match ctx.heap.get(ctx.scalar_value(item)) {
                    Value::Int(integer) => Some(crate::floats::int_to_float(integer)?),
                    Value::Bool(value) => Some(f64::from(u8::from(*value))),
                    _ => None,
                },
            };
            let Some(value) = value else {
                total = ctx.alloc(Value::Float(compensated_value(high, low)));
                break;
            };
            let rounded = high + value;
            low += if high.abs() >= value.abs() {
                (high - rounded) + value
            } else {
                (value - rounded) + high
            };
            high = rounded;
            ctx.heap.release_temporaries_since(mark);
            let Some(next) = ctx.next_item(iterator)? else {
                return Ok(ctx.alloc(Value::Float(compensated_value(high, low))));
            };
            item = next;
        }
    }

    loop {
        total = ctx.binary("+", total, item)?;
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(total);
        let Some(next) = ctx.next_item(iterator)? else {
            return Ok(total);
        };
        item = next;
    }
}

/// Avoid changing the sign of zero or turning an infinite result into a NaN.
fn compensated_value(high: f64, low: f64) -> f64 {
    if low != 0.0 && low.is_finite() {
        high + low
    } else {
        high
    }
}
