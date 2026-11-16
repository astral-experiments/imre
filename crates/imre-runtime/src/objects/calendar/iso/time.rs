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

/// Two ASCII digits at fixed positions in a civil field.
pub(super) fn pair(a: u8, b: u8) -> Option<i32> {
    let (a, b) = (a.wrapping_sub(b'0'), b.wrapping_sub(b'0'));
    (a < 10 && b < 10).then_some(i32::from(a) * 10 + i32::from(b))
}

/// Parse `HH[:MM[:SS[.ffffff]]]` or its compact form. CPython also accepts a
/// compact fraction without a decimal mark when at least two digits follow SS.
fn components(mut bytes: &[u8], before_zone: bool) -> Option<[i32; 4]> {
    let (mut fields, fraction) = match bytes {
        [a, b, b':', c, d, b':', e, f, tail @ ..] => {
            let fields = [pair(*a, *b)?, pair(*c, *d)?, pair(*e, *f)?, 0];
            match tail {
                [] => return Some(fields),
                [_] => return (before_zone || tail == b"\0").then_some(fields),
                [b'.' | b',', fraction @ ..] => (fields, fraction),
                _ => return None,
            }
        }
        _ => {
            let mut fields = [0; 4];
            let separated = bytes.get(2) == Some(&b':');
            for (i, field) in fields[..3].iter_mut().enumerate() {
                let ([a, b], rest) = bytes.split_first_chunk::<2>()?;
                *field = pair(*a, *b)?;
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

            (fields, bytes)
        }
    };

    let precision = fraction.len().min(6);
    fields[3] = digits(&fraction[..precision])?
        * [1_000_000, 100_000, 10_000, 1_000, 100, 10, 1][precision];
    let trailing = fraction[precision..].iter().find(|b| !b.is_ascii_digit());
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
