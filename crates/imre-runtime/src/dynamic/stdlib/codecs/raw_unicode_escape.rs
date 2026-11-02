//! Raw Unicode escapes preserve Latin-1 bytes and unpaired surrogate code points.
use super::{Context, DecodeIssue, Id, Result, resume_decode};

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
