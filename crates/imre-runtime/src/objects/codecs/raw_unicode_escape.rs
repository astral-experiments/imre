//! Stateless raw-unicode-escape conversion. Literal bytes always represent Latin-1.
use super::super::bytes::DecodeIssue;
use super::{Error, Result};

/// Leave an unmatched backslash or unfinished hexadecimal escape for the next input chunk.
pub(crate) fn complete_prefix(data: &[u8]) -> usize {
    let end = data.len();
    let mut digits = end;
    while digits > end.saturating_sub(8) && data[digits - 1].is_ascii_hexdigit() {
        digits -= 1;
    }
    let marker = if data.last() == Some(&b'\\') {
        end
    } else if digits > 0 && matches!(data[digits - 1], b'u' | b'U') {
        let width = if data[digits - 1] == b'u' { 4 } else { 8 };
        if end - digits >= width {
            return end;
        }
        digits - 1
    } else {
        return end;
    };
    let count = data[..marker]
        .iter()
        .rev()
        .take_while(|&&c| c == b'\\')
        .count();
    if count % 2 == 1 { marker - 1 } else { end }
}

/// Append every complete code point preceding the first invalid escape.
pub(crate) fn prefix(
    points: &mut Vec<u32>,
    data: &[u8],
    mut pos: usize,
) -> Result<Option<DecodeIssue>> {
    points
        .try_reserve(data.len() - pos)
        .map_err(|_| Error::allocation())?;
    while pos < data.len() {
        let start = pos;
        let byte = data[pos];
        pos += 1;
        if byte != b'\\' {
            points.push(u32::from(byte));
            continue;
        }
        let width = match data.get(pos) {
            Some(b'\\') => {
                points.extend([u32::from(b'\\'); 2]);
                pos += 1;
                continue;
            }
            Some(b'u') => 4,
            Some(b'U') => 8,
            _ => {
                points.push(u32::from(byte));
                continue;
            }
        };
        pos += 1;
        let digits = pos;
        let mut point = 0;
        while pos - digits < width {
            let Some(n) = data.get(pos).and_then(|&b| char::from(b).to_digit(16)) else {
                break;
            };
            point = (point << 4) | n;
            pos += 1;
        }
        let reason = if pos - digits < width {
            if width == 4 {
                r"truncated \uXXXX escape"
            } else {
                r"truncated \UXXXXXXXX escape"
            }
        } else if point > 0x10ffff {
            r"\Uxxxxxxxx out of range"
        } else {
            points.push(point);
            continue;
        };
        return Ok(Some(DecodeIssue {
            encoding: "rawunicodeescape",
            start,
            end: pos,
            reason,
        }));
    }
    Ok(None)
}

/// Emit unescaped Latin-1 bytes and fixed-width lowercase hexadecimal escapes for other points.
pub(crate) fn encode(points: impl Iterator<Item = u32>) -> Result<Vec<u8>> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::new();
    for point in points {
        if let Ok(byte) = u8::try_from(point) {
            out.try_reserve(1).map_err(|_| Error::allocation())?;
            out.push(byte);
        } else {
            let width = if point <= 0xffff { 4 } else { 8 };
            out.try_reserve(width + 2)
                .map_err(|_| Error::allocation())?;
            out.extend([b'\\', if width == 4 { b'u' } else { b'U' }]);
            for index in (0..width).rev() {
                out.push(HEX[((point >> (index * 4)) & 0xf) as usize]);
            }
        }
    }
    Ok(out)
}
