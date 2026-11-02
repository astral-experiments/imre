use super::{Context, Error, Id, Result, Value};
use std::ops::Range;

/// Scan one string starting at a Python character offset in a larger document.
pub(in crate::dynamic::stdlib) fn scanstring(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((_, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "_json.scanstring requires a module receiver",
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "scanstring() takes no keyword arguments",
        ));
    }
    let count = args.len();
    if !(2..=3).contains(&count) {
        let expected = if count < 2 { "at least 2" } else { "at most 3" };
        return Err(Error::new(
            "TypeError",
            format!("scanstring() takes {expected} arguments ({count} given)"),
        ));
    }
    let end = ctx.index_integer(args[1])?.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })?;
    let strict = ctx.optional_truth(args.get(2).copied(), true)?;
    let storage = super::string_argument(ctx, args[0])?;
    scan_string(ctx, args[0], storage, end, strict)
}

fn scan_string(ctx: &mut Context, input: Id, storage: Id, start: i64, strict: bool) -> Result<Id> {
    let out_of_bounds = || Error::new("ValueError", "end is out of bounds");
    let at = usize::try_from(start).map_err(|_| out_of_bounds())?;
    let unterminated = "Unterminated string starting at";
    let (body, after) = match ctx.heap.get(storage) {
        Value::Str(text) => {
            let byte = text
                .char_indices()
                .map(|(i, _)| i)
                .chain([text.len()])
                .nth(at)
                .ok_or_else(out_of_bounds)?;
            let end = match crate::objects::json_string_end(
                text.as_bytes(),
                byte,
                byte.saturating_sub(1),
                strict,
            ) {
                Ok(end) => end,
                Err((offset, message)) => {
                    let position = if message == unterminated {
                        start - 1
                    } else {
                        text[..offset].chars().count() as i64
                    };
                    return Err(scan_error(ctx, input, position, message));
                }
            };
            let after = start + text[byte..end].chars().count() as i64;
            (byte..end - 1, after)
        }
        Value::SurrogateStr(points) => {
            if at > points.len() {
                return Err(out_of_bounds());
            }
            let after =
                match crate::objects::json_string_end(points, at, at.saturating_sub(1), strict) {
                    Ok(after) => after,
                    Err((offset, message)) => {
                        let pos = if message == unterminated {
                            start - 1
                        } else {
                            offset as i64
                        };
                        return Err(scan_error(ctx, input, pos, message));
                    }
                };
            (at..after - 1, after as i64)
        }
        _ => unreachable!("validated scanner string"),
    };
    let value = string_body(ctx, storage, body)?;
    let after = ctx.int(after);
    Ok(ctx.alloc(Value::Tuple(vec![value, after])))
}

/// Decode a validated span from the original string, including any raw Python surrogates.
fn string_body(ctx: &mut Context, storage: Id, body: Range<usize>) -> Result<Id> {
    let points = match ctx.heap.get(storage) {
        Value::Str(text) => {
            let text = &text[body];
            if !text.as_bytes().contains(&b'\\') {
                return Ok(ctx.text(crate::dynamic::copy_label(text)?));
            }
            decoded_points(text.chars().map(u32::from), text.len())?
        }
        Value::SurrogateStr(points) => {
            let points = &points[body];
            decoded_points(points.iter().copied(), points.len())?
        }
        _ => unreachable!("validated scanner string"),
    };
    Ok(ctx.text_codepoints(points))
}

pub(super) fn decoded_points<I: Iterator<Item = u32> + Clone>(
    body: I,
    bound: usize,
) -> std::result::Result<Vec<u32>, crate::Error> {
    let mut points = Vec::new();
    points
        .try_reserve_exact(bound)
        .map_err(|_| crate::Error::allocation())?;
    points.extend(unescape_points(body));
    Ok(points)
}

/// Resolve the active decoder's exception just as the accelerator does; a fresh
/// import may install a distinct JSONDecodeError class.
pub(super) fn scan_error(ctx: &mut Context, input: Id, pos: i64, message: &str) -> Error {
    let result = (|| {
        let decoder = ctx.module("json.decoder")?;
        let class = ctx.lookup_attr(decoder, "JSONDecodeError")?;
        let message = ctx.text(message);
        let pos = ctx.int(pos);
        let instance = ctx.call(class, vec![message, input, pos], Vec::new())?;
        let base = ctx.builtin("BaseException");
        if !ctx.subclass(class, base) {
            return Err(Error::new(
                "SystemError",
                format!(
                    "_PyErr_SetObject: exception {} is not a BaseException subclass",
                    ctx.repr(class)
                ),
            ));
        }
        Ok(instance)
    })();
    match result {
        Ok(id) => match ctx.heap.get(id) {
            Value::Raised(error) => {
                let mut error = error.clone();
                error.tracing().instance = Some(id);
                error
            }
            _ => Error::new("TypeError", "exceptions must derive from BaseException"),
        },
        Err(error) => error,
    }
}

/// Decode already validated JSON escapes, joining only adjacent escaped surrogate pairs.
fn unescape_points<I: Iterator<Item = u32> + Clone>(mut input: I) -> impl Iterator<Item = u32> {
    fn hex(input: &mut impl Iterator<Item = u32>) -> u32 {
        input.take(4).fold(0, |n, p| {
            n * 16 + char::from_u32(p).unwrap().to_digit(16).unwrap()
        })
    }

    std::iter::from_fn(move || {
        let point = input.next()?;
        if point != u32::from(b'\\') {
            return Some(point);
        }
        let point = match input.next().expect("validated JSON escape") as u8 {
            b'"' => u32::from(b'"'),
            b'\\' => u32::from(b'\\'),
            b'/' => u32::from(b'/'),
            b'b' => 8,
            b'f' => 12,
            b'n' => 10,
            b'r' => 13,
            b't' => 9,
            b'u' => {
                let first = hex(&mut input);
                if (0xd800..=0xdbff).contains(&first) {
                    let mut look = input.clone();
                    if look.next() == Some(u32::from(b'\\')) && look.next() == Some(u32::from(b'u'))
                    {
                        let second = hex(&mut look);
                        if (0xdc00..=0xdfff).contains(&second) {
                            input = look;
                            return Some(0x10000 + ((first - 0xd800) << 10) + second - 0xdc00);
                        }
                    }
                }
                first
            }
            _ => unreachable!("validated JSON escape"),
        };
        Some(point)
    })
}
