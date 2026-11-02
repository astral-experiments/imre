//! Byte escapes consumed by codecs.escape_decode and protocol-0 pickle strings.
use super::{Context, Error, Id, NONE, Result, Value};
use std::borrow::Cow;

pub(super) fn decode(ctx: &mut Context, input: Id, errors: Option<Id>) -> Result<Id> {
    let storage = ctx.scalar_value(input);
    let data = if matches!(ctx.heap.get(storage), Value::SurrogateStr(_)) {
        Cow::Owned(super::encode_text(ctx, input, None, None)?)
    } else {
        Cow::Borrowed(match ctx.heap.get(storage) {
            Value::Str(text) => text.as_bytes(),
            Value::Array(array) => array.as_bytes(),
            _ => ctx.bytes_like(input)?,
        })
    };
    let consumed = data.len();
    let errors = match errors {
        None | Some(NONE) => "strict",
        Some(id) if ctx.string_storage(id).is_some() => super::error_name(ctx, Some(id))?,
        Some(id) => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "escape_decode() argument 2 must be str or None, not {}",
                    ctx.type_name(id)
                ),
            ));
        }
    };
    let (output, warning) = unescape(&data, errors)?;
    if let Some(escape) = warning {
        let detail = if escape > 255 {
            format!(r#"b"\{escape:o}" is an invalid octal escape sequence."#)
        } else {
            format!(
                r#"b"\{}" is an invalid escape sequence."#,
                char::from(escape as u8)
            )
        };
        ctx.emit_builtin_warning(
            crate::objects::exceptions::Kind::DeprecationWarning,
            &format!("{detail} Such sequences will not work in the future. "),
            1,
        )?;
    }
    let bytes = ctx.alloc(Value::Bytes(output));
    let consumed = ctx.int(consumed as i64);
    Ok(ctx.alloc(Value::Tuple(vec![bytes, consumed])))
}

/// Defer the first invalid-escape warning until the entire input has decoded.
fn unescape(data: &[u8], errors: &str) -> Result<(Vec<u8>, Option<u16>)> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(data.len())
        .map_err(|_| crate::Error::allocation())?;
    let mut warning = None;
    let mut offset = 0;
    while let Some(&byte) = data.get(offset) {
        offset += 1;
        if byte != b'\\' {
            output.push(byte);
            continue;
        }
        let Some(&code) = data.get(offset) else {
            return Err(Error::new("ValueError", r"Trailing \ in string"));
        };
        offset += 1;
        let value = match code {
            b'\n' => continue,
            b'\\' | b'\'' | b'"' => code,
            b'a' => 7,
            b'b' => 8,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 11,
            b'f' => 12,
            b'r' => b'\r',
            b'0'..=b'7' => {
                let mut octal = u16::from(code - b'0');
                for _ in 0..2 {
                    let Some(&digit @ b'0'..=b'7') = data.get(offset) else {
                        break;
                    };
                    octal = (octal << 3) + u16::from(digit - b'0');
                    offset += 1;
                }
                if octal > 255 {
                    warning.get_or_insert(octal);
                }
                octal as u8
            }
            b'x' => {
                let start = offset - 2;
                let mut hex = 0;
                while offset - start < 4 {
                    let Some(digit) = data.get(offset).and_then(|&b| char::from(b).to_digit(16))
                    else {
                        break;
                    };
                    hex = (hex << 4) + digit as u8;
                    offset += 1;
                }
                if offset - start == 4 {
                    hex
                } else {
                    match errors {
                        "strict" => {
                            return Err(Error::new(
                                "ValueError",
                                format!(r"invalid \x escape at position {start}"),
                            ));
                        }
                        "ignore" => continue,
                        "replace" => b'?',
                        _ => {
                            return Err(Error::new(
                                "ValueError",
                                format!(
                                    "decoding error; unknown error handling code: {errors:.400}"
                                ),
                            ));
                        }
                    }
                }
            }
            _ => {
                warning.get_or_insert(u16::from(code));
                output.push(b'\\');
                code
            }
        };
        output.push(value);
    }
    Ok((output, warning))
}
