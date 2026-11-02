//! Stateless UTF-16 and UTF-32 decoding into Python code points.
use super::{Context, DecodeIssue, Id, Result, resume_decode};
use crate::objects::{FixedUnicode, Utf16Order};
use FixedUnicode::{Utf16, Utf32Be, Utf32Le};
use Utf16Order::{Big, Little};

/// A codec without an explicit byte order strips one BOM or uses native order.
pub(super) fn encoding(key: &str) -> Option<(FixedUnicode, bool)> {
    match key {
        "utf16" | "u16" | "utf_16" => Some((Utf16(Utf16Order::from_sign(0)), true)),
        "utf32" | "u32" | "utf_32" => Some((
            if cfg!(target_endian = "little") {
                Utf32Le
            } else {
                Utf32Be
            },
            true,
        )),
        _ => FixedUnicode::by_key(key).map(|format| (format, false)),
    }
}

pub(super) fn decode(
    ctx: &mut Context,
    input: Id,
    errors: &str,
    format: FixedUnicode,
    bom: bool,
) -> Result<Id> {
    if let Utf16(order) = format {
        let sign = match (bom, order) {
            (true, _) => 0,
            (false, Little) => -1,
            (false, Big) => 1,
        };
        let (text, _, _) = super::utf16::decode(ctx, input, errors, sign, true)?;
        return Ok(text);
    }
    let data = ctx.bytes_like(input)?;
    let (little, start) = match (bom, data) {
        (true, [0xff, 0xfe, 0, 0, ..]) => (true, 4),
        (true, [0, 0, 0xfe, 0xff, ..]) => (false, 4),
        _ => (matches!(format, Utf32Le), 0),
    };
    let mut points = Vec::new();
    let Some(issue) = utf32_prefix(little, &mut points, data, start)? else {
        return Ok(ctx.text_codepoints(points));
    };
    let error = issue.error(data);
    resume_decode(ctx, errors, points, error, |out, bytes, pos| {
        utf32_prefix(little, out, bytes, pos)
    })
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
) -> Result<Option<DecodeIssue>> {
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
            None => ("truncated data", data.len()),
            Some(scalar) => {
                points.push(scalar);
                start += 4;
                continue;
            }
        };
        return Ok(Some(DecodeIssue {
            encoding: if little { "utf-32-le" } else { "utf-32-be" },
            start,
            end,
            reason,
        }));
    }
    Ok(None)
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
