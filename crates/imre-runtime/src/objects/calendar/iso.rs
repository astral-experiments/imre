//! Parse calendar or ISO week dates followed by an optional clock and UTC offset.

use super::{boxed::Calendar, *};
use crate::objects::{exceptions::write_string_repr, values::ReprWriter};
mod time;
pub(crate) use time::parse as parse_time;

pub(crate) struct Parsed {
    pub(crate) date: [i32; 3],
    pub(crate) clock: [i32; 4],
    pub(crate) offset: Option<i64>,
}

/// An ISO week date can omit its weekday; even an ASCII digit can separate the clock.
fn date_length(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 7 {
        return None;
    }
    if bytes.len() == 7 {
        return Some(7);
    }
    if bytes[4] == b'-' {
        if bytes[5] != b'W' {
            return Some(10);
        }
        if bytes.get(8) != Some(&b'-') {
            return Some(8);
        }
        return match bytes.len() {
            9 => None,
            _ if bytes.get(10).is_some_and(u8::is_ascii_digit) => Some(8),
            _ => Some(10),
        };
    }
    if bytes[4] != b'W' {
        return Some(8);
    }
    let end = 7 + bytes[7..].iter().take_while(|b| b.is_ascii_digit()).count();
    Some(if end < 9 {
        end
    } else if end % 2 == 0 {
        7
    } else {
        8
    })
}

fn parse_date(bytes: &[u8]) -> Option<[i32; 3]> {
    let ([a, b, c, d], suffix) = bytes.split_first_chunk::<4>()?;
    let year = time::pair(*a, *b)? * 100 + time::pair(*c, *d)?;
    match suffix {
        [b'W', w, x] | [b'-', b'W', w, x] => Calendar::iso_week_date(year, time::pair(*w, *x)?, 1),
        [b'W', w, x, d] | [b'-', b'W', w, x, b'-', d] => {
            Calendar::iso_week_date(year, time::pair(*w, *x)?, i32::from(d.checked_sub(b'0')?))
        }
        [m, n, d, e] | [b'-', m, n, b'-', d, e] => {
            Some([year, time::pair(*m, *n)?, time::pair(*d, *e)?])
        }
        _ => None,
    }
}

pub(crate) fn parse(text: &str) -> Option<Parsed> {
    let split = date_length(text.as_bytes())?;
    let date = parse_date(text.as_bytes().get(..split)?)?;
    let tail = text.get(split..)?;
    let (clock, offset) = if let Some(separator) = tail.chars().next() {
        time::parse_clock(&tail.as_bytes()[separator.len_utf8()..])?
    } else {
        ([0; 4], None)
    };
    Some(Parsed {
        date,
        clock,
        offset,
    })
}

/// CPython gates exact UTF-8 byte lengths, then consumes the fixed-size fields.
/// In a ten-byte input a compact date therefore leaves two trailing bytes unread.
pub(crate) fn parse_date_only(text: &str) -> Option<[i32; 3]> {
    let bytes = text.as_bytes();
    match bytes.len() {
        7 | 8 => parse_date(bytes),
        10 if bytes[4] == b'-' => parse_date(bytes),
        10 => parse_date(&bytes[..8]),
        _ => None,
    }
}

/// # Safety
/// The caller roots an exact immutable string for the duration of the parse.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_datetime_fromisoformat(text: *const Str) -> *const Object {
    // SAFETY: the checked call keeps the string alive until the result is returned.
    let text = unsafe { &*text };
    rc_result(datetime(text))
}

fn datetime(text: &str) -> Result<crate::ObjectOwner, Error> {
    let Some(Parsed {
        mut date,
        mut clock,
        offset,
    }) = parse(text)
    else {
        let mut message = String::from("Invalid isoformat string: ");
        write_string_repr(text, &mut ReprWriter(&mut message)).map_err(|_| Error::allocation())?;
        return Err(Error::new(Kind::ValueError, message));
    };
    let zone = match offset {
        None => None,
        Some(0) => Some(utc()),
        Some(us) => Some(make_zone(try_owner(Value::delta(i128::from(us)))?, None)?),
    };
    Calendar::iso_midnight(&mut date, &mut clock)?;
    let local = Civil::from_fields(date, clock)?;
    try_owner(Value::Date {
        local,
        zone,
        fold: 0,
    })
}
