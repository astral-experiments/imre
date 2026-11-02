//! Stateless UTF-16 and UTF-32 decoding into Python code points.
use super::{Context, DecodeIssue, Id, Result, resume_decode};
use crate::objects::FixedUnicode;
use FixedUnicode::{Utf16Be, Utf16Le, Utf32Be, Utf32Le};

/// A codec without an explicit byte order strips one BOM or uses native order.
pub(super) fn encoding(key: &str) -> Option<(FixedUnicode, bool)> {
    match key {
        "utf16" | "u16" | "utf_16" => Some((
            if cfg!(target_endian = "little") {
                Utf16Le
            } else {
                Utf16Be
            },
            true,
        )),
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
    let data = ctx.bytes_like(input)?;
    let (format, start) = if bom {
        match (format, data) {
            (Utf16Le | Utf16Be, [0xff, 0xfe, ..]) => (Utf16Le, 2),
            (Utf16Le | Utf16Be, [0xfe, 0xff, ..]) => (Utf16Be, 2),
            (Utf32Le | Utf32Be, [0xff, 0xfe, 0, 0, ..]) => (Utf32Le, 4),
            (Utf32Le | Utf32Be, [0, 0, 0xfe, 0xff, ..]) => (Utf32Be, 4),
            _ => (format, 0),
        }
    } else {
        (format, 0)
    };
    let mut points = Vec::new();
    let Some(issue) = prefix(format, &mut points, data, start)? else {
        return Ok(ctx.text_codepoints(points));
    };
    let error = issue.error(data);
    resume_decode(ctx, errors, points, error, |out, bytes, pos| {
        prefix(format, out, bytes, pos)
    })
}

fn unit(format: FixedUnicode, data: &[u8]) -> Option<(u32, usize)> {
    Some(match format {
        Utf16Le => (u32::from(u16::from_le_bytes(*data.first_chunk()?)), 2),
        Utf16Be => (u32::from(u16::from_be_bytes(*data.first_chunk()?)), 2),
        Utf32Le => (u32::from_le_bytes(*data.first_chunk()?), 4),
        Utf32Be => (u32::from_be_bytes(*data.first_chunk()?), 4),
    })
}

fn prefix(
    format: FixedUnicode,
    points: &mut Vec<u32>,
    data: &[u8],
    mut offset: usize,
) -> Result<Option<DecodeIssue>> {
    let width = match format {
        Utf16Le | Utf16Be => 2,
        Utf32Le | Utf32Be => 4,
    };
    points
        .try_reserve((data.len() - offset) / width)
        .map_err(|_| crate::Error::allocation())?;
    while offset < data.len() {
        let start = offset;
        let issue = |end, reason| {
            Ok(Some(DecodeIssue {
                encoding: format.name(),
                start,
                end,
                reason,
            }))
        };
        let Some((point, _)) = unit(format, &data[offset..]) else {
            return issue(data.len(), "truncated data");
        };
        offset += width;
        let point = match point {
            0xd800..=0xdbff if width == 2 => {
                let Some((trail, _)) = unit(format, &data[offset..]) else {
                    return issue(data.len(), "unexpected end of data");
                };
                if !(0xdc00..=0xdfff).contains(&trail) {
                    return issue(offset, "illegal UTF-16 surrogate");
                }
                offset += 2;
                0x10000 + ((point - 0xd800) << 10) + (trail - 0xdc00)
            }
            0xdc00..=0xdfff if width == 2 => return issue(offset, "illegal encoding"),
            0xd800..=0xdfff => {
                return issue(
                    offset,
                    "code point in surrogate code point range(0xd800, 0xe000)",
                );
            }
            0x110000.. => return issue(offset, "code point not in range(0x110000)"),
            scalar => scalar,
        };
        points.push(point);
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
        ("16", true) => Some(Utf16Le),
        ("16", false) => Some(Utf16Be),
        ("32", true) => Some(Utf32Le),
        ("32", false) => Some(Utf32Be),
        _ => None,
    }
}

pub(super) fn surrogate(format: FixedUnicode, data: &[u8]) -> Option<(u32, usize)> {
    unit(format, data).filter(|(point, _)| (0xd800..=0xdfff).contains(point))
}
