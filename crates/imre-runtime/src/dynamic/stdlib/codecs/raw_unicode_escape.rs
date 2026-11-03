//! Raw Unicode escapes preserve Latin-1 bytes and unpaired surrogate code points.
use std::io::Write;

use super::{Context, DecodeIssue, Id, NONE, Result, Value, resume_decode, type_error};

/// Encode every Python code point, including unpaired surrogates, without calling an error handler.
pub(super) fn encode(ctx: &mut Context, input: Id) -> Result<Id> {
    let output = match ctx.heap.get(ctx.scalar_value(input)) {
        Value::Str(text) => encode_points(text.chars().map(u32::from))?,
        Value::SurrogateStr(points) => encode_points(points.iter().copied())?,
        _ => {
            let kind = if input == NONE {
                "None"
            } else {
                ctx.type_name(input)
            };
            return Err(type_error(format!(
                "raw_unicode_escape_encode() argument 1 must be str, not {kind}"
            )));
        }
    };
    Ok(ctx.heap.bytes_from_data(output))
}

fn encode_points(points: impl Iterator<Item = u32>) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for point in points {
        // The longest escape consists of a backslash, an uppercase U and eight hex digits.
        output
            .try_reserve(10)
            .map_err(|_| crate::Error::allocation())?;
        match point {
            0..=0xff => output.push(point as u8),
            0x100..=0xffff => write!(output, "\\u{point:04x}").unwrap(),
            _ => write!(output, "\\U{point:08x}").unwrap(),
        }
    }
    Ok(output)
}

pub(super) fn decode(ctx: &mut Context, input: Id, errors: &str) -> Result<Id> {
    let data = ctx.bytes_like(input)?;
    let mut points = Vec::new();
    let Some(issue) = prefix(&mut points, data, 0)? else {
        return Ok(ctx.text_codepoints(points));
    };
    let error = issue.error(data);
    resume_decode(ctx, errors, points, error, prefix)
}

fn prefix(points: &mut Vec<u32>, data: &[u8], mut offset: usize) -> Result<Option<DecodeIssue>> {
    points
        .try_reserve(data.len() - offset)
        .map_err(|_| crate::Error::allocation())?;
    while let Some(&byte) = data.get(offset) {
        let start = offset;
        offset += 1;
        if byte != b'\\' {
            points.push(u32::from(byte));
            continue;
        }
        let (digits, truncated) = match data.get(offset) {
            Some(b'u') => (4, r"truncated \uXXXX escape"),
            Some(b'U') => (8, r"truncated \UXXXXXXXX escape"),
            Some(b'\\') => {
                // A pair stays literal, even when immediately followed by u or U.
                points.extend([u32::from(b'\\'); 2]);
                offset += 1;
                continue;
            }
            _ => {
                points.push(u32::from(byte));
                continue;
            }
        };
        offset += 1;
        let end = offset + digits;
        let mut point = 0;
        while offset < end {
            let Some(digit) = data.get(offset).and_then(|&b| char::from(b).to_digit(16)) else {
                break;
            };
            point = point * 16 + digit;
            offset += 1;
        }
        let reason = if offset != end {
            truncated
        } else if point > 0x10ffff {
            r"\Uxxxxxxxx out of range"
        } else {
            points.push(point);
            continue;
        };
        return Ok(Some(DecodeIssue {
            encoding: "rawunicodeescape",
            start,
            end: offset,
            reason,
        }));
    }
    Ok(None)
}
