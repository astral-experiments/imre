use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use std::borrow::Cow;

use ruff_python_literal::cformat::{CFormatPart, CFormatString};
use ruff_python_literal::format::{
    FieldName, FieldNamePart, FieldType, FormatPart, FormatString, FromTemplate,
};

use super::*;

use crate::objects::{python_whitespace, split_parts, visit_line_fields as split_lines};

fn integer(source: &str, mut base: u32) -> Result<crate::Int> {
    let original = source;
    let original_base = base;
    let error = || {
        Error::from(crate::integer_text::invalid_literal_in_base(
            original,
            original_base,
        ))
    };
    let source = source.trim();
    let source = if source.is_ascii() {
        Cow::Borrowed(source)
    } else {
        Cow::Owned(
            source
                .chars()
                .map(|ch| {
                    crate::integer_text::decimal_digit(ch)
                        .map_or(ch, |digit| char::from(b'0' + digit))
                })
                .collect(),
        )
    };
    let source = source.as_ref();
    let (negative, digits) = match source.as_bytes().first() {
        Some(b'-') => (true, &source[1..]),
        Some(b'+') => (false, &source[1..]),
        _ => (false, source),
    };
    let radix_prefix = digits.as_bytes().get(..2).and_then(|v| match v {
        b"0b" | b"0B" => Some(2),
        b"0o" | b"0O" => Some(8),
        b"0x" | b"0X" => Some(16),
        _ => None,
    });
    let mut digits = digits;
    let guessed = base == 0;
    let prefixed = radix_prefix.is_some_and(|r| base == 0 || r == base);
    if prefixed {
        base = radix_prefix.unwrap();
        digits = digits[2..].strip_prefix('_').unwrap_or(&digits[2..]);
    } else if guessed {
        base = 10;
    }
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|ch| ch == '_' || ch.is_digit(base))
    {
        return Err(error());
    }
    let normalized = digits.replace('_', "");
    let value = BigInt::parse_bytes(normalized.as_bytes(), base).ok_or_else(error)?;
    if guessed && !prefixed && digits.starts_with('0') && !value.is_zero() {
        return Err(error());
    }
    Ok(crate::Int::new(if negative { -value } else { value }))
}

fn encoding(ctx: &Context, id: Option<Id>) -> Result<bool> {
    let name = id
        .map(|x| ctx.str_value(x))
        .transpose()?
        .unwrap_or("utf-8")
        .to_ascii_lowercase()
        .replace('_', "-");
    match name.as_str() {
        "utf8" | "utf-8" | "u8" => Ok(false),
        "ascii" | "us-ascii" => Ok(true),
        _ => Err(Error::new(
            "LookupError",
            format!("unknown encoding: {name}"),
        )),
    }
}

fn strict(ctx: &Context, id: Option<Id>) -> Result<()> {
    if let Some(id) = id
        && ctx.str_value(id)? != "strict"
    {
        return Err(Error::new(
            "NotImplementedError",
            "only strict codec error handling is supported",
        ));
    }
    Ok(())
}

fn decode(ctx: &Context, id: Id, encoding_id: Option<Id>, errors: Option<Id>) -> Result<String> {
    let Value::Bytes(bytes) = ctx.heap.get(id) else {
        return Err(type_error("decoding requires bytes"));
    };
    strict(ctx, errors)?;
    if encoding(ctx, encoding_id)? && !bytes.is_ascii() {
        return Err(Error::new("UnicodeDecodeError", "non-ASCII byte"));
    }
    String::from_utf8(bytes.clone()).map_err(|e| Error::new("UnicodeDecodeError", e.to_string()))
}

fn encode(ctx: &Context, id: Id, encoding_id: Option<Id>, errors: Option<Id>) -> Result<Vec<u8>> {
    let text = ctx.str_value(id)?;
    strict(ctx, errors)?;
    if encoding(ctx, encoding_id)? && !text.is_ascii() {
        return Err(Error::new("UnicodeEncodeError", "non-ASCII character"));
    }
    Ok(text.as_bytes().to_vec())
}

pub(super) fn construct(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match name {
        "bool" => {
            positional(name, kwargs)?;
            arity(name, args, 0, 1)?;
            Ok(boolean(args.first().is_some_and(|&id| ctx.truth(id))))
        }
        "repr" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            Ok(ctx.text(ctx.repr(args[0])))
        }
        "int" => {
            if kwargs.iter().any(|(k, _)| k != "base") {
                return Err(type_error("int accepts only the keyword base"));
            }
            let [value, base] = bind(name, args, kwargs, ["value", "base"])?;
            if value.is_none() && base.is_some() {
                return Err(type_error("int() missing string argument"));
            }
            let radix = base.map(|id| ctx.as_integer(id)).transpose()?.unwrap_or(10);
            if radix != 0 && !(2..=36).contains(&radix) {
                return Err(Error::new(
                    "ValueError",
                    "int() base must be >= 2 and <= 36, or 0",
                ));
            }
            let number = match value.map(|id| ctx.heap.get(id)) {
                None => crate::Int::from(0),
                Some(Value::Str(s)) => integer(s, radix as u32)?,
                Some(Value::Bytes(b)) if b.is_ascii() => {
                    integer(std::str::from_utf8(b).unwrap(), radix as u32)?
                }
                Some(Value::Bytes(_)) => {
                    return Err(Error::new("ValueError", "invalid byte literal for int()"));
                }
                _ if base.is_some() => {
                    return Err(type_error(
                        "int() can't convert non-string with explicit base",
                    ));
                }
                _ => ctx.as_int(value.unwrap())?,
            };
            Ok(ctx.alloc(Value::Int(number)))
        }
        "str" => {
            let [value, encoding, errors] =
                bind(name, args, kwargs, ["object", "encoding", "errors"])?;
            let rendered = if encoding.is_some() || errors.is_some() {
                decode(
                    ctx,
                    value.ok_or_else(|| type_error("decoding requires bytes"))?,
                    encoding,
                    errors,
                )?
            } else {
                value.map(|id| ctx.display(id)).unwrap_or_default()
            };
            Ok(ctx.text(rendered))
        }
        "bytes" => {
            let [value, encoding_id, errors] =
                bind(name, args, kwargs, ["source", "encoding", "errors"])?;
            let bytes = match value.map(|id| ctx.heap.get(id)) {
                Some(Value::Str(_)) => {
                    if encoding_id.is_none() {
                        return Err(type_error("string argument without an encoding"));
                    }
                    encode(ctx, value.unwrap(), encoding_id, errors)?
                }
                _ if encoding_id.is_some() || errors.is_some() => {
                    return Err(type_error("encoding without a string argument"));
                }
                None => Vec::new(),
                Some(Value::Bytes(_)) => return Ok(value.unwrap()),
                Some(Value::Int(_) | Value::Bool(_)) => {
                    let size = usize::try_from(ctx.as_integer(value.unwrap())?)
                        .map_err(|_| Error::new("ValueError", "negative count"))?;
                    let mut v = Vec::new();
                    v.try_reserve_exact(size)
                        .map_err(|_| Error::new("MemoryError", "bytes allocation is too large"))?;
                    v.resize(size, 0);
                    v
                }
                _ => ctx
                    .items(value.unwrap())?
                    .into_iter()
                    .map(|id| {
                        let number = ctx.as_integer(id)?;
                        u8::try_from(number)
                            .map_err(|_| Error::new("ValueError", "bytes must be in range(0, 256)"))
                    })
                    .collect::<Result<_>>()?,
            };
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        _ => Err(type_error(format!("unsupported type {name}"))),
    }
}

/// Apply a scalar format specifier used by both f-strings and str.format.
pub(in super::super) fn format_value(ctx: &mut Context, value: Id, spec: &str) -> Result<String> {
    if spec.is_empty() {
        return Ok(ctx.display(value));
    }
    if matches!(ctx.heap.get(value), Value::Str(_)) && spec == "s" {
        return Ok(ctx.display(value));
    }
    if matches!(ctx.heap.get(value), Value::Bool(_) | Value::Int(_)) {
        let int = ctx.as_int(value)?;
        let int = int.big();
        let (sign, spec) = match spec.as_bytes().first() {
            Some(b'+' | b' ' | b'-') => (&spec[..1], &spec[1..]),
            _ => ("", spec),
        };
        let digits = match spec {
            "" | "d" => int.magnitude().to_string(),
            "b" => format!("{:b}", int.magnitude()),
            "o" => format!("{:o}", int.magnitude()),
            "x" => format!("{:x}", int.magnitude()),
            "X" => format!("{:X}", int.magnitude()),
            _ => {
                return Err(Error::new(
                    "ValueError",
                    format!("unsupported integer format {spec:?}"),
                ));
            }
        };
        let sign = if int.is_negative() {
            "-"
        } else if sign == "-" {
            ""
        } else {
            sign
        };
        return Ok(format!("{sign}{digits}"));
    }
    Err(type_error(format!("unsupported format string {spec:?}")))
}

fn format_string(
    ctx: &mut Context,
    source: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<String> {
    #[derive(Default)]
    struct Indices {
        next: usize,
        manual: bool,
    }
    fn render(
        ctx: &mut Context,
        s: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
        indices: &mut Indices,
        nested: bool,
    ) -> Result<String> {
        let parsed =
            FormatString::from_raw_str(s).map_err(|e| Error::new("ValueError", e.to_string()))?;
        let mut output = String::new();
        for part in parsed.format_parts {
            let FormatPart::Field {
                field_name,
                conversion_spec,
                format_spec,
            } = part
            else {
                if let FormatPart::Literal(text) = part {
                    output.push_str(&text);
                }
                continue;
            };
            let field = FieldName::parse(&field_name)
                .map_err(|e| Error::new("ValueError", e.to_string()))?;
            let positional = |index| {
                args.get(index).copied().ok_or_else(|| {
                    Error::new(
                        "IndexError",
                        format!("replacement index {index} out of range"),
                    )
                })
            };
            let mixed = || {
                Error::new(
                    "ValueError",
                    "cannot mix automatic and manual field numbering",
                )
            };
            let mut value = match field.field_type {
                FieldType::Auto => {
                    if indices.manual {
                        return Err(mixed());
                    }
                    let index = indices.next;
                    indices.next += 1;
                    positional(index)?
                }
                FieldType::Index(index) => {
                    if indices.next != 0 {
                        return Err(mixed());
                    }
                    indices.manual = true;
                    positional(index)?
                }
                FieldType::Keyword(key) => kwargs
                    .iter()
                    .find(|(k, _)| k == &key)
                    .map(|(_, id)| *id)
                    .ok_or_else(|| Error::new("KeyError", key))?,
            };
            for access in field.parts {
                value = match access {
                    FieldNamePart::Attribute(name) => ctx.lookup_attr(value, &name)?,
                    FieldNamePart::Index(index) => {
                        let index = i64::try_from(index)
                            .map_err(|_| Error::new("IndexError", "index too large"))?;
                        let index = ctx.int(index);
                        ctx.get_item(value, index)?
                    }
                    FieldNamePart::StringIndex(key) => {
                        let key = ctx.text(key);
                        ctx.get_item(value, key)?
                    }
                }
            }
            if let Some(conv) = conversion_spec {
                let s = match conv {
                    's' => ctx.display(value),
                    'r' => ctx.repr(value),
                    'a' => ascii(&ctx.repr(value)),
                    _ => return Err(Error::new("ValueError", "unknown format conversion")),
                };
                value = ctx.text(s);
            }
            let spec = if format_spec.contains(['{', '}']) {
                if nested {
                    return Err(Error::new(
                        "ValueError",
                        "format specifier nested too deeply",
                    ));
                }
                render(ctx, &format_spec, args, kwargs, indices, true)?
            } else {
                format_spec
            };
            output.push_str(&format_value(ctx, value, &spec)?);
        }
        Ok(output)
    }
    render(ctx, source, args, kwargs, &mut Indices::default(), false)
}

fn ascii(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for ch in s.chars() {
        match ch as u32 {
            0..=0x7f => out.push(ch),
            n @ 0x80..=0xff => write!(out, "\\x{n:02x}").unwrap(),
            n @ 0x100..=0xffff => write!(out, "\\u{n:04x}").unwrap(),
            n => write!(out, "\\U{n:08x}").unwrap(),
        }
    }
    out
}

/// Old-style interpolation accepts a single operand, a positional tuple or a mapping.
pub(in super::super) fn percent(ctx: &mut Context, format: &str, rhs: Id) -> Result<Id> {
    let parsed = format
        .parse::<CFormatString>()
        .map_err(|e| Error::new("ValueError", e.to_string()))?;
    let mapping = matches!(
        ctx.heap.get(rhs),
        Value::Dict(_) | Value::DefaultDict { .. }
    );
    let values = match ctx.heap.get(rhs) {
        Value::Tuple(values) | Value::NamedTuple { values, .. } => values.clone(),
        Value::Instance { class, .. } if ctx.namedtuple_fields(*class).is_some() => {
            ctx.items(rhs)?
        }
        _ => vec![rhs],
    };
    let mut args = values.into_iter();
    let mut output = String::new();
    for (_, part) in parsed.iter() {
        match part {
            CFormatPart::Literal(literal) => output.push_str(literal),
            CFormatPart::Spec(spec) => {
                if spec.precision.is_some()
                    || spec.min_field_width.is_some()
                    || !spec.flags.is_empty()
                {
                    return Err(Error::new(
                        "ValueError",
                        "unsupported flags, width or precision for percent interpolation",
                    ));
                }
                let arg = if let Some(name) = &spec.mapping_key {
                    if !mapping {
                        return Err(type_error("format requires a mapping"));
                    }
                    // A mapping lookup consumes the single positional operand.
                    args.next();
                    let key = ctx.text(name);
                    ctx.get_item(rhs, key)?
                } else {
                    args.next()
                        .ok_or_else(|| type_error("not enough arguments for format string"))?
                };
                let rendered = match spec.format_char {
                    's' => ctx.display(arg),
                    'r' => ctx.repr(arg),
                    'a' => ascii(&ctx.repr(arg)),
                    'd' | 'i' | 'u' => ctx.as_int(arg)?.to_string(),
                    ch @ ('o' | 'x' | 'X') => format_value(ctx, arg, &ch.to_string())?,
                    'c' => match ctx.heap.get(arg) {
                        Value::Str(s) if s.chars().count() == 1 => s.clone(),
                        _ => u32::try_from(ctx.as_integer(arg)?)
                            .ok()
                            .and_then(char::from_u32)
                            .ok_or_else(|| Error::new("OverflowError", "%c arg not in range"))?
                            .to_string(),
                    },
                    _ => {
                        return Err(Error::new(
                            "ValueError",
                            format!("unsupported percent conversion {}", spec.format_char),
                        ));
                    }
                };
                output.push_str(&rendered);
            }
        }
    }
    if !mapping && args.next().is_some() {
        return Err(type_error(
            "not all arguments converted during string formatting",
        ));
    }
    Ok(ctx.text(output))
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&receiver, args) = args
        .split_first()
        .ok_or_else(|| type_error("missing string receiver"))?;
    if name == "decode" || name == "encode" {
        let [encoding, errors] = bind(name, args, kwargs, ["encoding", "errors"])?;
        return if name == "decode" {
            let s = decode(ctx, receiver, encoding, errors)?;
            Ok(ctx.text(s))
        } else {
            let b = encode(ctx, receiver, encoding, errors)?;
            Ok(ctx.alloc(Value::Bytes(b)))
        };
    }
    let source = ctx.str_value(receiver)?.to_owned();
    match name {
        "format" => {
            let result = format_string(ctx, &source, args, kwargs)?;
            Ok(ctx.text(result))
        }
        "split" | "rsplit" => {
            let [sep, max] = bind(name, args, kwargs, ["sep", "maxsplit"])?;
            let sep = sep
                .filter(|&x| x != NONE)
                .map(|x| ctx.str_value(x).map(str::to_owned))
                .transpose()?;
            let max = max.map(|x| ctx.as_integer(x)).transpose()?.unwrap_or(-1);
            let mut values = Vec::new();
            let reverse = name == "rsplit";
            split_parts(
                &source,
                sep.as_deref(),
                usize::try_from(max).unwrap_or(usize::MAX),
                reverse,
                |field| values.push(ctx.text(field)),
            )
            .map_err(|message| Error::new("ValueError", message))?;
            if reverse {
                values.reverse();
            }
            Ok(ctx.alloc(Value::List(values)))
        }
        "splitlines" => {
            let [keep] = bind(name, args, kwargs, ["keepends"])?;
            let keep = keep.map(|x| ctx.as_integer(x)).transpose()?.unwrap_or(0) != 0;
            let mut values = Vec::new();
            split_lines(&source, keep, |field| values.push(ctx.text(field)));
            Ok(ctx.alloc(Value::List(values)))
        }
        "strip" | "lstrip" | "rstrip" => {
            positional(name, kwargs)?;
            arity(name, args, 0, 1)?;
            let chars = args
                .first()
                .filter(|&&x| x != NONE)
                .map(|&x| ctx.str_value(x))
                .transpose()?;
            let matches = |c: char| chars.map_or_else(|| python_whitespace(c), |s| s.contains(c));
            let trimmed = match name {
                "strip" => source.trim_matches(matches),
                "rstrip" => source.trim_end_matches(matches),
                _ => source.trim_start_matches(matches),
            };
            Ok(ctx.text(trimmed))
        }
        "startswith" | "endswith" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 3)?;
            let length = source.chars().count() as i64;
            let bound = |i: i64| {
                if i < 0 {
                    length.saturating_add(i).max(0)
                } else {
                    i
                }
            };
            let start = args
                .get(1)
                .filter(|&&id| id != NONE)
                .map(|&x| ctx.as_integer(x))
                .transpose()?
                .map(bound)
                .unwrap_or(0);
            let end = args
                .get(2)
                .filter(|&&id| id != NONE)
                .map(|&x| ctx.as_integer(x))
                .transpose()?
                .map(bound)
                .unwrap_or(length)
                .min(length);
            let part: String = source
                .chars()
                .skip(start as usize)
                .take(end.saturating_sub(start).max(0) as usize)
                .collect();
            let candidates = match ctx.heap.get(args[0]) {
                Value::Tuple(values) => values.as_slice(),
                _ => &args[..1],
            };
            for &id in candidates {
                let needle = ctx.str_value(id)?;
                if start <= end
                    && (if name == "startswith" {
                        part.starts_with(needle)
                    } else {
                        part.ends_with(needle)
                    })
                {
                    return Ok(TRUE);
                }
            }
            Ok(FALSE)
        }
        "replace" => {
            positional(name, kwargs)?;
            arity(name, args, 2, 3)?;
            let count = args
                .get(2)
                .map(|&x| ctx.as_integer(x))
                .transpose()?
                .unwrap_or(-1);
            let old = ctx.str_value(args[0])?;
            let new = ctx.str_value(args[1])?;
            Ok(ctx.text(source.replacen(old, new, usize::try_from(count).unwrap_or(usize::MAX))))
        }
        "join" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            let values = ctx.items(args[0])?;
            let parts = values
                .iter()
                .map(|&id| ctx.str_value(id))
                .collect::<Result<Vec<_>>>()?;
            Ok(ctx.text(parts.join(&source)))
        }
        "lower" | "upper" => {
            positional(name, kwargs)?;
            arity(name, args, 0, 0)?;
            Ok(ctx.text(if name == "lower" {
                source.to_lowercase()
            } else {
                source.to_uppercase()
            }))
        }
        "removeprefix" | "removesuffix" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            let affix = ctx.str_value(args[0])?;
            let stripped = if name == "removeprefix" {
                source.strip_prefix(affix)
            } else {
                source.strip_suffix(affix)
            };
            Ok(ctx.text(stripped.unwrap_or(&source)))
        }
        _ => Err(type_error(format!("unsupported string method {name}"))),
    }
}
