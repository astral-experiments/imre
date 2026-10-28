use num_bigint::Sign;

use super::*;

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
    if let Some(order) = order
        && ctx.str_value(order).is_err()
    {
        let kind = if order == NONE {
            "None"
        } else {
            ctx.type_name(order)
        };
        return Err(type_error(format!(
            "to_bytes() argument 'byteorder' must be str, not {kind}"
        )));
    }
    let signed = ctx.optional_truth(signed, false)?;
    let little = match order
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("big")
    {
        "little" => true,
        "big" => false,
        _ => {
            return Err(Error::new(
                "ValueError",
                "byteorder must be either 'little' or 'big'",
            ));
        }
    };
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
