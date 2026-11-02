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

pub(super) fn bit_length(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (&receiver, args) = args
        .split_first()
        .ok_or_else(|| type_error("unbound method int.bit_length() needs an argument"))?;
    let value = ctx.as_int(receiver).map_err(|_| {
        type_error(format!(
            "descriptor 'bit_length' for 'int' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        ))
    })?;
    if !kwargs.is_empty() {
        return Err(type_error("int.bit_length() takes no keyword arguments"));
    }
    if !args.is_empty() {
        return Err(type_error(format!(
            "int.bit_length() takes no arguments ({} given)",
            args.len()
        )));
    }
    let bits = value.as_small().map_or_else(
        || value.big().bits(),
        |n| u64::from(u64::BITS - n.unsigned_abs().leading_zeros()),
    );
    Ok(ctx.alloc(Value::Int(crate::Int::from_usize(bits as usize))))
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
        .ok_or_else(|| type_error("from_bytes needs a class"))?;
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
    if size < 0 {
        return Err(Error::new(
            "ValueError",
            "length argument must be non-negative",
        ));
    }
    let size = size as usize;
    if size > crate::MAX_BYTES_LEN {
        return Err(Error::new("OverflowError", "byte string is too large"));
    }
    // CPython allocates the result before checking whether the integer fits.
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let value = value.big();
    let negative = value.sign() == Sign::Minus;
    if negative && !signed {
        return Err(Error::new(
            "OverflowError",
            "can't convert negative int to unsigned",
        ));
    }
    let bits = value.bits();
    // At a signed boundary the negative power of two already includes its sign bit.
    let sign_bit = signed && bits != 0 && !(negative && value.trailing_zeros() == Some(bits - 1));
    if (bits + u64::from(sign_bit)).div_ceil(8) > size as u64 {
        return Err(Error::new("OverflowError", "int too big to convert"));
    }
    output.resize(size, 0u8);
    for (byte, source) in output
        .iter_mut()
        .zip(value.iter_u32_digits().flat_map(u32::to_le_bytes))
    {
        *byte = source;
    }
    if negative {
        let mut carry = true;
        for byte in &mut output {
            (*byte, carry) = (!*byte).overflowing_add(u8::from(carry));
        }
    }
    if !little {
        output.reverse();
    }
    Ok(ctx.alloc(Value::Bytes(output)))
}
