//! Parse calendar or ISO week dates followed by an optional clock and UTC offset.

use super::*;

struct Parsed {
    date: [i32; 3],
    clock: [i32; 4],
    offset: Option<i64>,
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
    let year = iso_time::digits(bytes.get(..4)?)?;
    let suffix = &bytes[4..];
    let extended = suffix.starts_with(b"-");
    let suffix = suffix.strip_prefix(b"-").unwrap_or(suffix);
    if let Some(week) = suffix.strip_prefix(b"W") {
        let (week, day) = week.split_at_checked(2)?;
        let day = match (day, extended) {
            ([], _) => 1,
            ([b'-', d], true) | ([d], false) => i32::from(d.checked_sub(b'0')?),
            _ => return None,
        };
        return Calendar::iso_week_date(year, iso_time::digits(week)?, day);
    }
    let (month, day) = suffix.split_at_checked(2)?;
    let day = if extended {
        day.strip_prefix(b"-")?
    } else {
        day
    };
    if day.len() != 2 {
        return None;
    }
    Some([year, iso_time::digits(month)?, iso_time::digits(day)?])
}

fn parse(text: &str) -> Option<Parsed> {
    let split = date_length(text.as_bytes())?;
    let date = parse_date(text.as_bytes().get(..split)?)?;
    let tail = text.get(split..)?;
    let (clock, offset) = if let Some(separator) = tail.chars().next() {
        iso_time::parse_clock(&tail.as_bytes()[separator.len_utf8()..])?
    } else {
        ([0; 4], None)
    };
    Some(Parsed {
        date,
        clock,
        offset,
    })
}

/// Accept a lone surrogate at a potential date/time separator, as CPython does before UTF-8 encoding.
fn parse_surrogates(points: &[u32]) -> Result<Option<Parsed>> {
    let mut text = String::new();
    let capacity = points
        .len()
        .checked_mul(4)
        .ok_or_else(|| Error::new("MemoryError", ""))?;
    text.try_reserve_exact(capacity)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let mut replaced = false;
    for (i, &point) in points.iter().enumerate() {
        if let Some(ch) = char::from_u32(point) {
            text.push(ch);
        } else if !replaced && matches!(i, 7 | 8 | 10) {
            text.push('T');
            replaced = true;
        } else {
            return Ok(None);
        }
    }
    Ok(parse(&text))
}

pub(super) fn fromisoformat(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (class, args) = calendar_class_args(ctx, "datetime.datetime", "fromisoformat", args)?;
    let name = ctx.class_qualname(class);
    let input = builtins::one_positional_arg(&format!("{name}.fromisoformat"), args, kwargs)?;
    let Some(storage) = ctx.string_storage(input) else {
        return Err(Error::new(
            "TypeError",
            "fromisoformat: argument must be str",
        ));
    };
    let parsed = match ctx.heap.get(storage) {
        Value::Str(text) => parse(text),
        Value::SurrogateStr(points) => parse_surrogates(points)?,
        _ => unreachable!("validated string storage"),
    };
    let Some(Parsed {
        mut date,
        mut clock,
        offset,
    }) = parsed
    else {
        let repr = builtins::call(ctx, "repr", &[input], &[])?;
        if let Ok(text) = ctx.str_value(repr) {
            return Err(Error::new(
                "ValueError",
                format!("Invalid isoformat string: {text}"),
            ));
        }
        let prefix = ctx.text("Invalid isoformat string: ");
        let message = ctx.concat_strings(prefix, repr)?;
        return Err(ctx.lookup_error("ValueError", message));
    };
    let tz = iso_zone(ctx, offset)?;
    Calendar::iso_midnight(&mut date, &mut clock)?;
    let [y, m, d] = date;
    let [h, min, sec, us] = clock;
    datetime_replace_fields(ctx, class, [y, m, d, h, min, sec, us, 0], tz)
}
