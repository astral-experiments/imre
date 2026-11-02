//! UTF-16 and UTF-32 decoding into Python code points.
use super::{Context, DecodeIssue, Id, Result, resume_decode};
use crate::objects::{FixedUnicode, Utf16Order};
use FixedUnicode::{Utf16, Utf32Be, Utf32Le};
use Utf16Order::{Big, Little};

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
    let data = ctx.bytes_like(input)?;
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
