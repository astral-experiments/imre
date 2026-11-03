//! UTF-16 and UTF-32 decoding into Python code points.
use super::{Context, DecodeIssue, Id, Result, Value, resume_decode, type_error};
use crate::objects::{FixedUnicode, Utf16Order};
use FixedUnicode::{Utf16, Utf32Be, Utf32Le};
use Utf16Order::{Big, Little};

/// Both wide codec families use the signed byte-order argument from the CPython API.
fn arguments(function: &str, args: &[Id], kwargs: &[(String, Id)], maximum: usize) -> Result<()> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "_codecs.{function}() takes no keyword arguments"
        )));
    }
    if args.is_empty() {
        return Err(type_error(format!(
            "{function} expected at least 1 argument, got 0"
        )));
    }
    if args.len() > maximum {
        return Err(type_error(format!(
            "{function} expected at most {maximum} arguments, got {}",
            args.len()
        )));
    }
    Ok(())
}

fn order(ctx: &mut Context, arg: Option<&Id>) -> Result<i32> {
    arg.map(|&id| super::super::os::c_int(ctx, id))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn format(function: &str) -> FixedUnicode {
    if function.starts_with("utf_16") {
        Utf16(Little)
    } else {
        Utf32Le
    }
}

fn policy(ctx: &mut Context, function: &str, arg: Option<Id>) -> Result<Option<Id>> {
    if let Some(id) = arg
        && matches!(ctx.heap.get(ctx.scalar_value(id)), Value::SurrogateStr(_))
    {
        ctx.encode_utf8_codepoints(id, false, "strict")?;
    }
    super::entry_error_policy(ctx, function, arg)
}

pub(in crate::dynamic::stdlib) fn encode(
    ctx: &mut Context,
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    arguments(function, args, kwargs, 3)?;
    let input = args[0];
    let length = super::encoder_input_length(ctx, function, input)?;
    let errors = policy(ctx, function, args.get(1).copied())?;
    let order = order(ctx, args.get(2))?;
    let errors = super::error_name(ctx, errors)?.to_owned();
    let bytes = super::encode_fixed(ctx, input, format(function), &errors, order)?;
    let data = ctx.heap.bytes_from_data(bytes);
    let consumed = ctx.alloc(Value::Int(crate::Int::from_usize(length)));
    Ok(ctx.alloc(Value::Tuple(vec![data, consumed])))
}

pub(in crate::dynamic::stdlib) fn extended(
    ctx: &mut Context,
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    arguments(function, args, kwargs, 4)?;
    let input = args[0];
    ctx.check_buffer_exporter(input)?;
    ctx.native_buffer_bytes(input)?;
    ctx.with_buffer_export(input, |ctx| {
        let errors = policy(ctx, function, args.get(1).copied())?;
        let errors = super::error_name(ctx, errors)?.to_owned();
        let order = order(ctx, args.get(2))?;
        let final_chunk = args
            .get(3)
            .map(|&id| ctx.truth(id))
            .transpose()?
            .unwrap_or(false);
        let (text, consumed, order) =
            decode_chunk(ctx, input, &errors, format(function), order, final_chunk)?;
        let consumed = ctx.alloc(Value::Int(crate::Int::from_usize(consumed)));
        let order = ctx.int(i64::from(order));
        Ok(ctx.alloc(Value::Tuple(vec![text, consumed, order])))
    })
}

pub(super) fn decode(
    ctx: &mut Context,
    input: Id,
    errors: &str,
    format: FixedUnicode,
    bom: bool,
) -> Result<Id> {
    let order = if bom { 0 } else { format.byte_order() };
    decode_chunk(ctx, input, errors, format, order, true).map(|(text, _, _)| text)
}

/// Decode complete units and report both the consumed bytes and the detected byte order.
/// An order of zero remains undecided until a BOM is consumed.
pub(in crate::dynamic::stdlib) fn decode_chunk(
    ctx: &mut Context,
    input: Id,
    errors: &str,
    format: FixedUnicode,
    order: i32,
    final_chunk: bool,
) -> Result<(Id, usize, i32)> {
    if let Utf16(_) = format {
        return super::utf16::decode(ctx, input, errors, order, final_chunk);
    }
    let data = ctx.native_buffer_bytes(input)?;
    let (order, start) = match (order, data.first_chunk::<4>()) {
        (0, Some([0xff, 0xfe, 0, 0])) => (-1, 4),
        (0, Some([0, 0, 0xfe, 0xff])) => (1, 4),
        _ => (order, 0),
    };
    let little = matches!(Utf16Order::from_sign(order), Little);
    let size = data.len();
    let mut points = Vec::new();
    let (mut consumed, issue) = utf32_prefix(little, &mut points, data, start, final_chunk)?;
    let text = if let Some(issue) = issue {
        let error = issue.error(data);
        resume_decode(ctx, errors, points, error, |out, bytes, pos| {
            let (end, issue) = utf32_prefix(little, out, bytes, pos, final_chunk)?;
            consumed = end;
            Ok(issue)
        })?
    } else {
        ctx.text_codepoints(points)
    };
    Ok((text, if final_chunk { size } else { consumed }, order))
}

fn utf32_unit(little: bool, data: &[u8]) -> Option<u32> {
    let unit = *data.first_chunk()?;
    Some(if little {
        u32::from_le_bytes(unit)
    } else {
        u32::from_be_bytes(unit)
    })
}

fn utf32_prefix(
    little: bool,
    points: &mut Vec<u32>,
    data: &[u8],
    mut start: usize,
    final_chunk: bool,
) -> Result<(usize, Option<DecodeIssue>)> {
    points
        .try_reserve((data.len() - start) / 4)
        .map_err(|_| crate::Error::allocation())?;
    while start < data.len() {
        let (reason, end) = match utf32_unit(little, &data[start..]) {
            Some(0xd800..=0xdfff) => (
                "code point in surrogate code point range(0xd800, 0xe000)",
                start + 4,
            ),
            Some(0x110000..) => ("code point not in range(0x110000)", start + 4),
            None if !final_chunk => break,
            None => ("truncated data", data.len()),
            Some(scalar) => {
                points.push(scalar);
                start += 4;
                continue;
            }
        };
        return Ok((
            start,
            Some(DecodeIssue {
                encoding: if little { "utf-32-le" } else { "utf-32-be" },
                start,
                end,
                reason,
            }),
        ));
    }
    Ok((start, None))
}

/// surrogatepass accepts UTF-name variants but does not consult arbitrary codec aliases.
pub(super) fn surrogate_encoding(name: &str) -> Option<FixedUnicode> {
    let (prefix, rest) = name.split_at_checked(3)?;
    if !prefix.eq_ignore_ascii_case("utf") {
        return None;
    }
    let rest = rest.strip_prefix(['-', '_']).unwrap_or(rest);
    let (width, end) = rest.split_at_checked(2)?;
    let little = if end.is_empty() {
        cfg!(target_endian = "little")
    } else {
        let end = end.strip_prefix(['-', '_']).unwrap_or(end);
        if end.eq_ignore_ascii_case("le") {
            true
        } else if end.eq_ignore_ascii_case("be") {
            false
        } else {
            return None;
        }
    };
    match (width, little) {
        ("16", true) => Some(Utf16(Little)),
        ("16", false) => Some(Utf16(Big)),
        ("32", true) => Some(Utf32Le),
        ("32", false) => Some(Utf32Be),
        _ => None,
    }
}

pub(super) fn surrogate(format: FixedUnicode, data: &[u8]) -> Option<(u32, usize)> {
    let (point, width) = match format {
        Utf16(order) => (u32::from(order.unit(*data.first_chunk()?)), 2),
        Utf32Le | Utf32Be => (utf32_unit(matches!(format, Utf32Le), data)?, 4),
    };
    (0xd800..=0xdfff).contains(&point).then_some((point, width))
}
