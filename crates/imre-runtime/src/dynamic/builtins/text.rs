use num_traits::Signed;

use ruff_python_literal::cformat::{
    CFormatContext, CFormatError, CFormatErrorType, CFormatPart, CFormatPrecision, CFormatQuantity,
    CFormatSpec, CFormatString,
};
use ruff_python_literal::format::{
    FieldName, FieldNamePart, FieldType, FormatPart, FormatString, FromTemplate,
};

use super::*;

use crate::objects::{python_whitespace, split_parts, translate, visit_line_fields as split_lines};

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
    let encoding = encoding_id
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("utf-8");
    Ok(crate::objects::Codec::lookup(encoding)?.decode(bytes, None)?)
}

fn encode(ctx: &Context, id: Id, encoding_id: Option<Id>, errors: Option<Id>) -> Result<Vec<u8>> {
    let text = ctx.str_value(id)?;
    let encoding = encoding_id
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("utf-8");
    let errors = errors
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("strict");
    Ok(crate::objects::Codec::encode_str(text, encoding, errors)?)
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
            Ok(boolean(ctx.optional_truth(args.first().copied(), false)?))
        }
        "repr" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            if let Value::Instance { class, .. } = *ctx.heap.get(args[0])
                && ctx.enum_values(class).is_some()
                && let Some(repr) = ctx.bound_slot(args[0], "__repr__")?
            {
                return ctx.call(repr, Vec::new(), Vec::new());
            }
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
                Some(Value::Str(s)) => crate::integer_text::parse_radix(s, radix as u32)?,
                Some(Value::Bytes(b)) if b.is_ascii() => {
                    crate::integer_text::parse_radix(std::str::from_utf8(b).unwrap(), radix as u32)?
                }
                Some(Value::Bytes(_)) => {
                    return Err(Error::new("ValueError", "invalid byte literal for int()"));
                }
                _ if base.is_some() => {
                    return Err(type_error(
                        "int() can't convert non-string with explicit base",
                    ));
                }
                Some(Value::Float(f)) => crate::Int::new(crate::floats::float_to_integer(*f)?),
                _ => ctx.as_int(value.unwrap())?,
            };
            Ok(ctx.alloc(Value::Int(number)))
        }
        "str" => {
            let [value, encoding, errors] =
                bind(name, args, kwargs, ["object", "encoding", "errors"])?;
            for (name, argument) in [("encoding", encoding), ("errors", errors)] {
                if let Some(argument) = argument
                    && ctx.string_storage(argument).is_none()
                {
                    return Err(type_error(format!(
                        "str() argument '{name}' must be str, not {}",
                        ctx.type_name(argument)
                    )));
                }
            }
            let Some(value) = value else {
                return Ok(ctx.text(""));
            };
            if encoding.is_some() || errors.is_some() {
                let decoded = decode(ctx, value, encoding, errors)?;
                return Ok(ctx.text(decoded));
            }
            if matches!(ctx.heap.get(value), Value::Str(_)) {
                return Ok(value);
            }
            let (name, method) = match ctx.bound_slot(value, "__str__")? {
                Some(method) => ("__str__", Some(method)),
                None if ctx.string_storage(value).is_none() => {
                    ("__repr__", ctx.bound_slot(value, "__repr__")?)
                }
                None => ("__str__", None),
            };
            if let Some(method) = method {
                let result = ctx.call(method, Vec::new(), Vec::new())?;
                if ctx.string_storage(result).is_none() {
                    return Err(type_error(format!(
                        "{name} returned non-string (type {})",
                        ctx.type_name(result)
                    )));
                }
                return Ok(result);
            }
            Ok(ctx.text(ctx.display(value)))
        }
        "bytes" | "bytearray" => {
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
                Some(Value::Bytes(_)) if name == "bytes" => return Ok(value.unwrap()),
                Some(Value::Int(_) | Value::Bool(_)) => {
                    zeroed_bytes(ctx.as_integer(value.unwrap())?)?
                }
                _ => bytes_from_iterable(ctx, value.unwrap())?,
            };
            Ok(ctx.alloc(if name == "bytes" {
                Value::Bytes(bytes)
            } else {
                Value::ByteArray(bytes)
            }))
        }
        _ => Err(type_error(format!("unsupported type {name}"))),
    }
}

pub(in super::super) fn bytes_from_iterable(ctx: &mut Context, obj: Id) -> Result<Vec<u8>> {
    if let Value::Bytes(data) | Value::ByteArray(data) = ctx.heap.get(obj) {
        return Ok(data.clone());
    }
    ctx.items(obj)?
        .into_iter()
        .map(|id| {
            u8::try_from(ctx.as_integer(id)?)
                .map_err(|_| Error::new("ValueError", "bytes must be in range(0, 256)"))
        })
        .collect()
}

pub(in super::super) fn zeroed_bytes(size: i64) -> Result<Vec<u8>> {
    let size = usize::try_from(size).map_err(|_| Error::new("ValueError", "negative count"))?;
    let mut data = Vec::new();
    data.try_reserve_exact(size)
        .map_err(|_| Error::new("MemoryError", "bytes allocation is too large"))?;
    data.resize(size, 0);
    Ok(data)
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

fn percent_args(ctx: &mut Context, rhs: Id) -> Result<std::vec::IntoIter<Id>> {
    let values = match ctx.heap.get(rhs) {
        Value::Tuple(values) | Value::NamedTuple { values, .. } => values.clone(),
        Value::Instance { class, .. } if ctx.namedtuple_fields(*class).is_some() => {
            ctx.items(rhs)?
        }
        _ => vec![rhs],
    };
    Ok(values.into_iter())
}

/// CPython reads a format character as unsigned immediately after decimal digits.
/// Elsewhere it promotes the platform's C char when constructing the diagnostic.
fn negative_format_char(prefix: &[u8], ch: char) -> bool {
    let after_quantity = prefix.last().is_some_and(u8::is_ascii_digit)
        && prefix.iter().any(|b| matches!(b, b'.' | b'1'..=b'9'));
    !after_quantity && i32::from(ch as u8 as std::ffi::c_char).is_negative()
}

/// Decimal conversions accept real numbers. Octal and hex require the integer-index protocol.
fn render_percent_integer(
    ctx: &mut Context,
    value: Id,
    spec: &CFormatSpec,
    field: &str,
) -> Result<String> {
    let conversion = spec.format_char;
    let decimal = matches!(conversion, 'd' | 'i' | 'u');
    let mut convert = || {
        if decimal {
            if let Ok(integer) = ctx.as_int(value) {
                return Ok(integer);
            }
            if let Value::Float(number) = ctx.heap.get(value) {
                return Ok(crate::Int::new(crate::floats::float_to_integer(*number)?));
            }
            if let Some(method) = ctx.bound_slot(value, "__int__")? {
                let result = ctx.call(method, Vec::new(), Vec::new())?;
                return ctx.as_int(result);
            }
        }
        ctx.index_integer(value)
    };
    let integer = convert().map_err(|error| {
        if error.kind == "TypeError" {
            let required = if decimal {
                "a real number"
            } else {
                "an integer"
            };
            type_error(format!(
                "%{conversion} format: {required} is required, not {}",
                ctx.type_name(value)
            ))
        } else {
            error
        }
    })?;
    // CPython leaves room for an integer sign and a two-character base prefix.
    if let Some(CFormatPrecision::Quantity(CFormatQuantity::Amount(precision))) = spec.precision
        && precision > (i32::MAX - 3) as usize
    {
        return Err(Error::new("OverflowError", "precision too large"));
    }
    crate::objects::percent_integer_field(field, integer).map_err(|e| {
        let error = Error::from(e);
        if error.kind == "MemoryError" {
            Error::new("MemoryError", "")
        } else {
            error
        }
    })
}

/// Format positional octal fields while preserving all literal bytes.
pub(in super::super) fn bytes_percent(ctx: &mut Context, format: &[u8], rhs: Id) -> Result<Id> {
    let mut args = percent_args(ctx, rhs)?;
    let mut input = format.iter().copied().enumerate().peekable();
    let mut output = Vec::new();
    let memory = |_| Error::new("MemoryError", "");
    output.try_reserve(format.len()).map_err(memory)?;
    while let Some((start, byte)) = input.next() {
        if byte != b'%' || input.next_if(|&(_, next)| next == b'%').is_some() {
            output.push(byte);
            continue;
        }
        let mut parsed = CFormatSpec::parse(&mut input, CFormatContext::Bytes);
        // Ruff limits both quantities to i32. CPython's bytes width fits Py_ssize_t.
        if let Err((CFormatErrorType::IntTooBig, index)) = parsed
            && !format[start..index].contains(&b'.')
        {
            let digits = format[..index]
                .iter()
                .rposition(|b| !b.is_ascii_digit())
                .unwrap()
                + 1;
            while input.next_if(|(_, b)| b.is_ascii_digit()).is_some() {}
            let tail = input.peek().map_or(format.len(), |&(i, _)| i);
            let width = std::str::from_utf8(&format[digits..tail]).unwrap();
            if width.parse::<isize>().is_ok() {
                // Validate the suffix with the same parser. The renderer uses the original width.
                let mut suffix = std::iter::once(b'1')
                    .chain(format[tail..].iter().copied())
                    .enumerate()
                    .peekable();
                parsed = CFormatSpec::parse(&mut suffix, CFormatContext::Bytes)
                    .map_err(|(kind, i)| (kind, tail - 1 + i));
                let end = suffix.peek().map_or(format.len(), |&(i, _)| tail - 1 + i);
                while input.next_if(|&(i, _)| i < end).is_some() {}
            }
        }
        let spec = parsed.map_err(|(typ, index)| match typ {
            CFormatErrorType::IntTooBig => {
                let message = if format[start..index].contains(&b'.') {
                    "prec too big"
                } else {
                    "width too big"
                };
                Error::new("ValueError", message)
            }
            CFormatErrorType::UnsupportedFormatChar(_) if args.len() == 0 => {
                type_error("not enough arguments for format string")
            }
            CFormatErrorType::UnsupportedFormatChar(c)
                if negative_format_char(&format[start + 1..index], c) =>
            {
                Error::new("OverflowError", "character argument not in range(0x110000)")
            }
            _ => Error::new("ValueError", CFormatError { typ, index }.to_string()),
        })?;
        if spec.format_char != 'o'
            || format.get(start + 1) == Some(&b'(')
            || matches!(spec.min_field_width, Some(CFormatQuantity::FromValuesTuple))
            || matches!(
                spec.precision,
                Some(CFormatPrecision::Quantity(CFormatQuantity::FromValuesTuple))
            )
        {
            return Err(Error::new(
                "NotImplementedError",
                "bytes formatting supports positional octal fields with static width and precision",
            ));
        }
        let value = args
            .next()
            .ok_or_else(|| type_error("not enough arguments for format string"))?;
        let end = input.peek().map_or(format.len(), |&(i, _)| i);
        let field = std::str::from_utf8(&format[start + 1..end])
            .expect("positional octal fields are ASCII");
        let rendered = render_percent_integer(ctx, value, &spec, field)?;
        // Keep space for the unprocessed literal bytes as well as this field.
        output
            .try_reserve(rendered.len().saturating_add(format.len() - end))
            .map_err(memory)?;
        output.extend_from_slice(rendered.as_bytes());
    }
    if args.len() != 0 {
        let mapping = match ctx.heap.get(rhs) {
            Value::Dict(_) | Value::DefaultDict { .. } | Value::List(_) | Value::Range { .. } => {
                true
            }
            Value::Instance { class, .. } => {
                ctx.class_attr(*class, "__getitem__").is_some() || ctx.dict_storage(rhs).is_some()
            }
            _ => false,
        };
        if !mapping {
            return Err(type_error(
                "not all arguments converted during bytes formatting",
            ));
        }
    }
    Ok(ctx.alloc(Value::Bytes(output)))
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
    let mut args = percent_args(ctx, rhs)?;
    let mut output = String::new();
    let mut positions = format.char_indices().enumerate();
    for (position, part) in parsed.iter() {
        match part {
            CFormatPart::Literal(literal) => output.push_str(literal),
            CFormatPart::Spec(spec) => {
                let decorated = spec.precision.is_some()
                    || spec.min_field_width.is_some()
                    || !spec.flags.is_empty();
                let number_field = matches!(spec.format_char, 'd' | 'i' | 'u' | 'o' | 'x' | 'X')
                    && !matches!(spec.min_field_width, Some(CFormatQuantity::FromValuesTuple))
                    && !matches!(
                        spec.precision,
                        Some(CFormatPrecision::Quantity(CFormatQuantity::FromValuesTuple))
                    );
                if decorated && !number_field {
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
                if decorated {
                    let (_, (start, _)) = positions
                        .find(|(i, _)| i == position)
                        .expect("parsed percent fields follow their source character positions");
                    let rendered = render_percent_integer(ctx, arg, spec, &format[start + 1..])?;
                    output
                        .try_reserve(rendered.len())
                        .map_err(|_| Error::new("MemoryError", ""))?;
                    output.push_str(&rendered);
                    continue;
                }
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
    owner: &str,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&receiver, args) = args.split_first().ok_or_else(|| {
        if name == "__new__" {
            type_error("str.__new__(): not enough arguments")
        } else if owner == "str" {
            type_error(format!("unbound method str.{name}() needs an argument"))
        } else {
            type_error("missing string receiver")
        }
    })?;
    if name == "__new__" {
        return ctx.new_str(receiver, args, kwargs);
    }
    if owner == "str" && ctx.str_value(receiver).is_err() {
        return Err(type_error(format!(
            "descriptor '{name}' for 'str' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        )));
    }
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
    if name == "expandtabs" {
        let [tabsize] = bind_method_arguments(name, args, kwargs, ["tabsize"], 1)?;
        let width = match tabsize {
            None => 8,
            Some(id) => {
                let index = ctx
                    .index_value_with_bool_result(id, false)?
                    .ok_or_else(|| {
                        type_error(format!(
                            "'{}' object cannot be interpreted as an integer",
                            ctx.type_name(id)
                        ))
                    })?;
                index
                    .to_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| {
                        Error::new("OverflowError", "Python int too large to convert to C int")
                    })?
            }
        };
        return Ok(
            match crate::objects::expand_tabs(ctx.str_value(receiver)?, width)? {
                Some(text) => ctx.text(text.into_string()),
                None => ctx.string_storage(receiver).unwrap(),
            },
        );
    }
    if name == "find" {
        if !kwargs.is_empty() {
            return Err(type_error("str.find() takes no keyword arguments"));
        }
        let Some(&needle) = args.first() else {
            return Err(type_error("find expected at least 1 argument, got 0"));
        };
        if args.len() > 3 {
            return Err(type_error(format!(
                "find expected at most 3 arguments, got {}",
                args.len()
            )));
        }
        if ctx.str_value(needle).is_err() {
            let kind = if needle == NONE {
                "None"
            } else {
                ctx.type_name(needle)
            };
            return Err(type_error(format!(
                "find() argument 1 must be str, not {kind}"
            )));
        }
        let mut bound = |arg: Option<&Id>| match arg {
            None | Some(&NONE) => Ok(None),
            Some(&arg) => ctx.index_value(arg)?.map(Some).ok_or_else(|| {
                type_error("slice indices must be integers or None or have an __index__ method")
            }),
        };
        let start = bound(args.get(1))?;
        let end = bound(args.get(2))?;
        let found = crate::objects::find_text(
            ctx.str_value(receiver)?,
            ctx.str_value(needle)?,
            start.as_ref(),
            end.as_ref(),
        );
        return Ok(ctx.alloc(Value::Int(found)));
    }
    if name == "rstrip"
        && let Value::Bytes(data) = ctx.heap.get(receiver)
    {
        positional(name, kwargs)?;
        arity(name, args, 0, 1)?;
        let chars = args
            .first()
            .copied()
            .filter(|&id| id != NONE)
            .map(|id| ctx.bytes_like(id))
            .transpose()?
            .unwrap_or(b" \t\n\x0b\x0c\r");
        return Ok(match crate::objects::rstrip_bytes(data, chars)? {
            Some(trimmed) => ctx.alloc(Value::Bytes(trimmed)),
            None => receiver,
        });
    }
    if name == "rsplit" && matches!(ctx.heap.get(receiver), Value::Bytes(_)) {
        let [sep, max] =
            super::bind_method_arguments("rsplit", args, kwargs, ["sep", "maxsplit"], 2)?;
        let limit = match max {
            None => usize::MAX,
            Some(id) => {
                let index = ctx.index_integer(id)?;
                let count = index
                    .to_i64()
                    .and_then(|value| isize::try_from(value).ok())
                    .ok_or_else(|| {
                        Error::new(
                            "OverflowError",
                            "Python int too large to convert to C ssize_t",
                        )
                    })?;
                usize::try_from(count).unwrap_or(usize::MAX)
            }
        };
        let separator = sep
            .filter(|&id| id != NONE)
            .map(|id| ctx.bytes_like(id))
            .transpose()?;
        let Value::Bytes(data) = ctx.heap.get(receiver) else {
            unreachable!()
        };
        let parts = crate::objects::rsplit_bytes(data, separator, limit)?;
        let fields = parts
            .into_iter()
            .map(|part| part.map_or(receiver, |bytes| ctx.alloc(Value::Bytes(bytes))))
            .collect();
        return Ok(ctx.alloc(Value::List(fields)));
    }
    if name == "isspace" {
        if !kwargs.is_empty() {
            return Err(type_error("str.isspace() takes no keyword arguments"));
        }
        if !args.is_empty() {
            return Err(type_error(format!(
                "str.isspace() takes no arguments ({} given)",
                args.len()
            )));
        }
        return Ok(boolean(crate::objects::python_isspace(
            ctx.str_value(receiver)?,
        )));
    }
    if name == "translate" {
        positional(name, kwargs)?;
        arity(name, args, 1, 1)?;
        let Value::Dict(table) = ctx.heap.get(args[0]) else {
            return Err(type_error("str.translate requires a plain dictionary"));
        };
        let text = translate::text(ctx.str_value(receiver)?, |ch| {
            let ordinal = i64::from(u32::from(ch));
            let entry = table
                .iter()
                .find(|&&(key, _)| match ctx.heap.get(ctx.scalar_value(key)) {
                    Value::Int(key) => key.to_i64() == Some(ordinal),
                    Value::Bool(key) => i64::from(*key) == ordinal,
                    _ => false,
                });
            let Some(&(_, id)) = entry else {
                return Ok(translate::Piece::Character(ch));
            };
            match ctx.heap.get(ctx.scalar_value(id)) {
                Value::None => Ok(translate::Piece::Text("")),
                Value::Str(value) => Ok(translate::Piece::Text(value)),
                Value::Int(value) => translate::Piece::ordinal(value),
                Value::Bool(value) => Ok(translate::Piece::Character(char::from(u8::from(*value)))),
                _ => Err(crate::Error::new(
                    crate::Kind::TypeError,
                    translate::INVALID_VALUE,
                )),
            }
        })?;
        return Ok(ctx.text(text.into_string()));
    }
    let source = ctx.str_value(receiver)?.to_owned();
    match name {
        "format" => {
            let result = format_string(ctx, &source, args, kwargs)?;
            Ok(ctx.text(result))
        }
        "partition" | "rpartition" => {
            if !kwargs.is_empty() {
                return Err(type_error(format!(
                    "str.{name}() takes no keyword arguments"
                )));
            }
            let [separator] = args else {
                return Err(type_error(format!(
                    "str.{name}() takes exactly one argument ({} given)",
                    args.len()
                )));
            };
            let sep = ctx.str_value(*separator).map_err(|_| {
                type_error(format!("must be str, not {}", ctx.type_name(*separator)))
            })?;
            let reverse = name == "rpartition";
            let fields = if let Some((before, after)) =
                crate::objects::partition_text(&source, sep, reverse)?
            {
                vec![ctx.text(before), *separator, ctx.text(after)]
            } else {
                let empty = ctx.text("");
                if reverse {
                    vec![empty, empty, receiver]
                } else {
                    vec![receiver, empty, empty]
                }
            };
            Ok(ctx.alloc(Value::Tuple(fields)))
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
