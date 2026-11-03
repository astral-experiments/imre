//! CPython 3.14.5 Shift JIS (JIS X 0201 and JIS X 0208).
use super::{DecodeIssue, Error, Result};

const COLUMNS: usize = 188;
const UNMAPPED: u16 = 0xfffe;
// Generated with scripts/generate-shift-jis.py. The rows and columns omit byte-range gaps.
const TABLE: &[u8; 42 * COLUMNS * 2] = include_bytes!("shift_jis.bin");

// Dense pages cover only the Unicode high bytes present in the decode table.
const PAGE_INDEX: ([u8; 256], usize) = {
    let mut pages = [u8::MAX; 256];
    let mut count = 0;
    let mut i = 0;
    while i < TABLE.len() {
        let scalar = u16::from_le_bytes([TABLE[i], TABLE[i + 1]]);
        let page = TABLE[i + 1] as usize;
        if scalar != UNMAPPED && pages[page] == u8::MAX {
            pages[page] = count as u8;
            count += 1;
        }
        i += 2;
    }
    (pages, count)
};

static ENCODE: [[[u8; 2]; 256]; PAGE_INDEX.1] = {
    let mut pages = [[[0; 2]; 256]; PAGE_INDEX.1];
    let mut i = 0;
    while i < TABLE.len() / 2 {
        let low = TABLE[2 * i];
        let high = TABLE[2 * i + 1];
        if u16::from_le_bytes([low, high]) != UNMAPPED {
            let row = (i / COLUMNS) as u8;
            let col = (i % COLUMNS) as u8;
            let lead = if row < 31 { 0x81 + row } else { 0xc1 + row };
            let trail = if col < 63 { 0x40 + col } else { 0x41 + col };
            pages[PAGE_INDEX.0[high as usize] as usize][low as usize] = [lead, trail];
        }
        i += 1;
    }
    pages
};

/// Return a complete one- or two-byte character, including CPython's two encode-only aliases.
pub(crate) fn encode_char(ch: char) -> Option<([u8; 2], usize)> {
    let byte = match ch {
        '\0'..='\u{7f}' => ch as u8,
        '\u{a5}' => 0x5c,
        '\u{203e}' => 0x7e,
        '\u{ff61}'..='\u{ff9f}' => 0xa1 + (u32::from(ch) - 0xff61) as u8,
        _ => {
            let [low, high] = u16::try_from(u32::from(ch)).ok()?.to_le_bytes();
            let page = ENCODE.get(usize::from(PAGE_INDEX.0[usize::from(high)]))?;
            let pair = page[usize::from(low)];
            return (pair[0] != 0).then_some((pair, 2));
        }
    };
    Some(([byte, 0], 1))
}

/// Decode at a character boundary. Invalid pairs consume only the leading byte.
fn character(data: &[u8], start: usize) -> std::result::Result<(char, usize), DecodeIssue> {
    let lead = data[start];
    let error = |reason| DecodeIssue {
        encoding: "shift_jis",
        start,
        end: start + 1,
        reason,
    };
    let row = match lead {
        0..=0x7f => return Ok((char::from(lead), 1)),
        0xa1..=0xdf => return Ok((char::from_u32(0xff61 + u32::from(lead - 0xa1)).unwrap(), 1)),
        0x81..=0x9f => lead - 0x81,
        0xe0..=0xea => lead - 0xc1,
        _ => return Err(error("illegal multibyte sequence")),
    };
    let trail = data
        .get(start + 1)
        .ok_or_else(|| error("incomplete multibyte sequence"))?;
    let col = match trail {
        0x40..=0x7e => trail - 0x40,
        0x80..=0xfc => trail - 0x41,
        _ => return Err(error("illegal multibyte sequence")),
    };
    let at = 2 * (usize::from(row) * COLUMNS + usize::from(col));
    let scalar = u16::from_le_bytes([TABLE[at], TABLE[at + 1]]);
    if scalar == UNMAPPED {
        return Err(error("illegal multibyte sequence"));
    }
    Ok((char::from_u32(u32::from(scalar)).unwrap(), 2))
}

/// Append complete characters until an error or an incomplete, non-final lead.
pub(crate) fn decode_prefix(
    text: &mut String,
    data: &[u8],
    mut offset: usize,
    final_: bool,
) -> Result<(usize, Option<DecodeIssue>)> {
    while offset < data.len() {
        match character(data, offset) {
            Ok((ch, size)) => {
                text.try_reserve(ch.len_utf8())
                    .map_err(|_| Error::allocation())?;
                text.push(ch);
                offset += size;
            }
            Err(issue) if !final_ && issue.reason == "incomplete multibyte sequence" => break,
            Err(issue) => return Ok((offset, Some(issue))),
        }
    }
    Ok((offset, None))
}

/// Decode a whole caller buffer, preserving it in error objects even when its final lead is pending.
pub(crate) fn decode_chunk(
    text: &mut String,
    data: &[u8],
    errors: Option<&str>,
    final_: bool,
) -> Result<usize> {
    let mut offset = 0;
    loop {
        let (consumed, issue) = decode_prefix(text, data, offset, final_)?;
        let Some(issue) = issue else {
            return Ok(consumed);
        };
        super::decoding_error(text, errors, &data[issue.start..issue.end], || {
            issue.error(data)
        })?;
        offset = issue.end;
    }
}

/// POSIX filesystem decoding preserves every unmapped byte as a low surrogate.
pub(super) fn decode_filesystem(data: &[u8], points: &mut Vec<u32>) {
    let mut offset = 0;
    while offset < data.len() {
        match character(data, offset) {
            Ok((ch, size)) => {
                points.push(u32::from(ch));
                offset += size;
            }
            Err(issue) => {
                points.push(0xdc00 + u32::from(data[offset]));
                offset = issue.end;
            }
        }
    }
}
