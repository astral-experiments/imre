use num_bigint::{BigInt, Sign};

use super::*;

/// Apply the integer addition slot without invoking a reflected method or coercion.
pub(super) fn add(ctx: &mut Context, receiver: Id, other: Id) -> Result<Id> {
    let Ok(rhs) = ctx.as_int(other) else {
        return Ok(NOT_IMPLEMENTED);
    };
    let sum = crate::integer_binary(&ctx.as_int(receiver)?, &rhs, 0)?;
    Ok(ctx.alloc(Value::Int(sum)))
}

/// Return an exact int from a validated integer receiver without invoking subclass conversions.
pub(super) fn exact_value(ctx: &mut Context, receiver: Id) -> Id {
    let value = ctx.scalar_value(receiver);
    match ctx.heap.get(value) {
        Value::Int(_) if value == receiver => value,
        Value::Int(number) => ctx.alloc(Value::Int(number.clone())),
        Value::Bool(bit) => ctx.int(i64::from(*bit)),
        _ => unreachable!("validated integer receiver"),
    }
}

/// Truncate an int or float receiver without invoking subclass conversions.
pub(super) fn truncate_real(ctx: &mut Context, receiver: Id) -> Result<Id> {
    if let Value::Float(value) = *ctx.heap.get(ctx.scalar_value(receiver)) {
        let integer = crate::floats::float_to_integer(value)?;
        Ok(ctx.alloc(Value::Int(crate::Int::new(integer))))
    } else {
        Ok(exact_value(ctx, receiver))
    }
}

/// Both read-only rational fields use the stored integer, bypassing subclass conversions.
pub(super) fn rational_field(ctx: &mut Context, field: &str, args: &[Id]) -> Result<Id> {
    let [receiver] = *args else {
        return Err(type_error("expected one integer getset receiver"));
    };
    let value = ctx.scalar_value(receiver);
    match ctx.heap.get(value) {
        Value::Int(_) | Value::Bool(_) if field == "denominator" => Ok(ctx.int(1)),
        Value::Int(_) | Value::Bool(_) => Ok(exact_value(ctx, receiver)),
        _ => Err(type_error(format!(
            "descriptor '{field}' for 'int' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        ))),
    }
}

/// Read the magnitude of an integer receiver with the no-argument descriptor convention.
pub(super) fn bit_count_or_length(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&receiver, args) = args
        .split_first()
        .ok_or_else(|| type_error(format!("unbound method int.{method}() needs an argument")))?;
    let value = ctx.as_int(receiver).map_err(|_| {
        type_error(format!(
            "descriptor '{method}' for 'int' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        ))
    })?;
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "int.{method}() takes no keyword arguments"
        )));
    }
    if !args.is_empty() {
        return Err(type_error(format!(
            "int.{method}() takes no arguments ({} given)",
            args.len()
        )));
    }
    let bits = if method == "bit_count" {
        value.bit_count()
    } else {
        value.bit_length()
    };
    Ok(ctx.alloc(Value::Int(bits)))
}

fn byte_options(
    ctx: &mut Context,
    method: &str,
    order: Option<Id>,
    signed: Option<Id>,
) -> Result<(bool, bool)> {
    if let Some(order) = order
        && ctx.string_storage(order).is_none()
    {
        let kind = if order == NONE {
            "None"
        } else {
            ctx.type_name(order)
        };
        return Err(type_error(format!(
            "{method}() argument 'byteorder' must be str, not {kind}"
        )));
    }
    let signed = ctx.optional_truth(signed, false)?;
    let little = match order.map(|id| ctx.str_value(id)) {
        None | Some(Ok("big")) => false,
        Some(Ok("little")) => true,
        _ => {
            return Err(Error::new(
                "ValueError",
                "byteorder must be either 'little' or 'big'",
            ));
        }
    };
    Ok((little, signed))
}

pub(super) fn from_bytes(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (&class, args) = args
        .split_first()
        .ok_or_else(|| type_error("descriptor 'from_bytes' of 'int' object needs an argument"))?;
    super::super::stdlib::datetime::validate_classmethod_receiver(ctx, "int", "from_bytes", class)?;
    let [source, order, signed] = bind_method_arguments(
        "from_bytes",
        args,
        kwargs,
        ["bytes", "byteorder", "signed"],
        2,
    )?;
    let source = source
        .ok_or_else(|| type_error("from_bytes() missing required argument 'bytes' (pos 1)"))?;
    let (little, signed) = byte_options(ctx, "from_bytes", order, signed)?;
    let source = text::call_bytes_method(ctx, source)?.unwrap_or(source);
    let items;
    let bytes = match ctx.heap.get(ctx.scalar_value(source)) {
        Value::Bytes(data) | Value::ByteArray(data) => data.as_slice(),
        Value::MemoryView { .. } => ctx.bytes_like(source)?,
        Value::Array(array) => array.as_bytes(),
        Value::Str(_) | Value::SurrogateStr(_) => {
            return Err(type_error(format!(
                "cannot convert '{}' object to bytes",
                ctx.type_name(source)
            )));
        }
        _ => {
            items = bytes_from_iterable(ctx, source)?;
            &items
        }
    };
    let number = match (little, signed) {
        (true, true) => BigInt::from_signed_bytes_le(bytes),
        (false, true) => BigInt::from_signed_bytes_be(bytes),
        (true, false) => BigInt::from_bytes_le(Sign::Plus, bytes),
        (false, false) => BigInt::from_bytes_be(Sign::Plus, bytes),
    };
    let value = ctx.alloc(Value::Int(crate::Int::new(number)));
    if class == ctx.builtin("int") {
        Ok(value)
    } else {
        ctx.call(class, vec![value], Vec::new())
    }
}

pub(super) fn to_bytes(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (&receiver, args) = args
        .split_first()
        .ok_or_else(|| type_error("unbound method int.to_bytes() needs an argument"))?;
    let value = ctx.as_int(receiver).map_err(|_| {
        type_error(format!(
            "descriptor 'to_bytes' for 'int' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        ))
    })?;
    let [length, order, signed] = bind_method_arguments(
        "to_bytes",
        args,
        kwargs,
        ["length", "byteorder", "signed"],
        2,
    )?;
    let size = match length {
        Some(id) => ctx
            .index_integer(id)?
            .to_i64()
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?,
        None => 1,
    };
    let (little, signed) = byte_options(ctx, "to_bytes", order, signed)?;
    let output = crate::objects::integer_bytes(&value, size, little, signed)?;
    Ok(ctx.alloc(Value::Bytes(output)))
}
