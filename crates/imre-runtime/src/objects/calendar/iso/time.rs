//! Parse the clock and optional UTC offset accepted by CPython's `time.fromisoformat`.

pub(super) fn digits(bytes: &[u8]) -> Option<i32> {
    let mut value = 0;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + i32::from(byte - b'0');
    }
    Some(value)
}

/// Parse `HH[:MM[:SS[.ffffff]]]` or its compact form. CPython also accepts a
/// compact fraction without a decimal mark when at least two digits follow SS.
fn components(mut bytes: &[u8], before_zone: bool) -> Option<[i32; 4]> {
    let mut fields = [0; 4];
    let separated = bytes.get(2) == Some(&b':');
    for (i, field) in fields[..3].iter_mut().enumerate() {
        let (pair, rest) = bytes.split_at_checked(2)?;
        *field = digits(pair)?;
        bytes = rest;
        if bytes.len() <= 1 {
            // parse_hh_mm_ss_ff reports a final non-NUL byte as trailing data.
            // parse_isoformat_time ignores that status before a zone marker.
            return (before_zone || bytes.is_empty() || bytes == b"\0").then_some(fields);
        }
        if matches!(bytes[0], b'.' | b',') {
            if i != 2 {
                return None;
            }
            bytes = &bytes[1..];
            break;
        }
        if separated {
            if bytes[0] != b':' || i == 2 {
                return None;
            }
            bytes = &bytes[1..];
        }
    }

    let precision = bytes.len().min(6);
    fields[3] = digits(&bytes[..precision])? * 10_i32.pow(6 - precision as u32);
    let trailing = bytes[precision..].iter().find(|b| !b.is_ascii_digit());
    (before_zone || trailing.is_none_or(|&b| b == 0)).then_some(fields)
}

pub(crate) fn parse(text: &str) -> Option<([i32; 4], Option<i64>)> {
    let bytes = text.as_bytes();
    parse_clock(bytes.strip_prefix(b"T").unwrap_or(bytes))
}

/// Parse the clock payload after any caller-specific prefix or separator.
pub(super) fn parse_clock(text: &[u8]) -> Option<([i32; 4], Option<i64>)> {
    let boundary = text
        .iter()
        .position(|b| matches!(b, b'Z' | b'+' | b'-'))
        .unwrap_or(text.len());
    let (clock, zone) = text.split_at(boundary);
    let fields = components(clock, !zone.is_empty())?;
    let offset = match zone {
        [] => None,
        [b'Z', rest @ ..] if rest.is_empty() || rest[0] == 0 => Some(0),
        [sign @ (b'+' | b'-'), rest @ ..] => {
            let [hours, minutes, seconds, micros] = components(rest, false)?;
            let seconds = (hours * 60 + minutes) * 60 + seconds;
            // tzinfo_from_isoformat_results returns UTC for zero whole seconds.
            let offset = if seconds == 0 {
                0
            } else {
                i64::from(seconds) * 1_000_000 + i64::from(micros)
            };
            Some(if *sign == b'-' { -offset } else { offset })
        }
        _ => return None,
    };
    Some((fields, offset))
}
