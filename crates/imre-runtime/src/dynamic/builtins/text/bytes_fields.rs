//! Binary operands for old-style bytes interpolation.
use super::*;

pub(super) fn is_mapping(ctx: &Context, rhs: Id) -> bool {
    match ctx.heap.get(ctx.scalar_value(rhs)) {
        Value::Dict(_)
        | Value::DefaultDict { .. }
        | Value::MappingProxy(_)
        | Value::Context(_)
        | Value::List(_)
        | Value::Range(_) => true,
        Value::Instance { class, .. } => {
            ctx.namedtuple_fields(*class).is_none()
                && (ctx.slot(rhs, "__getitem__").is_some() || ctx.dict_storage(rhs).is_some())
        }
        _ => false,
    }
}

/// Use the raw binary payload first, then the type's special conversion method.
fn binary_operand(ctx: &mut Context, value: Id) -> Result<&[u8]> {
    if matches!(
        ctx.heap.get(ctx.scalar_value(value)),
        Value::Bytes(_) | Value::ByteArray(_)
    ) {
        return ctx.bytes_like(value);
    }
    // A descriptor lookup failure falls through to the buffer protocol.
    if let Ok(Some(method)) = ctx.bound_slot(value, "__bytes__") {
        let result = ctx.call(method, Vec::new(), Vec::new())?;
        let bytes = ctx.builtin("bytes");
        if ctx.instance_of(result, bytes) {
            return ctx.bytes_like(result);
        }
        return Err(type_error(format!(
            "__bytes__ returned non-bytes (type {:.200})",
            ctx.type_name(result)
        )));
    }
    ctx.bytes_like(value).map_err(|_| type_error(format!(
        "%b requires a bytes-like object, or an object that implements __bytes__, not '{:.100}'",
        ctx.type_name(value)
    )))
}

/// CPython consumes starred quantities even when the conversion character is invalid.
pub(super) fn consume_pending_stars(
    ctx: &Context,
    prefix: &[u8],
    args: &mut std::vec::IntoIter<Id>,
) -> Result<()> {
    if prefix.contains(&b'*') {
        // The parser has already validated this prefix. Only stars can consume operands.
        let quantities = prefix
            .iter()
            .copied()
            .filter(|b| matches!(b, b'.' | b'*'))
            .chain(std::iter::once(b's'));
        let spec = CFormatSpec::parse(
            &mut quantities.enumerate().peekable(),
            CFormatContext::Bytes,
        )
        .expect("previously parsed field quantities");
        percent_layout(ctx, &spec, args)?;
    }
    Ok(())
}

/// Width and precision count bytes. Both conversions space-pad and ignore numeric flags.
pub(super) fn append(
    ctx: &mut Context,
    spec: &CFormatSpec,
    args: &mut std::vec::IntoIter<Id>,
    output: &mut Vec<u8>,
    remaining: usize,
) -> Result<()> {
    let (width, precision) = percent_layout(ctx, spec, args)?;
    let left = width < 0 || spec.flags.contains(CConversionFlags::LEFT_ADJUST);
    let width = width.checked_abs().unwrap_or(0) as usize;
    let value = args
        .next()
        .ok_or_else(|| type_error("not enough arguments for format string"))?;
    let data = binary_operand(ctx, value)?;
    let data = &data[..precision.map_or(data.len(), |n| n.min(data.len()))];
    let padding = width.saturating_sub(data.len());
    output
        .try_reserve(width.max(data.len()).saturating_add(remaining))
        .map_err(|_| Error::new("MemoryError", ""))?;
    if !left {
        output.resize(output.len() + padding, b' ');
    }
    output.extend_from_slice(data);
    if left {
        output.resize(output.len() + padding, b' ');
    }
    Ok(())
}
