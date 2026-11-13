use std::borrow::Cow;

use num_traits::{Signed, ToPrimitive};

use ruff_python_literal::cformat::{
    CConversionFlags, CFormatContext, CFormatError, CFormatErrorType, CFormatPart,
    CFormatPrecision, CFormatQuantity, CFormatSpec, CFormatString, CFormatType,
};
use ruff_python_literal::format::{
    FieldName, FieldNamePart, FieldType, FormatPart, FormatString, FromTemplate,
};

use super::super::operators::append_rendered_text;
use super::super::stdlib::codecs::{decode_named, encode_text_object};
use super::*;

use crate::objects::{
    Case, codepoint_line, python_whitespace, split_parts, translate,
    visit_line_fields as split_lines,
};

mod bytes_fields;
mod maketrans;

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
        "ascii" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            format_conversion(ctx, args[0], 'a')
        }
        "repr" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            if let Value::TypeVarTuple { name, .. } = ctx.heap.get(args[0]) {
                return Ok(*name);
            }
            if let Some(result) = ctx.repr_override(args[0])? {
                return Ok(result);
            }
            let text = ctx.checked_format(args[0], true)?;
            Ok(ctx.text(text))
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
            let storage = if let Some(value) = value.filter(|_| base.is_none()) {
                if let Some(integer) = ctx.numeric_int(value)? {
                    return Ok(integer);
                }
                Some(ctx.scalar_value(value))
            } else {
                value.map(|id| ctx.string_storage(id).unwrap_or(id))
            };
            let number = match storage.map(|id| ctx.heap.get(id)) {
                None => crate::Int::from(0),
                Some(Value::Str(s)) if value == storage => {
                    if base.is_none()
                        && let Some(digit) = crate::integer_text::single_decimal_digit(s)
                    {
                        crate::Int::from(i64::from(digit))
                    } else {
                        crate::integer_text::parse_radix(s, radix as u32)?
                    }
                }
                Some(Value::Str(_) | Value::SurrogateStr(_)) => {
                    parse_integer_string(ctx, value.unwrap(), radix as u32)?
                }
                Some(Value::Bytes(b) | Value::ByteArray(b)) => {
                    crate::integer_text::parse_bytes_radix(b, radix as u32)?
                }
                _ if base.is_some() => {
                    return Err(type_error(
                        "int() can't convert non-string with explicit base",
                    ));
                }
                _ => {
                    return Err(type_error(format!(
                        "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                        ctx.type_name(value.unwrap())
                    )));
                }
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
                return decode_named(ctx, ctx.scalar_value(value), encoding, errors);
            }
            let target = ctx.weak_proxy_target(value)?;
            if target != value {
                ctx.heap.keep(target);
            }
            // mappingproxy.__str__ returns the backing mapping's string object directly.
            let value = ctx.mapping_proxy_target(target);
            if matches!(ctx.heap.get(value), Value::Str(_) | Value::SurrogateStr(_)) {
                return Ok(value);
            }
            if let Value::Calendar { tzname, .. } = *ctx.heap.get(value)
                && tzname != NONE
            {
                return Ok(tzname);
            }
            // A descriptor can return a temporary callable. Its lifetime ends with
            // the slot call, even when another Python callback immediately uses the text.
            let before_slot = ctx.heap.temporary_mark();
            let string_hook = ctx.bound_slot(value, "__str__")?;
            if string_hook.is_none()
                && let Value::Raised(error) = ctx.heap.get(value)
                && ctx.is_os_error(error)
            {
                if let Some(text) = ctx.checked_os_text(value)? {
                    return Ok(text);
                }
                let text = ctx.checked_format(value, false)?;
                return Ok(ctx.text(text));
            }
            let method = match string_hook {
                Some(method) => Some(method),
                None if matches!(ctx.heap.get(value), Value::Raised(_))
                    || ctx.string_storage(value).is_some() =>
                {
                    None
                }
                None => ctx.bound_slot(value, "__repr__")?,
            };
            if let Some(method) = method {
                let result = ctx.call(method, Vec::new(), Vec::new());
                let result = ctx.release_with_result(before_slot, result)?;
                if ctx.string_storage(result).is_none() {
                    return Err(type_error(format!(
                        "__str__ returned non-string (type {})",
                        ctx.type_name(result)
                    )));
                }
                return Ok(result);
            }
            if let Value::Raised(error) = ctx.heap.get(value)
                && let Some(fields) = ctx.syntax_members(error)
            {
                let (message, filename, line) = (fields[0], fields[1], fields[2]);
                if let Some(suffix) = ctx.surrogate_syntax_suffix(filename, line)? {
                    return ctx.with_slot_depth(
                        "maximum recursion depth exceeded while getting the str of an object",
                        |ctx| {
                            let text = super::call(ctx, "str", &[message], &[])?;
                            ctx.concat_strings(text, suffix)
                        },
                    );
                }
                if ctx.string_storage(filename).is_none()
                    && !matches!(ctx.heap.get(line), Value::Int(_))
                {
                    // Without a location, SyntaxError delegates directly to str(msg), including its identity.
                    if matches!(ctx.heap.get(message), Value::Str(_)) {
                        return Ok(message);
                    }
                    let class = match ctx.heap.get(message) {
                        Value::Instance { class, .. } => Some(*class),
                        Value::Raised(error) => Some(error.class),
                        _ => None,
                    };
                    if class.is_some_and(|class| {
                        ctx.class_attr(class, "__str__").is_some()
                            || ctx.class_attr(class, "__repr__").is_some()
                    }) {
                        return super::call(ctx, "str", &[message], &[]);
                    }
                }
            }
            if let Value::SurrogateStr(points) = ctx.heap.get(ctx.scalar_value(value)) {
                // str(subclass) returns an exact str, distinct from the subclass's payload.
                return Ok(ctx.text_codepoints(points.clone()));
            }
            if let Some(text) = ctx.single_exception_str(value)? {
                return Ok(text);
            }
            let text = ctx.checked_format(value, false)?;
            Ok(ctx.text(text))
        }
        "bytes" | "bytearray" => {
            let [value, encoding_id, errors] =
                bind(name, args, kwargs, ["source", "encoding", "errors"])?;
            if let ("bytes", Some(source), None, None) = (name, value, encoding_id, errors) {
                if let Some(result) = call_bytes_method(ctx, source)? {
                    return Ok(result);
                }
                if ctx.string_storage(source).is_none()
                    && let Some(bytes) = bytes_count(ctx, source)?
                {
                    return Ok(ctx.alloc(Value::Bytes(bytes)));
                }
            }
            let bytes = match value.map(|id| ctx.heap.get(id)) {
                Some(_) if value.is_some_and(|id| ctx.string_storage(id).is_some()) => {
                    if encoding_id.is_none() {
                        return Err(type_error("string argument without an encoding"));
                    }
                    let result = encode_text_object(ctx, value.unwrap(), encoding_id, errors)?;
                    if name == "bytes" {
                        return Ok(result);
                    }
                    ctx.bytes_like(result)?.to_vec()
                }
                _ if encoding_id.is_some() => {
                    return Err(type_error("encoding without a string argument"));
                }
                _ if errors.is_some() => {
                    return Err(type_error("errors without a string argument"));
                }
                None => Vec::new(),
                Some(Value::Bytes(_)) if name == "bytes" => return Ok(value.unwrap()),
                Some(Value::Int(_) | Value::Bool(_)) => {
                    zeroed_bytes(ctx.as_integer(value.unwrap())?)?
                }
                _ => {
                    let source = value.unwrap();
                    let bytes = bytes_from_source(ctx, source, name == "bytes")?;
                    // An exact tuple or buffer fills newly allocated bytes. The iterable
                    // writer instead finishes from known byte data and can return a singleton.
                    if name == "bytes"
                        && !matches!(ctx.heap.get(source), Value::Tuple(_))
                        && !matches!(
                            ctx.heap.get(ctx.scalar_value(source)),
                            Value::Bytes(_)
                                | Value::ByteArray(_)
                                | Value::MemoryView { .. }
                                | Value::BytesIoBuffer { .. }
                                | Value::Array(_)
                        )
                    {
                        return Ok(ctx.heap.bytes_from_data(bytes));
                    }
                    bytes
                }
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

/// Parse a string whose original Python repr is needed if conversion fails.
fn parse_integer_string(ctx: &mut Context, value: Id, base: u32) -> Result<crate::Int> {
    const REPR_LIMIT: usize = 200;

    let text = match ctx.heap.get(ctx.scalar_value(value)) {
        Value::Str(s) => Cow::Borrowed(s.as_str()),
        Value::SurrogateStr(points) => {
            // CPython substitutes '?' at the first unconvertible code point. Keep the
            // prefix so an embedded NUL still gets the normal integer digit-limit check.
            let mut text: String = points.iter().map_while(|&cp| char::from_u32(cp)).collect();
            text.push('?');
            Cow::Owned(text)
        }
        _ => unreachable!("expected str storage"),
    };
    if let Some(integer) = crate::integer_text::try_parse_radix(&text, base)? {
        return Ok(integer);
    }
    let repr = super::call(ctx, "repr", &[value], &[])?;
    let mut message: Vec<_> = format!("invalid literal for int() with base {base}: ")
        .chars()
        .map(u32::from)
        .collect();
    match ctx.heap.get(ctx.scalar_value(repr)) {
        Value::Str(text) => message.extend(text.chars().take(REPR_LIMIT).map(u32::from)),
        Value::SurrogateStr(points) => message.extend(points.iter().take(REPR_LIMIT)),
        _ => return Err(type_error("expected str")),
    }
    let message = ctx.text_codepoints(message);
    Err(ctx.exception("ValueError", &[message]))
}

pub(super) fn bytes_hex(
    ctx: &mut Context,
    owner: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (receiver, rest) = method_receiver_args(ctx, owner, "hex", args, &[])?;
    let [sep, group] = bind_method_arguments("hex", rest, kwargs, ["sep", "bytes_per_sep"], 2)?;
    let group = hex_group(ctx, group)?;
    let separator = sep
        .map(|sep| ctx.with_buffer_export(receiver, |ctx| hex_separator(ctx, sep, true)))
        .transpose()?;
    let bytes = ctx.bytes_like(receiver)?;
    if separator.is_none() || group == 0 {
        return Ok(ctx.text(crate::objects::hash_hex(bytes)?.into_string()));
    }
    let output = crate::objects::hex_with_separator(bytes, separator, group)?;
    Ok(ctx.text(String::from_utf8(output).expect("ASCII separator and hex digits")))
}

pub(in crate::dynamic) fn hex_group(ctx: &mut Context, group: Option<Id>) -> Result<i32> {
    match group {
        Some(id) => {
            ctx.index_integer(id)?.big().to_i32().ok_or_else(|| {
                Error::new("OverflowError", "Python int too large to convert to C int")
            })
        }
        None => Ok(1),
    }
}

pub(in crate::dynamic) fn hex_separator(
    ctx: &mut Context,
    sep: Id,
    ascii_only: bool,
) -> Result<u8> {
    let len = super::call(ctx, "len", &[sep], &[])?;
    if ctx.as_integer(len)? != 1 {
        return Err(Error::new("ValueError", "sep must be length 1."));
    }
    let byte = match ctx.heap.get(ctx.scalar_value(sep)) {
        // A str subclass may override __len__. CPython still requires one-byte storage.
        Value::Str(text) if text.chars().all(|ch| u32::from(ch) <= 0xff) => {
            Some(text.chars().next().unwrap_or_default() as u8)
        }
        Value::Str(_) | Value::SurrogateStr(_) => None,
        Value::Bytes(bytes) => Some(bytes.first().copied().unwrap_or(0)),
        _ => return Err(type_error("sep must be str or bytes.")),
    };
    byte.filter(|byte| !ascii_only || byte.is_ascii())
        .ok_or_else(|| Error::new("ValueError", "sep must be ASCII."))
}

/// Decode one contiguous ASCII buffer, accepting whitespace only between complete pairs.
pub(super) fn fromhex(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&class, args) = args
        .split_first()
        .ok_or_else(|| type_error("fromhex needs a class"))?;
    let method_name = ctx
        .own_class_attr(class, "__qualname__")
        .and_then(|v| ctx.str_value(v).ok())
        .map(|qualname| format!("{qualname}.fromhex"));
    let value = one_positional_arg(method_name.as_deref().unwrap_or(name), args, kwargs)?;
    let invalid = |position: usize| {
        Error::new(
            "ValueError",
            format!("non-hexadecimal number found in fromhex() arg at position {position}"),
        )
    };
    let data = match ctx.heap.get(ctx.scalar_value(value)) {
        Value::Str(text) => {
            if let Some(position) = text.as_bytes().iter().position(|b| !b.is_ascii()) {
                return Err(invalid(position));
            }
            text.as_bytes()
        }
        Value::SurrogateStr(points) => {
            return Err(invalid(
                points
                    .iter()
                    .position(|&c| c >= 128)
                    .expect("surrogate in Python string"),
            ));
        }
        Value::Array(array) => array.as_bytes(),
        _ => ctx.bytes_like(value).map_err(|err| {
            if err.kind == "TypeError" {
                let mut kind = ctx.type_name(value).to_owned();
                if let Value::Instance { class, .. } = *ctx.heap.get(value) {
                    if let Some(name) = ctx
                        .own_class_attr(class, "__qualname__")
                        .and_then(|v| ctx.str_value(v).ok())
                    {
                        kind = name.to_owned();
                    }
                    if let Some(module) = ctx
                        .own_class_attr(class, "__module__")
                        .and_then(|v| ctx.str_value(v).ok())
                        && !matches!(module, "builtins" | "__main__")
                    {
                        kind = format!("{module}.{kind}");
                    }
                }
                type_error(format!(
                    "fromhex() argument must be str or bytes-like, not {kind}"
                ))
            } else {
                err
            }
        })?,
    };
    let mut result = Vec::new();
    result
        .try_reserve_exact(data.len() / 2)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let mut digits = data.iter().copied().enumerate();
    while let Some((position, first)) = digits.next() {
        if first.is_ascii_whitespace() || first == 0x0b {
            continue;
        }
        let high = crate::objects::hexadecimal(first).ok_or_else(|| invalid(position))?;
        let (position, second) = digits.next().ok_or_else(|| {
            Error::new(
                "ValueError",
                "fromhex() arg must contain an even number of hexadecimal digits",
            )
        })?;
        let low = crate::objects::hexadecimal(second).ok_or_else(|| invalid(position))?;
        result.push((high << 4) | low);
    }
    match ctx.heap.get(class) {
        Value::Native(owner) if owner == "bytes" => Ok(ctx.alloc(Value::Bytes(result))),
        Value::Native(owner) if owner == "bytearray" => Ok(ctx.alloc(Value::ByteArray(result))),
        _ => {
            let argument = ctx.alloc(Value::Bytes(result));
            ctx.call(class, vec![argument], Vec::new())
        }
    }
}

/// Decode a bytearray position or size before inspecting its potentially mutated storage.
fn bytearray_index(ctx: &mut Context, value: Id) -> Result<isize> {
    crate::signed_index(&ctx.index_integer(value)?).ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

/// Apply a bytearray method to the underlying storage of an exact or subclass receiver.
pub(super) fn bytearray_method(
    ctx: &mut Context,
    name: &str,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, args)) = args.split_first() else {
        return Err(type_error(format!(
            "unbound method {name}() needs an argument"
        )));
    };
    let storage = ctx.scalar_value(receiver);
    let Value::ByteArray(data) = ctx.heap.get(storage) else {
        return Err(type_error(format!(
            "descriptor '{method}' for 'bytearray' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        )));
    };
    if matches!(method, "clear" | "copy" | "reverse") {
        if !kwargs.is_empty() {
            return Err(type_error(format!("{name}() takes no keyword arguments")));
        }
        if !args.is_empty() {
            return Err(type_error(format!(
                "{name}() takes no arguments ({} given)",
                args.len()
            )));
        }
        if method == "copy" {
            let copy = super::super::copy_ids(data)?;
            return Ok(ctx.alloc(Value::ByteArray(copy)));
        }
        if method == "clear" && !data.is_empty() {
            ctx.check_bytearray_resize(storage)?;
        }
        let Value::ByteArray(data) = ctx.heap.get_mut(storage) else {
            unreachable!()
        };
        if method == "reverse" {
            data.reverse();
        } else {
            data.clear();
        }
        return Ok(NONE);
    }
    if matches!(method, "insert" | "pop") && !kwargs.is_empty() {
        return Err(type_error(format!("{name}() takes no keyword arguments")));
    }
    if method == "insert" {
        let [index, item] = args else {
            return Err(type_error(format!(
                "insert expected 2 arguments, got {}",
                args.len()
            )));
        };
        let index = bytearray_index(ctx, *index)?;
        let byte = ctx.byte_value(*item)?;
        // Either __index__ can resize the receiver or change its active exports.
        let storage = ctx.scalar_value(receiver);
        let len = ctx.bytes_like(storage)?.len();
        if len == isize::MAX as usize {
            return Err(Error::new(
                "OverflowError",
                "cannot add more objects to bytearray",
            ));
        }
        ctx.check_bytearray_resize(storage)?;
        let position = if index < 0 {
            len.saturating_add_signed(index)
        } else {
            len.min(index as usize)
        };
        let Value::ByteArray(data) = ctx.heap.get_mut(storage) else {
            unreachable!()
        };
        data.try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        data.insert(position, byte);
        return Ok(NONE);
    }
    if method == "pop" {
        let index = match args {
            [] => -1,
            [index] => bytearray_index(ctx, *index)?,
            _ => {
                return Err(type_error(format!(
                    "pop expected at most 1 argument, got {}",
                    args.len()
                )));
            }
        };
        // __index__ can resize the bytearray or acquire a new buffer export.
        let storage = ctx.scalar_value(receiver);
        let len = ctx.bytes_like(storage)?.len();
        if len == 0 {
            return Err(Error::new("IndexError", "pop from empty bytearray"));
        }
        let index = crate::normalized_index(index, len)
            .ok_or_else(|| Error::new("IndexError", "pop index out of range"))?;
        ctx.check_bytearray_resize(storage)?;
        let Value::ByteArray(data) = ctx.heap.get_mut(storage) else {
            unreachable!()
        };
        let byte = data.remove(index);
        return Ok(ctx.int(i64::from(byte)));
    }
    let argument = one_positional_arg(name, args, kwargs)?;
    if method == "resize" {
        let size = bytearray_index(ctx, argument)?;
        let size = usize::try_from(size).map_err(|_| {
            Error::new(
                "ValueError",
                format!("Can only resize to positive sizes, got {size}"),
            )
        })?;
        // __index__ may have resized the receiver or changed its active exports.
        let storage = ctx.scalar_value(receiver);
        let current = ctx.bytes_like(storage)?.len();
        if size != current {
            ctx.check_bytearray_resize(storage)?;
            // CPython also needs room for the trailing NUL.
            if size == isize::MAX as usize {
                return Err(Error::new("MemoryError", ""));
            }
            let Value::ByteArray(data) = ctx.heap.get_mut(storage) else {
                unreachable!()
            };
            data.try_reserve_exact(size.saturating_sub(current))
                .map_err(|_| Error::new("MemoryError", ""))?;
            data.resize(size, 0);
        }
        return Ok(NONE);
    }
    #[cfg(target_os = "linux")]
    if method == "extend"
        && matches!(
            ctx.heap.get(argument),
            Value::Mmap { .. } | Value::MemoryView { .. }
        )
        && let Ok(Some((owner, range))) = super::super::stdlib::mmap::mapped_span(ctx, argument)
    {
        if !range.is_empty() {
            let map = std::rc::Rc::clone(super::super::stdlib::mmap::region(ctx.heap.get(owner))?);
            ctx.check_bytearray_resize(storage)?;
            let Value::ByteArray(target) = ctx.heap.get_mut(storage) else {
                unreachable!()
            };
            let len = range.len();
            super::super::memoryview::Storage::Mapped(&map, range)
                .append_buffered(target, 0, len)?;
        }
        return Ok(NONE);
    }
    let byte;
    let extra = if method == "append" {
        byte = [ctx.byte_value(argument)?];
        Cow::Borrowed(byte.as_slice())
    } else if matches!(
        ctx.heap.get(ctx.scalar_value(argument)),
        Value::Array(_) | Value::MemoryView { .. }
    ) {
        ctx.check_buffer_exporter(argument)?;
        let bytes = ctx.native_buffer_bytes(argument).map_err(|_| {
            type_error(format!(
                "can't set bytearray slice from {}",
                ctx.type_name(argument)
            ))
        })?;
        Cow::Owned(super::super::copy_ids(bytes)?)
    } else {
        Cow::Owned(bytes_from_iterable(ctx, argument)?)
    };
    if !extra.is_empty() {
        ctx.check_bytearray_resize(storage)?;
        let Value::ByteArray(data) = ctx.heap.get_mut(storage) else {
            unreachable!()
        };
        data.try_reserve(extra.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        data.extend_from_slice(&extra);
    }
    Ok(NONE)
}

/// Call the type's `__bytes__` hook and preserve the identity of its bytes result.
pub(super) fn call_bytes_method(ctx: &mut Context, source: Id) -> Result<Option<Id>> {
    let Some(method) = ctx.bound_slot(source, "__bytes__")? else {
        return Ok(None);
    };
    let result = ctx.call(method, Vec::new(), Vec::new())?;
    if !matches!(ctx.heap.get(ctx.scalar_value(result)), Value::Bytes(_)) {
        let name = ctx.type_name(result);
        return Err(type_error(format!(
            "__bytes__ returned non-bytes (type {})",
            &name[..name.floor_char_boundary(200)]
        )));
    }
    Ok(Some(result))
}

pub(in super::super) fn bytes_from_iterable(ctx: &mut Context, obj: Id) -> Result<Vec<u8>> {
    bytes_from_source(ctx, obj, false)
}

/// The count form allocates distinct single-byte results, unlike the iterable writer.
fn bytes_count(ctx: &mut Context, source: Id) -> Result<Option<Vec<u8>>> {
    // bytes(x) tries the index slot before buffers and iterators. A TypeError
    // from that slot falls through to PyBytes_FromObject.
    let ty = ctx.builtin("TypeError");
    match ctx.index_result(source) {
        Ok(Some((_, number))) => {
            let size = number.to_i64().ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    format!(
                        "cannot fit '{}' into an index-sized integer",
                        ctx.type_name(source)
                    ),
                )
            })?;
            zeroed_bytes(size).map(Some)
        }
        Ok(None) => Ok(None),
        Err(error) if ctx.raised_error_matches(&error, ty) => Ok(None),
        Err(error) => Err(error),
    }
}

fn bytes_from_source(ctx: &mut Context, obj: Id, bytes_constructor: bool) -> Result<Vec<u8>> {
    #[cfg(target_os = "linux")]
    if matches!(ctx.heap.get(obj), Value::Mmap { .. }) {
        return Ok(ctx.read_buffer_bytes(obj)?.into_owned());
    }
    if bytes_constructor && let Value::Array(array) = ctx.heap.get(ctx.scalar_value(obj)) {
        return super::super::copy_ids(array.as_bytes());
    }
    if matches!(ctx.heap.get(obj), Value::MemoryView { .. }) {
        ctx.check_view_export(obj)?;
        return ctx.memoryview_bytes(obj);
    }
    if matches!(
        ctx.heap.get(ctx.scalar_value(obj)),
        Value::Bytes(_) | Value::ByteArray(_) | Value::BytesIoBuffer { .. }
    ) {
        return super::super::copy_ids(ctx.bytes_like(obj)?);
    }
    // PyBytes_FromObject replaces TypeError only while acquiring the iterator.
    let iter = ctx.iterator(obj).map_err(|error| {
        let ty = ctx.builtin("TypeError");
        if bytes_constructor && ctx.raised_error_matches(&error, ty) {
            type_error(format!(
                "cannot convert '{:.200}' object to bytes",
                ctx.type_name(obj)
            ))
        } else {
            error
        }
    })?;
    ctx.heap.keep(iter);
    let mark = ctx.heap.temporary_mark();
    let mut bytes = Vec::new();
    while let Some(item) = ctx.next_item(iter)? {
        ctx.heap.keep(item);
        let byte = ctx
            .index_integer(item)?
            .to_i64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| Error::new("ValueError", "bytes must be in range(0, 256)"))?;
        bytes
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        bytes.push(byte);
        ctx.heap.release_temporaries_since(mark);
    }
    Ok(bytes)
}

pub(in super::super) fn zeroed_bytes(size: i64) -> Result<Vec<u8>> {
    let size = usize::try_from(size).map_err(|_| Error::new("ValueError", "negative count"))?;
    let mut data = Vec::new();
    data.try_reserve_exact(size)
        .map_err(|_| Error::new("MemoryError", "bytes allocation is too large"))?;
    data.resize(size, 0);
    Ok(data)
}

/// Convert a numeric formatting operand to binary64, checking large integers first.
fn float_operand(ctx: &Context, value: Id) -> Result<Option<f64>> {
    Ok(match ctx.heap.get(ctx.scalar_value(value)) {
        Value::Float(n) => Some(*n),
        Value::Int(n) => Some(crate::floats::int_to_float(n)?),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    })
}

/// Apply the type's format method and return its string, including a string subclass.
pub(in super::super) fn format_value(ctx: &mut Context, value: Id, spec: Option<Id>) -> Result<Id> {
    if let Some(spec) = spec
        && ctx.string_storage(spec).is_none()
    {
        return Err(type_error(format!(
            "format() argument 2 must be str, not {}",
            ctx.type_name(spec)
        )));
    }
    let method = match ctx.heap.get(value) {
        Value::Calendar {
            value: calendar, ..
        } if matches!(calendar.class(), "date" | "datetime" | "time") => {
            Some(ctx.lookup_attr(value, "__format__")?)
        }
        _ => ctx.bound_slot(value, "__format__")?,
    };
    if let Some(method) = method {
        let spec = match spec {
            Some(spec) => spec,
            None => ctx
                .heap
                .source_string_constant("")
                .ok_or_else(|| Error::new("MemoryError", ""))?,
        };
        let result = ctx.call(method, super::super::copy_ids(&[spec])?, Vec::new())?;
        if ctx.string_storage(result).is_none() {
            return Err(type_error(format!(
                "__format__ must return a str, not {}",
                ctx.type_name(result)
            )));
        }
        return Ok(result);
    }
    let owner = format_owner(ctx.type_name(ctx.scalar_value(value)));
    builtin_format(ctx, owner, value, spec)
}

/// Render with the selected base method without redispatching to the receiver's override.
pub(super) fn builtin_format(
    ctx: &mut Context,
    owner: &str,
    value: Id,
    spec: Option<Id>,
) -> Result<Id> {
    let scalar_tail;
    let mut surrogate_fill = None;
    let spec = match spec {
        Some(id)
            if owner == "str"
                && let Value::SurrogateStr(points) = ctx.heap.get(ctx.scalar_value(id))
                && let [fill, rest @ ..] = points.as_slice()
                && char::from_u32(*fill).is_none()
                && let Some(tail) = rest
                    .iter()
                    .copied()
                    .map(char::from_u32)
                    .collect::<Option<String>>()
                && matches!(crate::format_grammar::alignment(&tail), (None, Some(_), _)) =>
        {
            // Keep the Python fill code point when only the alignment and its tail fit in UTF-8.
            surrogate_fill = Some(*fill);
            scalar_tail = tail;
            scalar_tail.as_str()
        }
        Some(id) => ctx.str_value(id)?,
        None => "",
    };
    if owner == "object" {
        crate::objects::scalar_format::check_object_spec(spec, ctx.type_name(value))?;
    }
    if spec.is_empty() {
        if let Value::Int(number) = ctx.heap.get(value) {
            let text = super::super::operators::rendered_integer(number)?;
            return Ok(ctx.text(text.into_string()));
        }
        return super::call(ctx, "str", &[value], &[]);
    }
    if let Some(storage) = ctx.string_storage(value) {
        let (field, precision) = crate::objects::scalar_format::string_field(
            spec,
            surrogate_fill.is_none(),
            ctx.type_name(value),
        )?;
        if surrogate_fill.is_none()
            && let Value::Str(text) = ctx.heap.get(storage)
        {
            return Ok(
                match crate::objects::scalar_format::utf8_field(text, &field, precision)? {
                    Some(text) => ctx.alloc(Value::Str(text)),
                    None if text.is_empty() => storage,
                    None => value,
                },
            );
        }
        let alignment = match field.align {
            Some('>') => "rjust",
            Some('^') => "format-center",
            _ => "ljust",
        };
        let width = field.width;
        let fill = surrogate_fill.unwrap_or(u32::from(field.fill));
        let mut text = storage;
        if let Some(limit) = precision
            && limit < ctx.instance_len(storage)?.unwrap()
        {
            let stop = ctx.int(limit as i64);
            let slice = ctx.alloc(Value::Slice([NONE, stop, NONE]));
            text = ctx.get_item(storage, slice)?;
        }
        let padded = if width == 0 {
            text
        } else {
            justify_string(ctx, text, width, fill, alignment)?
        };
        // An unchanged nonempty string subclass keeps its identity.
        return Ok(if padded == storage && ctx.truth(storage)? {
            value
        } else {
            padded
        });
    }
    if let Value::Float(number) = ctx.heap.get(ctx.scalar_value(value)) {
        let text = crate::objects::scalar_format::float_field(spec, *number, ctx.type_name(value))?;
        return Ok(ctx.text(text));
    }
    if matches!(
        ctx.heap.get(ctx.scalar_value(value)),
        Value::Bool(_) | Value::Int(_)
    ) {
        let text = crate::objects::scalar_format::integer_field(
            spec,
            ctx.as_int(value)?,
            ctx.type_name(value),
        )?;
        return Ok(ctx.text(text));
    }
    Err(type_error(format!("unsupported format string {spec:?}")))
}

/// Convert a brace or f-string field before rendering any nested format specification.
pub(in super::super) fn format_conversion(ctx: &mut Context, value: Id, conv: char) -> Result<Id> {
    Ok(match conv {
        's' => super::call(ctx, "str", &[value], &[])?,
        'r' => super::call(ctx, "repr", &[value], &[])?,
        'a' => {
            let repr = super::call(ctx, "repr", &[value], &[])?;
            if let Value::SurrogateStr(points) = ctx.heap.get(ctx.scalar_value(repr)) {
                return Ok(ctx.text(ascii_codepoints(points.iter().copied())));
            }
            let text = ctx.str_value(repr)?;
            if text.is_ascii() {
                repr
            } else {
                let escaped = ascii(text);
                ctx.text(escaped)
            }
        }
        _ => return Err(Error::new("ValueError", "unknown format conversion")),
    })
}

#[derive(Clone, Copy)]
enum FormatSource<'a> {
    Arguments(&'a [Id], &'a [(String, Id)]),
    Mapping(Id),
}

fn format_string(ctx: &mut Context, source: &str, fields: FormatSource<'_>) -> Result<Id> {
    #[derive(Default)]
    struct Indices {
        next: usize,
        manual: bool,
    }
    fn append_format_text(points: &mut Vec<u32>, text: &str) -> Result<()> {
        points
            .try_reserve(text.chars().count())
            .map_err(|_| crate::Error::memory_error())?;
        points.extend(text.chars().map(u32::from));
        Ok(())
    }
    fn render(
        ctx: &mut Context,
        s: &str,
        fields: FormatSource<'_>,
        indices: &mut Indices,
        nested: bool,
    ) -> Result<Id> {
        let parsed =
            FormatString::from_raw_str(s).map_err(|e| Error::new("ValueError", e.to_string()))?;
        let single = parsed.format_parts.len() == 1;
        let mut output = String::new();
        let mut codepoints = Vec::new();
        for part in parsed.format_parts {
            let FormatPart::Field {
                field_name,
                conversion_spec,
                format_spec,
            } = part
            else {
                if let FormatPart::Literal(text) = part {
                    append_rendered_text(&mut output, &text)?;
                }
                continue;
            };
            let field = FieldName::parse(&field_name)
                .map_err(|e| Error::new("ValueError", e.to_string()))?;
            let positional = |index| {
                let FormatSource::Arguments(args, _) = fields else {
                    return Err(Error::new(
                        "ValueError",
                        "Format string contains positional fields",
                    ));
                };
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
                FieldType::Keyword(key) => match fields {
                    FormatSource::Arguments(_, kwargs) => kwargs
                        .iter()
                        .find(|(k, _)| k == &key)
                        .map(|(_, id)| *id)
                        .ok_or_else(|| {
                            let key = ctx.text(key);
                            ctx.lookup_error("KeyError", key)
                        })?,
                    FormatSource::Mapping(mapping) => {
                        let key = ctx.text(key);
                        ctx.get_item(mapping, key)?
                    }
                },
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
                value = format_conversion(ctx, value, conv)?;
            }
            let spec = if format_spec.contains(['{', '}']) {
                if nested {
                    return Err(Error::new(
                        "ValueError",
                        "format specifier nested too deeply",
                    ));
                }
                Some(render(ctx, &format_spec, fields, indices, true)?)
            } else {
                (!format_spec.is_empty()).then(|| ctx.text(format_spec))
            };
            let rendered = format_value(ctx, value, spec)?;
            if single {
                return Ok(rendered);
            }
            if let Value::SurrogateStr(field) = ctx.heap.get(ctx.scalar_value(rendered)) {
                // Keep ordinary fields in UTF-8 until a field needs Python code points.
                append_format_text(&mut codepoints, &output)?;
                output.clear();
                codepoints
                    .try_reserve(field.len())
                    .map_err(|_| crate::Error::memory_error())?;
                codepoints.extend_from_slice(field);
            } else {
                append_rendered_text(&mut output, ctx.str_value(rendered)?)?;
            }
        }
        if codepoints.is_empty() {
            return Ok(ctx.text(output));
        }
        append_format_text(&mut codepoints, &output)?;
        Ok(ctx.text_codepoints(codepoints))
    }
    render(ctx, source, fields, &mut Indices::default(), false)
}

fn ascii(s: &str) -> String {
    ascii_codepoints(s.chars().map(u32::from))
}

fn ascii_codepoints(points: impl Iterator<Item = u32>) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for point in points {
        match point {
            n @ 0..=0x7f => out.push(n as u8 as char),
            n @ 0x80..=0xff => write!(out, "\\x{n:02x}").unwrap(),
            n @ 0x100..=0xffff => write!(out, "\\u{n:04x}").unwrap(),
            n => write!(out, "\\U{n:08x}").unwrap(),
        }
    }
    out
}

fn percent_args(ctx: &Context, rhs: Id) -> Result<std::vec::IntoIter<Id>> {
    let values = match ctx.tuple_items(rhs)? {
        Some(values) => values,
        None => super::super::copy_ids(&[rhs])?,
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

/// Convert the numeric operand after its starred width and precision have been consumed.
fn render_percent_number(
    ctx: &mut Context,
    args: &mut dyn Iterator<Item = Id>,
    spec: &CFormatSpec,
    field: &str,
) -> Result<String> {
    let (width, precision) = percent_layout(ctx, spec, args)?;
    let value = args
        .next()
        .ok_or_else(|| type_error("not enough arguments for format string"))?;
    let star_width =
        matches!(spec.min_field_width, Some(CFormatQuantity::FromValuesTuple)).then_some(width);
    let star_precision = precision.filter(|_| {
        matches!(
            spec.precision,
            Some(CFormatPrecision::Quantity(CFormatQuantity::FromValuesTuple))
        )
    });
    let stars = star_width
        .map(|w| crate::Int::from(w as i64))
        .into_iter()
        .chain(star_precision.map(crate::Int::from_usize));
    if matches!(spec.format_type, CFormatType::Float(_)) {
        let number = float_operand(ctx, value)?.ok_or_else(|| {
            type_error(format!("must be real number, not {}", ctx.type_name(value)))
        })?;
        return Ok(crate::objects::percent_float_field(field, stars, number)?);
    }
    let conversion = spec.format_char;
    let decimal = matches!(conversion, 'd' | 'i' | 'u');
    let mut convert = || {
        if decimal {
            if let Ok(integer) = ctx.as_int(value) {
                return Ok(integer);
            }
            if let Some(integer) = ctx.numeric_int(value)? {
                return ctx.as_int(integer);
            }
        }
        ctx.index_integer(value)
    };
    let integer = convert().map_err(|error| {
        let ty = ctx.builtin("TypeError");
        if ctx.raised_error_matches(&error, ty) {
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
    let precision = match spec.precision {
        Some(CFormatPrecision::Quantity(CFormatQuantity::Amount(n))) => Some(n),
        _ => star_precision,
    };
    if precision.is_some_and(|n| n > (i32::MAX - 3) as usize) {
        return Err(Error::new("OverflowError", "precision too large"));
    }
    let operands = stars.chain(std::iter::once(integer));
    crate::objects::percent_integer_field(field, operands).map_err(|e| {
        let error = Error::from(e);
        if error.kind == "MemoryError" {
            Error::new("MemoryError", "")
        } else {
            error
        }
    })
}

/// Interpolate binary fields without decoding the template or the arguments.
pub(in super::super) fn bytes_percent(ctx: &mut Context, format: &[u8], rhs: Id) -> Result<Id> {
    let mut args = percent_args(ctx, rhs)?;
    let mapping = bytes_fields::is_mapping(ctx, rhs);
    let mut input = format.iter().copied().enumerate().peekable();
    let mut output = Vec::new();
    let memory = |_| Error::new("MemoryError", "");
    output.try_reserve(format.len()).map_err(memory)?;
    while let Some((start, byte)) = input.next() {
        if byte != b'%' || input.next_if(|&(_, next)| next == b'%').is_some() {
            output.push(byte);
            continue;
        }
        // A mapping key is an arbitrary byte string. Look it up before parsing its field.
        if input.next_if(|&(_, byte)| byte == b'(').is_some() {
            if !mapping {
                return Err(type_error("format requires a mapping"));
            }
            let mut depth = 1;
            let (end, _) = input
                .find(|&(_, byte)| {
                    match byte {
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    depth == 0
                })
                .ok_or_else(|| Error::new("ValueError", "incomplete format key"))?;
            let key = ctx.alloc(Value::Bytes(format[start + 2..end].to_vec()));
            if matches!(ctx.heap.get(rhs), Value::List(_) | Value::Range(_)) {
                return Err(type_error(format!(
                    "{} indices must be integers or slices, not bytes",
                    ctx.type_name(rhs)
                )));
            }
            args = vec![ctx.get_item(rhs, key)?].into_iter();
        }
        let field_start = input.peek().map_or(format.len(), |&(i, _)| i);
        let mut parsed = if let Some(&(index, b'(')) = input.peek() {
            // A second opening parenthesis occupies the conversion-character position.
            Err((CFormatErrorType::UnsupportedFormatChar('('), index))
        } else {
            CFormatSpec::parse(&mut input, CFormatContext::Bytes)
        };
        // Ruff limits both quantities to i32. CPython's bytes width fits Py_ssize_t.
        if let Err((CFormatErrorType::IntTooBig, index)) = parsed
            && !format[field_start..index].contains(&b'.')
        {
            let digits = format[..index]
                .iter()
                .rposition(|b| !b.is_ascii_digit())
                .unwrap()
                + 1;
            while input.next_if(|(_, b)| b.is_ascii_digit()).is_some() {}
            let tail = input.peek().map_or(format.len(), |&(i, _)| i);
            let width = std::str::from_utf8(&format[digits..tail]).unwrap();
            if let Ok(width) = width.parse::<isize>() {
                // Validate the suffix with the same parser. The renderer uses the original width.
                let mut suffix = std::iter::once(b'1')
                    .chain(format[tail..].iter().copied())
                    .enumerate()
                    .peekable();
                parsed = CFormatSpec::parse(&mut suffix, CFormatContext::Bytes)
                    .map_err(|(kind, i)| (kind, tail - 1 + i));
                if let Ok(spec) = &mut parsed {
                    spec.min_field_width = Some(CFormatQuantity::Amount(width as usize));
                }
                let end = suffix.peek().map_or(format.len(), |&(i, _)| tail - 1 + i);
                while input.next_if(|&(i, _)| i < end).is_some() {}
            }
        }
        let spec = match parsed {
            Ok(spec) => spec,
            Err((typ, index)) => {
                let end = if matches!(typ, CFormatErrorType::IncompleteFormat) {
                    format.len()
                } else {
                    index
                };
                bytes_fields::consume_pending_stars(ctx, &format[field_start..end], &mut args)?;
                return Err(match typ {
                    CFormatErrorType::IntTooBig => {
                        let message = if format[field_start..index].contains(&b'.') {
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
                        if negative_format_char(&format[field_start..index], c) =>
                    {
                        Error::new("OverflowError", "character argument not in range(0x110000)")
                    }
                    _ => Error::new("ValueError", CFormatError { typ, index }.to_string()),
                });
            }
        };
        if matches!(spec.format_char, 's' | 'b' | 'c') {
            let end = input.peek().map_or(format.len(), |&(i, _)| i);
            bytes_fields::append(ctx, &spec, &mut args, &mut output, format.len() - end)?;
            continue;
        }
        if !matches!(spec.format_type, CFormatType::Number(_)) {
            return Err(Error::new(
                "NotImplementedError",
                "unsupported bytes format field",
            ));
        }
        let end = input.peek().map_or(format.len(), |&(i, _)| i);
        let field =
            std::str::from_utf8(&format[field_start..end]).expect("numeric fields are ASCII");
        let rendered = render_percent_number(ctx, &mut args, &spec, field)?;
        // Keep space for the unprocessed literal bytes as well as this field.
        output
            .try_reserve(rendered.len().saturating_add(format.len() - end))
            .map_err(memory)?;
        output.extend_from_slice(rendered.as_bytes());
    }
    if args.len() != 0 && !mapping {
        return Err(type_error(
            "not all arguments converted during bytes formatting",
        ));
    }
    Ok(ctx.alloc(Value::Bytes(output)))
}

/// Consume the width and precision before asking for, or converting, the field value.
fn percent_layout(
    ctx: &Context,
    spec: &CFormatSpec,
    args: &mut dyn Iterator<Item = Id>,
) -> Result<(isize, Option<usize>)> {
    let mut star = || {
        let arg = args
            .next()
            .ok_or_else(|| type_error("not enough arguments for format string"))?;
        ctx.as_int(arg).map_err(|_| type_error("* wants int"))
    };
    let width = match spec.min_field_width {
        None => 0,
        Some(CFormatQuantity::Amount(n)) => {
            isize::try_from(n).map_err(|_| Error::new("ValueError", "width too big"))?
        }
        Some(CFormatQuantity::FromValuesTuple) => star()?.big().to_isize().ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?,
    };
    let precision = match spec.precision {
        None => None,
        Some(CFormatPrecision::Dot) => Some(0),
        Some(CFormatPrecision::Quantity(CFormatQuantity::Amount(n))) => Some(
            i32::try_from(n).map_err(|_| Error::new("ValueError", "precision too big"))? as usize,
        ),
        Some(CFormatPrecision::Quantity(CFormatQuantity::FromValuesTuple)) => Some(
            star()?
                .big()
                .to_i32()
                .ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C int")
                })?
                .max(0) as usize,
        ),
    };
    Ok((width, precision))
}

/// Render the operand type name used by CPython's character conversion diagnostics.
fn percent_type_name(ctx: &mut Context, arg: Id) -> Result<&str> {
    if let Some(class) = ctx.heap.get(arg).instance_class() {
        let name = super::super::stdlib::qualified_type(ctx, class)?;
        ctx.str_value(name)
    } else {
        Ok(ctx.type_name(arg))
    }
}

/// Convert a Unicode `%c` argument without invoking string-subclass overrides.
fn percent_char(ctx: &mut Context, arg: Id) -> Result<Id> {
    if let Some(storage) = ctx.string_storage(arg) {
        let len = match ctx.heap.get(storage) {
            Value::Str(s) => s.chars().count(),
            Value::SurrogateStr(s) => s.len(),
            _ => unreachable!("string storage"),
        };
        return if len == 1 {
            Ok(storage)
        } else {
            Err(type_error(format!(
                "%c requires an int or a unicode character, not a string of length {len}"
            )))
        };
    }
    match ctx.index_integer(arg) {
        Ok(n) => {
            let point = n
                .to_i64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|&p| p <= 0x10ffff)
                .ok_or_else(|| Error::new("OverflowError", "%c arg not in range(0x110000)"))?;
            Ok(ctx.text_codepoints(vec![point]))
        }
        Err(error) if ctx.is_builtin_error(&error, "TypeError") => {
            let name = percent_type_name(ctx, arg)?;
            Err(type_error(format!(
                "%c requires an int or a unicode character, not {name}"
            )))
        }
        Err(error) => Err(error),
    }
}

/// Old-style interpolation accepts a single operand, a positional tuple or a mapping.
pub(in super::super) fn percent(ctx: &mut Context, format: &str, rhs: Id) -> Result<Id> {
    let parsed = format
        .parse::<CFormatString>()
        .map_err(|e| Error::new("ValueError", e.to_string()))?;
    let mapping = bytes_fields::is_mapping(ctx, rhs)
        || matches!(
            ctx.heap.get(ctx.scalar_value(rhs)),
            Value::Bytes(_) | Value::ByteArray(_)
        );
    let mut args = percent_args(ctx, rhs)?;
    let mut output = String::new();
    // Flush scalar runs into this buffer only for fields containing a lone surrogate.
    let mut codepoints = Vec::new();
    let mut positions = format.char_indices().enumerate();
    for (position, part) in parsed.iter() {
        match part {
            CFormatPart::Literal(literal) => output.push_str(literal),
            CFormatPart::Spec(spec) => {
                let decorated = spec.precision.is_some()
                    || spec.min_field_width.is_some()
                    || !spec.flags.is_empty();
                let float_field = matches!(spec.format_type, CFormatType::Float(_));
                let number_field =
                    float_field || matches!(spec.format_type, CFormatType::Number(_));
                let text_field = decorated && matches!(spec.format_char, 's' | 'r' | 'a' | 'c');
                if decorated && !number_field && !text_field {
                    return Err(Error::new(
                        "ValueError",
                        "unsupported flags, width or precision for percent interpolation",
                    ));
                }
                let mut mapped;
                let field_args: &mut dyn Iterator<Item = Id> = if let Some(name) = &spec.mapping_key
                {
                    if !mapping {
                        return Err(type_error("format requires a mapping"));
                    }
                    // A mapping lookup consumes the single positional operand.
                    args.next();
                    let key = ctx.text(name);
                    mapped = std::iter::once(ctx.get_item(rhs, key)?);
                    &mut mapped
                } else {
                    &mut args
                };
                if number_field {
                    let (_, (start, _)) = positions
                        .find(|(i, _)| i == position)
                        .expect("parsed percent fields follow their source character positions");
                    let rendered =
                        render_percent_number(ctx, field_args, spec, &format[start + 1..])?;
                    output
                        .try_reserve(rendered.len())
                        .map_err(|_| Error::new("MemoryError", ""))?;
                    output.push_str(&rendered);
                    continue;
                }
                let layout = text_field
                    .then(|| percent_layout(ctx, spec, field_args))
                    .transpose()?;
                let arg = field_args
                    .next()
                    .ok_or_else(|| type_error("not enough arguments for format string"))?;
                let rendered: Cow<'_, str> = match spec.format_char {
                    's' | 'c' => {
                        let text = if spec.format_char == 'c' {
                            percent_char(ctx, arg)?
                        } else {
                            super::call(ctx, "str", &[arg], &[])?
                        };
                        if let Value::SurrogateStr(points) = ctx.heap.get(ctx.scalar_value(text)) {
                            let (mut width, mut precision) = layout.unwrap_or_default();
                            if spec.format_char == 'c' {
                                precision = None;
                                // A negated minimum Py_ssize_t gives no padding in CPython.
                                if width == isize::MIN {
                                    width = 0;
                                }
                            }
                            let field =
                                &points[..precision.unwrap_or(points.len()).min(points.len())];
                            let padding = width.unsigned_abs().saturating_sub(field.len());
                            codepoints
                                .try_reserve(
                                    output
                                        .len()
                                        .saturating_add(field.len())
                                        .saturating_add(padding),
                                )
                                .map_err(|_| {
                                    Error::new("MemoryError", "formatted string is too long")
                                })?;
                            codepoints.extend(output.chars().map(u32::from));
                            output.clear();
                            let left =
                                width < 0 || spec.flags.contains(CConversionFlags::LEFT_ADJUST);
                            if !left {
                                codepoints.extend(std::iter::repeat_n(u32::from(' '), padding));
                            }
                            codepoints.extend_from_slice(field);
                            if left {
                                codepoints.extend(std::iter::repeat_n(u32::from(' '), padding));
                            }
                            continue;
                        }
                        Cow::Borrowed(ctx.str_value(text)?)
                    }
                    'r' => ctx.checked_format(arg, true)?.into(),
                    'a' => ascii(&ctx.checked_format(arg, true)?).into(),
                    _ => {
                        return Err(Error::new(
                            "ValueError",
                            format!("unsupported percent conversion {}", spec.format_char),
                        ));
                    }
                };
                if let Some((width, precision)) = layout {
                    let left = width < 0 || spec.flags.contains(CConversionFlags::LEFT_ADJUST);
                    let width = width.checked_abs().unwrap_or(0) as usize;
                    let end = precision
                        .filter(|_| spec.format_char != 'c')
                        .and_then(|n| rendered.char_indices().nth(n))
                        .map_or(rendered.len(), |(i, _)| i);
                    let text = &rendered[..end];
                    let padding = width.saturating_sub(text.chars().count());
                    output
                        .try_reserve(text.len().saturating_add(padding))
                        .map_err(|_| Error::new("MemoryError", "formatted string is too long"))?;
                    if !left {
                        output.extend(std::iter::repeat_n(' ', padding));
                    }
                    output.push_str(text);
                    if left {
                        output.extend(std::iter::repeat_n(' ', padding));
                    }
                } else {
                    output.push_str(&rendered);
                }
            }
        }
    }
    if !mapping && args.next().is_some() {
        return Err(type_error(
            "not all arguments converted during string formatting",
        ));
    }
    if codepoints.is_empty() {
        return Ok(ctx.text(output));
    }
    codepoints
        .try_reserve(output.len())
        .map_err(|_| Error::new("MemoryError", "formatted string is too long"))?;
    codepoints.extend(output.chars().map(u32::from));
    Ok(ctx.text_codepoints(codepoints))
}

/// Convert the optional start and end in argument order, honoring the index protocol.
fn search_bounds(
    ctx: &mut Context,
    args: &[Id],
) -> Result<(Option<crate::Int>, Option<crate::Int>)> {
    let mut bound = |arg: Option<&Id>| match arg {
        None | Some(&NONE) => Ok(None),
        Some(&arg) => ctx.index_value(arg)?.map(Some).ok_or_else(|| {
            type_error("slice indices must be integers or None or have an __index__ method")
        }),
    };
    Ok((bound(args.get(1))?, bound(args.get(2))?))
}

/// Convert a split limit through the index protocol, treating negative limits as unlimited.
fn split_limit(ctx: &mut Context, max: Option<Id>) -> Result<usize> {
    let Some(id) = max else {
        return Ok(usize::MAX);
    };
    let count = ctx
        .index_integer(id)?
        .to_i64()
        .and_then(|value| isize::try_from(value).ok())
        .ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

/// Split code points in the requested direction without scalarizing lone surrogates.
fn split_codepoints(
    source: &[u32],
    separator: Option<&[u32]>,
    mut limit: usize,
    reverse: bool,
    mut push: impl FnMut(&[u32]),
) {
    let whitespace = |&point: &u32| char::from_u32(point).is_some_and(python_whitespace);
    let mut rest = source;
    loop {
        if separator.is_none() {
            rest = if reverse {
                &rest[..rest
                    .iter()
                    .rposition(|p| !whitespace(p))
                    .map_or(0, |i| i + 1)]
            } else {
                &rest[rest
                    .iter()
                    .position(|p| !whitespace(p))
                    .unwrap_or(rest.len())..]
            };
            if rest.is_empty() {
                break;
            }
        }
        let boundary = if limit == 0 {
            None
        } else if let Some(sep) = separator {
            let mut windows = rest.windows(sep.len());
            if reverse {
                windows.rposition(|s| s == sep)
            } else {
                windows.position(|s| s == sep)
            }
        } else if reverse {
            rest.iter().rposition(whitespace)
        } else {
            rest.iter().position(whitespace)
        };
        let Some(at) = boundary else {
            push(rest);
            break;
        };
        let left = &rest[..at];
        let right = &rest[at + separator.map_or(1, <[u32]>::len)..];
        if reverse {
            push(right);
            rest = left;
        } else {
            push(left);
            rest = right;
        }
        limit -= 1;
    }
}

/// Work in UTF-8 bytes for scalar strings and Python code points for surrogate strings.
fn remove_affix(ctx: &mut Context, receiver: Id, name: &str, affix: Id) -> Result<Id> {
    fn remainder<T: PartialEq>(
        source: &[T],
        affix: &[T],
        prefix: bool,
    ) -> Option<std::ops::Range<usize>> {
        if affix.is_empty() {
            return None;
        }
        if prefix {
            source
                .starts_with(affix)
                .then_some(affix.len()..source.len())
        } else {
            source
                .ends_with(affix)
                .then(|| 0..source.len() - affix.len())
        }
    }

    let storage = ctx
        .string_storage(receiver)
        .expect("validated str receiver");
    let needle = ctx.string_storage(affix).ok_or_else(|| {
        let kind = if affix == NONE {
            "None"
        } else {
            ctx.type_name(affix)
        };
        let kind = &kind[..kind.floor_char_boundary(50)];
        type_error(format!("{name}() argument must be str, not {kind}"))
    })?;
    let prefix = name == "removeprefix";
    match (ctx.heap.get(storage), ctx.heap.get(needle)) {
        (Value::Str(source), Value::Str(affix)) => {
            if let Some(range) = remainder(source.as_bytes(), affix.as_bytes(), prefix) {
                let part = crate::Str::try_new(&source[range])
                    .map_err(|_| Error::new("MemoryError", ""))?;
                return Ok(ctx.text(part));
            }
        }
        (Value::SurrogateStr(source), _) => {
            let affix = ctx.string_codepoints(needle)?;
            if let Some(range) = remainder(source, &affix, prefix) {
                let points = super::super::copy_ids(&source[range])?;
                return Ok(ctx.text_codepoints(points));
            }
        }
        // A string with a surrogate cannot be contained in a scalar string.
        (Value::Str(_), Value::SurrogateStr(_)) => {}
        _ => unreachable!("validated str operands"),
    }
    ctx.unchanged_string(receiver, storage)
}

/// Strip matching Python code points while keeping scalar strings in UTF-8.
fn strip_string(ctx: &mut Context, receiver: Id, name: &str, chars: Option<Id>) -> Result<Id> {
    let chars = chars
        .map(|id| {
            ctx.string_storage(id)
                .map(|storage| ctx.heap.get(storage))
                .ok_or_else(|| type_error(format!("{name} arg must be None or str")))
        })
        .transpose()?;
    let remove = |point| match chars {
        None => char::from_u32(point).is_some_and(python_whitespace),
        Some(Value::Str(set)) => char::from_u32(point).is_some_and(|ch| set.contains(ch)),
        Some(Value::SurrogateStr(set)) => set.contains(&point),
        _ => unreachable!("validated str cutset"),
    };
    let storage = ctx
        .string_storage(receiver)
        .expect("validated str receiver");
    match ctx.heap.get(storage) {
        Value::Str(source) => {
            let matches = |ch: char| remove(u32::from(ch));
            let part = match name {
                "lstrip" => source.trim_start_matches(matches),
                "rstrip" => source.trim_end_matches(matches),
                _ => source.trim_matches(matches),
            };
            if part.len() == source.len() {
                if receiver == storage || source.is_empty() {
                    return Ok(storage);
                }
                // An unchanged nonempty str subclass produces a fresh exact str.
                return ctx.heap.fresh_string(Value::Str(part.into()));
            }
            Ok(ctx.text(part.to_owned()))
        }
        Value::SurrogateStr(points) => {
            let mut start = 0;
            let mut end = points.len();
            while name != "rstrip" && start < end && remove(points[start]) {
                start += 1;
            }
            while name != "lstrip" && start < end && remove(points[end - 1]) {
                end -= 1;
            }
            if receiver == storage && start == 0 && end == points.len() {
                return Ok(storage);
            }
            Ok(ctx.text_codepoints(points[start..end].to_vec()))
        }
        _ => unreachable!("validated str receiver"),
    }
}

/// Keep unpaired surrogates as line content without converting them to UTF-8.
fn split_string_lines(ctx: &mut Context, receiver: Id, keep: bool) -> Result<Id> {
    let storage = ctx
        .string_storage(receiver)
        .expect("validated str receiver");
    let mut values = Vec::new();
    if matches!(ctx.heap.get(storage), Value::SurrogateStr(_)) {
        let mut start = 0;
        loop {
            let points = ctx.string_codepoints(storage)?;
            let Some(field) = codepoint_line(&points, &mut start, keep) else {
                break;
            };
            let line = if receiver == storage && field.len() == points.len() {
                storage
            } else {
                let field = super::super::copy_ids(field)?;
                ctx.text_codepoints(field)
            };
            values.push(line);
        }
    } else {
        let source = ctx.str_value(storage)?.to_owned();
        split_lines(&source, keep, |field| {
            values.push(if receiver == storage && field.len() == source.len() {
                storage
            } else {
                ctx.text(field)
            });
        });
    }
    Ok(ctx.alloc(Value::List(values)))
}

/// Partition at a Python character boundary, retaining the separator object on a hit.
fn partition_string(ctx: &mut Context, receiver: Id, separator: Id, reverse: bool) -> Result<Id> {
    let storage = ctx
        .string_storage(receiver)
        .expect("validated string receiver");
    let needle = ctx
        .string_storage(separator)
        .ok_or_else(|| type_error(format!("must be str, not {}", ctx.type_name(separator))))?;
    let scalar = |id| matches!(ctx.heap.get(id), Value::Str(_));
    let halves = if scalar(storage) && scalar(needle) {
        crate::objects::partition_text(ctx.str_value(storage)?, ctx.str_value(needle)?, reverse)?
            .map(|(left, right)| (left.to_owned(), right.to_owned()))
            .map(|(left, right)| (ctx.text(left), ctx.text(right)))
    } else {
        let source = ctx.string_codepoints(storage)?;
        let sep = ctx.string_codepoints(needle)?;
        if sep.is_empty() {
            return Err(Error::new("ValueError", "empty separator"));
        }
        let mut windows = source.windows(sep.len());
        let found = if reverse {
            windows.rposition(|w| w == sep.as_ref())
        } else {
            windows.position(|w| w == sep.as_ref())
        };
        let halves = found.map(|i| (source[..i].to_vec(), source[i + sep.len()..].to_vec()));
        halves.map(|(left, right)| (ctx.text_codepoints(left), ctx.text_codepoints(right)))
    };
    let fields = if let Some((left, right)) = halves {
        vec![left, separator, right]
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

fn split_string(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [sep, max] = bind(name, args, kwargs, ["sep", "maxsplit"])?;
    let limit = split_limit(ctx, max)?;
    let sep = sep
        .filter(|&v| v != NONE)
        .map(|v| {
            ctx.string_storage(v)
                .ok_or_else(|| type_error(format!("must be str or None, not {}", ctx.type_name(v))))
        })
        .transpose()?;
    let storage = ctx
        .string_storage(receiver)
        .expect("validated str receiver");
    if receiver != storage
        && let Some(sep) = sep
    {
        // CPython returns the original subtype when the separator cannot fit its length or kind.
        let source = ctx.string_codepoints(storage)?;
        let needle = ctx.string_codepoints(sep)?;
        let kind = |s: &[u32]| match s.iter().max().copied().unwrap_or(0) {
            0..=0xff => 1,
            0x100..=0xffff => 2,
            _ => 4,
        };
        if needle.len() > source.len() || kind(&needle) > kind(&source) {
            return Ok(ctx.alloc(Value::List(vec![receiver])));
        }
    }
    let reverse = name == "rsplit";
    let reuse_whole = receiver == storage && (sep.is_some() || limit != 0);
    let surrogate = |id| matches!(ctx.heap.get(id), Value::SurrogateStr(_));
    let mut values = Vec::new();
    if surrogate(storage) || sep.is_some_and(surrogate) {
        let source = ctx.string_codepoints(storage)?.into_owned();
        let needle = sep
            .map(|id| ctx.string_codepoints(id).map(Cow::into_owned))
            .transpose()?;
        if needle.as_deref() == Some(&[][..]) {
            return Err(Error::new("ValueError", "empty separator"));
        }
        split_codepoints(&source, needle.as_deref(), limit, reverse, |field| {
            values.push(if reuse_whole && field.len() == source.len() {
                storage
            } else {
                ctx.text_codepoints(field.to_vec())
            });
        });
    } else {
        let source = ctx.str_value(storage)?.to_owned();
        let needle = sep
            .map(|id| ctx.str_value(id).map(str::to_owned))
            .transpose()?;
        split_parts(&source, needle.as_deref(), limit, reverse, |field| {
            values.push(if reuse_whole && field.len() == source.len() {
                storage
            } else {
                ctx.text(field)
            });
        })
        .map_err(|msg| Error::new("ValueError", msg))?;
    }
    if reverse {
        values.reverse();
    }
    Ok(ctx.alloc(Value::List(values)))
}

/// Materialize the iterable before inspecting its buffers, as PySequence_Fast does for bytes.join.
fn bytes_join(ctx: &mut Context, receiver: Id, iterable: Id) -> Result<Id> {
    let sequence = if matches!(ctx.heap.get(iterable), Value::List(_) | Value::Tuple(_)) {
        iterable
    } else {
        let iter = ctx.iterator(iterable).map_err(|error| {
            let ty = ctx.builtin("TypeError");
            if ctx.raised_error_matches(&error, ty) {
                type_error("can only join an iterable")
            } else {
                error
            }
        })?;
        ctx.heap.keep(iter);
        let sequence = ctx.alloc(Value::List(Vec::new()));
        ctx.heap.keep(sequence);
        list_extend(ctx, sequence, iter)?;
        sequence
    };
    let (Value::List(items) | Value::Tuple(items)) = ctx.heap.get(sequence) else {
        unreachable!()
    };
    if let [item] = items.as_slice()
        && matches!(ctx.heap.get(*item), Value::Bytes(_))
    {
        return Ok(*item);
    }
    let separator = ctx.bytes_like(receiver)?;
    let too_long = || Error::new("OverflowError", "join() result is too long");
    let mut size = separator
        .len()
        .checked_mul(items.len().saturating_sub(1))
        .ok_or_else(too_long)?;
    let mut buffers = Vec::new();
    buffers
        .try_reserve_exact(items.len())
        .map_err(|_| crate::Error::memory_error())?;
    for (i, &item) in items.iter().enumerate() {
        ctx.check_buffer_exporter(item)?;
        let buffer = match ctx.heap.get(ctx.scalar_value(item)) {
            Value::Array(array) => array.as_bytes(),
            _ => ctx.bytes_like(item).map_err(|_| {
                let name = ctx.type_name(item);
                type_error(format!(
                    "sequence item {i}: expected a bytes-like object, {} found",
                    &name[..name.floor_char_boundary(80)]
                ))
            })?,
        };
        size = size.checked_add(buffer.len()).ok_or_else(too_long)?;
        buffers.push(buffer);
    }
    if size > isize::MAX as usize {
        return Err(too_long());
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(size)
        .map_err(|_| crate::Error::memory_error())?;
    for (i, buffer) in buffers.into_iter().enumerate() {
        if i != 0 {
            result.extend_from_slice(separator);
        }
        result.extend_from_slice(buffer);
    }
    Ok(ctx.alloc(Value::Bytes(result)))
}

// Rust strings cannot hold lone surrogates. Match these inputs in Python code points.
fn replace_points(ctx: &mut Context, receiver: Id, old: Id, new: Id, limit: usize) -> Result<Id> {
    let value = ctx.string_codepoints(receiver)?;
    let needle = ctx.string_codepoints(old)?;
    let replacement = ctx.string_codepoints(new)?;
    let (value, needle, replacement) = (value.as_ref(), needle.as_ref(), replacement.as_ref());
    let matches = || {
        let mut next = 0;
        (0..=value.len())
            .filter(move |&i| {
                if i < next || !value[i..].starts_with(needle) {
                    return false;
                }
                next = i + needle.len();
                true
            })
            .take(limit)
    };
    let count = matches().count();
    if count == 0 || old == new || (needle.is_empty() && replacement.is_empty()) {
        return Ok(ctx.string_storage(receiver).unwrap());
    }
    let retained = value.len() - count * needle.len();
    let memory_error = || Error::new("MemoryError", "");
    let size = replacement
        .len()
        .checked_mul(count)
        .and_then(|extra| retained.checked_add(extra))
        .ok_or_else(memory_error)?;
    let mut result = Vec::new();
    result.try_reserve_exact(size).map_err(|_| memory_error())?;
    let mut copied = 0;
    for i in matches() {
        result.extend_from_slice(&value[copied..i]);
        result.extend_from_slice(replacement);
        copied = i + needle.len();
    }
    result.extend_from_slice(&value[copied..]);
    Ok(ctx.text_codepoints(result))
}

/// Replace nonoverlapping bytes, keeping both arguments exported during count conversion.
fn binary_replace(
    ctx: &mut Context,
    receiver: Id,
    old: Id,
    new: Id,
    count: Option<Id>,
) -> Result<Id> {
    for buffer in [old, new] {
        ctx.check_buffer_exporter(buffer)?;
        ctx.native_buffer_bytes(buffer)?;
    }
    let limit = ctx.with_buffer_export(old, |ctx| {
        ctx.with_buffer_export(new, |ctx| {
            let mark = ctx.heap.temporary_mark();
            let result = split_limit(ctx, count);
            // A temporary __index__ result can finalize before either export is released.
            let collect = ctx.heap.discard_truth_operands(mark, ctx.modules);
            if let Err(error) = &result {
                for root in error.roots() {
                    ctx.heap.keep(root);
                }
            }
            if collect {
                ctx.collect_unreachable();
            }
            result
        })
    })?;
    // __index__ may change the contents of either operand or resize a separate receiver.
    let source = ctx.bytes_like(receiver)?;
    let old = ctx.native_buffer_bytes(old)?;
    let new = ctx.native_buffer_bytes(new)?;
    let matches = memchr::memmem::Finder::new(old);
    let positions = || matches.find_iter(source).take(limit);
    let count = if old.is_empty() && new.is_empty() {
        0
    } else {
        positions().count()
    };
    let exact_bytes = matches!(ctx.heap.get(receiver), Value::Bytes(_));
    if count == 0 && exact_bytes {
        return Ok(receiver);
    }
    let retained = source.len() - count * old.len();
    let size = new
        .len()
        .checked_mul(count)
        .and_then(|extra| retained.checked_add(extra))
        .filter(|&len| len <= isize::MAX as usize)
        .ok_or_else(|| Error::new("OverflowError", "replace bytes is too long"))?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(size)
        .map_err(|_| crate::Error::memory_error())?;
    let mut copied = 0;
    for start in positions().take(count) {
        result.extend_from_slice(&source[copied..start]);
        result.extend_from_slice(new);
        copied = start + old.len();
    }
    result.extend_from_slice(&source[copied..]);
    Ok(ctx.alloc(
        if matches!(ctx.heap.get(ctx.scalar_value(receiver)), Value::Bytes(_)) {
            Value::Bytes(result)
        } else {
            Value::ByteArray(result)
        },
    ))
}

/// Justify a binary sequence after converting the width, which may mutate either argument.
fn binary_justify(
    ctx: &mut Context,
    owner: &str,
    receiver: Id,
    name: &str,
    width: usize,
    fill: Option<Id>,
) -> Result<Id> {
    let fill = if let Some(id) = fill {
        let storage = ctx.scalar_value(id);
        match ctx.heap.get(storage) {
            Value::Bytes(bytes) | Value::ByteArray(bytes) => match bytes.as_slice() {
                [byte] => *byte,
                _ => {
                    return Err(type_error(format!(
                        "{name}(): argument 2 must be a byte string of length 1, not a {} object of length {}",
                        ctx.type_name(storage),
                        bytes.len()
                    )));
                }
            },
            _ => {
                let kind = if id == NONE {
                    "None"
                } else {
                    ctx.type_name(id)
                };
                return Err(type_error(format!(
                    "{name}() argument 2 must be a byte string of length 1, not {kind:.50}"
                )));
            }
        }
    } else {
        b' '
    };
    if owner == "bytes" && width > crate::MAX_BYTES_LEN {
        return Err(Error::new("OverflowError", "byte string is too large"));
    }
    let storage = ctx.scalar_value(receiver);
    let source = ctx.bytes_like(storage)?;
    if owner == "bytes" && receiver == storage && width <= source.len() {
        return Ok(receiver);
    }
    let len = source.len().max(width);
    let mut padded = Vec::new();
    padded
        .try_reserve_exact(len)
        .map_err(|_| crate::Error::allocation())?;
    if name == "rjust" {
        padded.resize(len - source.len(), fill);
    }
    padded.extend_from_slice(source);
    padded.resize(len, fill);
    Ok(ctx.alloc(if owner == "bytes" {
        Value::Bytes(padded)
    } else {
        Value::ByteArray(padded)
    }))
}

/// Justify binary data by byte count or Unicode text by code point count.
fn justify(
    ctx: &mut Context,
    owner: &str,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "{owner}.{name}() takes no keyword arguments"
        )));
    }
    let Some((&width, extra)) = args.split_first() else {
        return Err(type_error(format!(
            "{name} expected at least 1 argument, got 0"
        )));
    };
    if extra.len() > 1 {
        return Err(type_error(format!(
            "{name} expected at most 2 arguments, got {}",
            args.len()
        )));
    }
    let width = crate::objects::zfill::width(&ctx.index_integer(width)?)?;
    if owner != "str" {
        return binary_justify(ctx, owner, receiver, name, width, extra.first().copied());
    }
    let fill = if let Some(&id) = extra.first() {
        match ctx.heap.get(ctx.scalar_value(id)) {
            Value::Str(s) => s
                .chars()
                .next()
                .filter(|c| c.len_utf8() == s.len())
                .map(u32::from),
            Value::SurrogateStr(points) => match points.as_slice() {
                [point] => Some(*point),
                _ => None,
            },
            _ => {
                return Err(type_error(format!(
                    "The fill character must be a unicode character, not {}",
                    ctx.type_name(id)
                )));
            }
        }
        .ok_or_else(|| type_error("The fill character must be exactly one character long"))?
    } else {
        u32::from(' ')
    };
    justify_string(ctx, receiver, width, fill, name)
}

fn justify_string(
    ctx: &mut Context,
    receiver: Id,
    width: usize,
    fill: u32,
    name: &str,
) -> Result<Id> {
    let storage = ctx.string_storage(receiver).unwrap();
    let source = ctx.heap.get(storage);
    let len = match source {
        Value::Str(s) => s.chars().count(),
        Value::SurrogateStr(s) => s.len(),
        _ => unreachable!(),
    };
    let margin = width.saturating_sub(len);
    if margin == 0 && (receiver == storage || len == 0) {
        return Ok(storage);
    }
    let left = match name {
        "ljust" => 0,
        "rjust" => margin,
        "format-center" => margin / 2,
        // CPython assigns the extra character on the left only for an odd target width.
        _ => margin / 2 + (margin & width & 1),
    };
    let memory_error = || Error::new("MemoryError", "");
    let result = if let Value::Str(s) = source
        && let Some(ch) = char::from_u32(fill).or((margin == 0).then_some(' '))
    {
        Value::Str(crate::objects::scalar_format::padded_text(
            s,
            ch,
            left,
            margin - left,
        )?)
    } else {
        let mut points = Vec::new();
        points
            .try_reserve_exact(len + margin)
            .map_err(|_| memory_error())?;
        points.resize(left, fill);
        match source {
            Value::Str(s) => points.extend(s.chars().map(u32::from)),
            Value::SurrogateStr(s) => points.extend_from_slice(s),
            _ => unreachable!(),
        }
        points.resize(len + margin, fill);
        Value::SurrogateStr(points)
    };
    ctx.heap.fresh_string(result)
}

/// Partition immutable bytes, preserving the separator's exported object on a hit.
fn partition_bytes(ctx: &mut Context, receiver: Id, separator: Id) -> Result<Id> {
    let locate = |ctx: &Context, buffer| {
        let needle = ctx.native_buffer_bytes(buffer)?;
        if needle.is_empty() {
            return Err(Error::new("ValueError", "empty separator"));
        }
        Ok(memchr::memmem::find(ctx.bytes_like(receiver)?, needle)
            .map(|start| start..start + needle.len()))
    };
    let method = ctx.python_buffer_method(separator, true)?;
    let range = if let Some(method) = method {
        ctx.with_contiguous_python_buffer(separator, method, 0, |ctx, view| locate(ctx, view))?
    } else {
        locate(ctx, separator)?
    };
    let fields = if let Some(range) = range {
        let source = ctx.bytes_like(receiver)?;
        let before = super::super::copy_ids(&source[..range.start])?;
        let after = super::super::copy_ids(&source[range.end..])?;
        // Py_buffer.obj for a Python exporter is its wrapper, emptied on release.
        let middle = if method.is_some() {
            ctx.alloc(Value::BufferWrapper)
        } else {
            separator
        };
        vec![
            ctx.heap.bytes_from_data(before),
            middle,
            ctx.heap.bytes_from_data(after),
        ]
    } else {
        let empty = ctx.heap.bytes_from_data(Vec::new());
        vec![receiver, empty, empty]
    };
    Ok(ctx.alloc(Value::Tuple(fields)))
}

/// Decode a boxed replacement, borrowing its text when lookup cannot run Python.
fn translation_value<'a, T>(
    ctx: &'a Context,
    item: Id,
    text: impl FnOnce(&'a str) -> Result<T>,
) -> Result<translate::Piece<T>> {
    match ctx.heap.get(ctx.scalar_value(item)) {
        Value::None => Ok(translate::Piece::Delete),
        Value::Str(value) => text(value).map(translate::Piece::Text),
        Value::Int(value) => translate::Piece::ordinal(value).map_err(Error::from),
        Value::Bool(value) => Ok(translate::Piece::Character(char::from(u8::from(*value)))),
        _ => Err(type_error(translate::INVALID_VALUE)),
    }
}

/// Subscribe with the integer ordinal, releasing each callback's result before the next lookup.
fn translation_item(ctx: &mut Context, table: Id, ch: char) -> Result<translate::Piece<String>> {
    let mark = ctx.heap.temporary_mark();
    let key = ctx.int(i64::from(u32::from(ch)));
    let item = if matches!(ctx.heap.get(table), Value::Dict(_)) {
        // A missing exact-dict key needs no Python exception or argument tuple.
        ctx.find_dictionary_key(table, key, None)
            .map(|(_, index)| index.map(|i| ctx.stored_entries(table)[i].1))
    } else {
        ctx.get_item(table, key).map(Some)
    };
    let item = match item {
        Ok(Some(item)) => Ok(item),
        Err(error) if !ctx.is_builtin_error(&error, "LookupError") => Err(error),
        Ok(None) | Err(_) => {
            ctx.discard_temporaries(mark, None);
            return Ok(translate::Piece::Character(ch));
        }
    };
    let item = ctx.release_with_result(mark, item)?;
    let piece = translation_value(ctx, item, super::super::copy_label);
    ctx.discard_temporaries(mark, None);
    piece
}

/// Try the shared loop with borrowed strings before resorting to Python subscription.
fn translate_str(ctx: &mut Context, receiver: Id, table: Id) -> Result<Id> {
    enum BorrowedError {
        Subscription,
        Error(Error),
    }
    impl From<crate::Error> for BorrowedError {
        fn from(error: crate::Error) -> Self {
            Self::Error(error.into())
        }
    }
    if let Value::Dict(entries) = ctx.heap.get(table) {
        let output = translate::borrowed_text(ctx.str_value(receiver)?, |ch| {
            match entries.lookup_ordinal(ctx, ch) {
                std::ops::ControlFlow::Continue(Some(item)) => {
                    translation_value(ctx, item, Ok).map_err(BorrowedError::Error)
                }
                std::ops::ControlFlow::Continue(None) => Ok(translate::Piece::Character(ch)),
                std::ops::ControlFlow::Break(()) => Err(BorrowedError::Subscription),
            }
        });
        match output {
            Ok(text) => return Ok(ctx.text(text.into_string())),
            Err(BorrowedError::Error(error)) => return Err(error),
            // The borrowed prefix made no Python calls. Restart before invoking any hook.
            Err(BorrowedError::Subscription) => {}
        }
    }
    let source = super::super::copy_label(ctx.str_value(receiver)?)?;
    let mut limit = ctx.heap.streaming_collection_limit();
    let text = translate::text(&source, |ch| {
        ctx.collect_streaming_garbage(&mut limit);
        translation_item(ctx, table, ch)
    })?;
    Ok(ctx.text(text.into_string()))
}

enum JoinInput<'a> {
    Iterable(Id),
    Formatted(&'a [Id]),
}

impl Context {
    /// Join the already formatted f-string fields without invoking string subclasses' addition.
    pub(in super::super) fn join_formatted(&mut self, values: &[Id]) -> Result<Id> {
        let empty = self.text("");
        join_strings(self, empty, JoinInput::Formatted(values))
    }
}

fn join_strings(ctx: &mut Context, receiver: Id, values: JoinInput<'_>) -> Result<Id> {
    let collected;
    let values = match values {
        JoinInput::Formatted(values) => values,
        JoinInput::Iterable(id) => match ctx.heap.get(id) {
            Value::List(items) | Value::Tuple(items) => items.as_slice(),
            _ => {
                collected = ctx.items(id)?;
                &collected
            }
        },
    };
    let mut content_bytes = Some(0usize);
    let mut surrogate = matches!(
        ctx.heap.get(ctx.scalar_value(receiver)),
        Value::SurrogateStr(_)
    );
    for (index, &item) in values.iter().enumerate() {
        let Some(storage) = ctx.string_storage(item) else {
            let class = ctx.exact_type(item)?;
            let name = ctx.class_name(class);
            return Err(type_error(format!(
                "sequence item {index}: expected str instance, {} found",
                &name[..name.floor_char_boundary(80)]
            )));
        };
        if values.len() == 1 {
            return Ok(storage);
        }
        if let Value::Str(text) = ctx.heap.get(storage) {
            content_bytes = content_bytes.and_then(|length| length.checked_add(text.len()));
        } else {
            surrogate = true;
        }
    }
    if surrogate {
        let sep = ctx.string_codepoints(receiver)?;
        let mut output = Vec::new();
        for (index, &item) in values.iter().enumerate() {
            let points = ctx.string_codepoints(item)?;
            if index != 0 {
                output
                    .try_reserve(sep.len())
                    .map_err(|_| crate::Error::memory_error())?;
                output.extend_from_slice(&sep);
            }
            output
                .try_reserve(points.len())
                .map_err(|_| crate::Error::memory_error())?;
            output.extend_from_slice(&points);
        }
        return Ok(ctx.text_codepoints(output));
    }
    let sep = ctx.str_value(receiver)?;
    if content_bytes == Some(0) && sep.is_empty() {
        return Ok(ctx.string_storage(receiver).unwrap());
    }
    let parts = values
        .iter()
        .map(|&item| ctx.str_value(item).expect("validated UTF-8 string"));
    let output = crate::objects::join_utf8(sep, content_bytes, parts)
        .ok_or_else(crate::Error::memory_error)?;
    Ok(ctx.text(output))
}

pub(super) fn method(
    ctx: &mut Context,
    owner: &str,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if owner == "str" && name == "maketrans" {
        return maketrans::table(ctx, args, kwargs);
    }
    if matches!(
        (owner, name),
        (
            "str",
            "__add__"
                | "__contains__"
                | "__eq__"
                | "__ne__"
                | "__getitem__"
                | "__hash__"
                | "__mod__"
                | "__rmod__"
        ) | ("bytes", "__mod__")
    ) {
        let (&receiver, args) = args.split_first().ok_or_else(|| {
            type_error(format!(
                "descriptor '{name}' of '{owner}' object needs an argument"
            ))
        })?;
        if !ctx.builtin_slot_accepts(receiver, owner) {
            return Err(type_error(format!(
                "descriptor '{name}' requires a '{owner}' object but received a '{}'",
                ctx.type_name(receiver)
            )));
        }
        if !kwargs.is_empty() {
            return Err(type_error(format!(
                "wrapper {name}() takes no keyword arguments"
            )));
        }
        if name == "__hash__" {
            if !args.is_empty() {
                return Err(type_error(format!(
                    "expected 0 arguments, got {}",
                    args.len()
                )));
            }
            let hash = ctx.dictionary_hash(ctx.scalar_value(receiver))?;
            return Ok(ctx.int(hash));
        }
        let &[other] = args else {
            return Err(type_error(format!(
                "expected 1 argument, got {}",
                args.len()
            )));
        };
        if name == "__getitem__" {
            return ctx.sequence_subscript(receiver, other);
        }
        if name == "__contains__" {
            return ctx.contains(other, ctx.scalar_value(receiver)).map(boolean);
        }
        if name == "__add__" {
            return ctx.concat_str(receiver, other);
        }
        if matches!(name, "__eq__" | "__ne__") {
            return Ok(match ctx.string_storage(other) {
                Some(other) => {
                    boolean(ctx.equal(ctx.scalar_value(receiver), other) == (name == "__eq__"))
                }
                None => NOT_IMPLEMENTED,
            });
        }
        if let Value::Bytes(format) = ctx.heap.get(ctx.scalar_value(receiver)) {
            return bytes_percent(ctx, &format.clone(), other);
        }
        let (format, value) = if name == "__rmod__" {
            if ctx.string_storage(other).is_none() {
                return Ok(NOT_IMPLEMENTED);
            }
            (other, receiver)
        } else {
            (receiver, other)
        };
        let format = ctx.str_value(format)?.to_owned();
        return percent(ctx, &format, value);
    }
    let Some((&receiver, args)) = args.split_first() else {
        return Err(type_error(format!(
            "unbound method {owner}.{name}() needs an argument"
        )));
    };
    let original_receiver = receiver;
    let receiver = if owner == "bytes"
        || (owner == "bytearray"
            && matches!(
                name,
                "endswith"
                    | "ljust"
                    | "lstrip"
                    | "replace"
                    | "rstrip"
                    | "split"
                    | "splitlines"
                    | "startswith"
                    | "strip"
            )) {
        let storage = ctx.scalar_value(receiver);
        if !matches!(
            (owner, ctx.heap.get(storage)),
            ("bytes", Value::Bytes(_)) | ("bytearray", Value::ByteArray(_))
        ) {
            return Err(type_error(format!(
                "descriptor '{name}' for '{owner}' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            )));
        }
        storage
    } else {
        receiver
    };
    if owner == "str" && ctx.string_storage(receiver).is_none() {
        return Err(type_error(format!(
            "descriptor '{name}' for 'str' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        )));
    }
    if name == "decode" || name == "encode" {
        let [encoding, errors] = bind(name, args, kwargs, ["encoding", "errors"])?;
        return if name == "decode" {
            decode_named(ctx, ctx.scalar_value(receiver), encoding, errors)
        } else {
            encode_text_object(ctx, receiver, encoding, errors)
        };
    }
    if matches!(
        (owner, name),
        ("str", "center" | "ljust" | "rjust")
            | ("bytes", "ljust" | "rjust")
            | ("bytearray", "ljust")
    ) {
        return justify(ctx, owner, original_receiver, name, args, kwargs);
    }
    if name == "zfill" {
        let width = one_positional_arg("str.zfill", args, kwargs)?;
        let width = crate::objects::zfill::width(&ctx.index_integer(width)?)?;
        let storage = ctx.string_storage(receiver).unwrap();
        let copy = storage != receiver;
        let result = match ctx.heap.get(storage) {
            Value::Str(text) => crate::objects::zfill::text(text, width, copy)?
                .map(|padded| Value::Str(padded.into())),
            Value::SurrogateStr(points) => {
                crate::objects::zfill::units(points, points.len(), width, copy)?
                    .map(Value::SurrogateStr)
            }
            _ => unreachable!(),
        };
        return match result {
            Some(value) => ctx.heap.fresh_string(value),
            None => Ok(storage),
        };
    }
    if name == "expandtabs" {
        let [tabsize] = bind_method_arguments(name, args, kwargs, ["tabsize"], 1)?;
        let width = match tabsize {
            None => 8,
            Some(id) => {
                let index = ctx.index_value(id)?.ok_or_else(|| {
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
    if matches!(
        (owner, name),
        ("str", "find" | "rfind" | "index" | "rindex" | "count")
            | ("bytes", "count" | "find" | "index" | "rfind")
            | ("bytes" | "bytearray", "startswith" | "endswith")
    ) {
        if !kwargs.is_empty() {
            return Err(type_error(format!(
                "{owner}.{name}() takes no keyword arguments"
            )));
        }
        let Some(&needle) = args.first() else {
            return Err(type_error(format!(
                "{name} expected at least 1 argument, got 0"
            )));
        };
        if args.len() > 3 {
            return Err(type_error(format!(
                "{name} expected at most 3 arguments, got {}",
                args.len()
            )));
        }
        if owner == "str" && ctx.string_storage(needle).is_none() {
            let kind = if needle == NONE {
                "None"
            } else {
                ctx.type_name(needle)
            };
            return Err(type_error(format!(
                "{name}() argument 1 must be str, not {kind}"
            )));
        }
        let (start, end) = search_bounds(ctx, args)?;
        if owner != "str" {
            let len = ctx.bytes_like(receiver)?.len() as i64;
            let index = |n: &crate::Int| {
                let n = n.to_i64().unwrap_or(if n.big().is_negative() {
                    i64::MIN
                } else {
                    i64::MAX
                });
                if n < 0 {
                    len.saturating_add(n).max(0)
                } else {
                    n
                }
            };
            let start = usize::try_from(start.as_ref().map_or(0, index)).unwrap_or(usize::MAX);
            let end = end.as_ref().map_or(len, index).min(len) as usize;
            if matches!(name, "startswith" | "endswith") {
                let tuple = ctx.tuple_items(needle)?;
                let candidates = tuple.as_deref().unwrap_or(&args[..1]);
                let value = ctx.bytes_like(receiver)?.get(start..end);
                for &candidate in candidates {
                    let affix = match ctx.heap.get(ctx.scalar_value(candidate)) {
                        Value::Array(array) => Ok(array.as_bytes()),
                        _ => ctx.bytes_like(candidate),
                    }
                    .map_err(|error| {
                        if tuple.is_some() {
                            error
                        } else {
                            type_error(format!(
                                "{name} first arg must be bytes or a tuple of bytes, not {}",
                                ctx.type_name(candidate)
                            ))
                        }
                    })?;
                    if value.is_some_and(|slice| {
                        if name == "startswith" {
                            slice.starts_with(affix)
                        } else {
                            slice.ends_with(affix)
                        }
                    }) {
                        return Ok(TRUE);
                    }
                }
                return Ok(FALSE);
            }
            ctx.check_buffer_exporter(needle)?;
            let byte;
            let needle = match ctx.heap.get(ctx.scalar_value(needle)) {
                Value::Bytes(_)
                | Value::ByteArray(_)
                | Value::MemoryView { .. }
                | Value::BytesIoBuffer { .. }
                | Value::Array(_) => ctx.native_buffer_bytes(needle)?,
                _ => {
                    let index = ctx.index_value(needle)?.ok_or_else(|| {
                        type_error(format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            ctx.type_name(needle)
                        ))
                    })?;
                    byte = index
                        .to_i64()
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or_else(|| Error::new("ValueError", "byte must be in range(0, 256)"))?;
                    std::slice::from_ref(&byte)
                }
            };
            let window = ctx.bytes_like(receiver)?.get(start..end);
            if name == "count" {
                let count = window.map_or(0, |v| match needle {
                    [] => v.len() + 1,
                    [byte] => memchr::memchr_iter(*byte, v).count(),
                    _ => memchr::memmem::find_iter(v, needle).count(),
                });
                return Ok(ctx.int(count as i64));
            }
            let offset = window.and_then(|v| {
                if name == "rfind" {
                    memchr::memmem::rfind(v, needle)
                } else {
                    memchr::memmem::find(v, needle)
                }
            });
            if name == "index" && offset.is_none() {
                return Err(Error::new("ValueError", "subsection not found"));
            }
            return Ok(ctx.int(offset.map_or(-1, |at| (start + at) as i64)));
        }
        if name == "count" {
            let value = match (
                ctx.heap.get(ctx.scalar_value(receiver)),
                ctx.heap.get(ctx.scalar_value(needle)),
            ) {
                (Value::Str(source), Value::Str(needle)) => {
                    crate::objects::count_text(source, needle, start.as_ref(), end.as_ref())
                }
                _ => crate::objects::count_codepoints(
                    &ctx.string_codepoints(receiver)?,
                    &ctx.string_codepoints(needle)?,
                    start.as_ref(),
                    end.as_ref(),
                ),
            };
            return Ok(ctx.alloc(Value::Int(value)));
        }
        let reverse = matches!(name, "rfind" | "rindex");
        let found = if [receiver, needle]
            .iter()
            .any(|&id| matches!(ctx.heap.get(ctx.scalar_value(id)), Value::SurrogateStr(_)))
        {
            crate::objects::find_codepoints(
                &ctx.string_codepoints(receiver)?,
                &ctx.string_codepoints(needle)?,
                start.as_ref(),
                end.as_ref(),
                reverse,
            )
        } else {
            crate::objects::find_text(
                ctx.str_value(receiver)?,
                ctx.str_value(needle)?,
                start.as_ref(),
                end.as_ref(),
                reverse,
            )
        };
        if matches!(name, "index" | "rindex") && found.as_small() == Some(-1) {
            return Err(Error::new("ValueError", "substring not found"));
        }
        return Ok(ctx.alloc(Value::Int(found)));
    }
    if owner == "bytes" && name == "removeprefix" {
        let method = format!("{}.{name}", ctx.type_name(original_receiver));
        let prefix = one_positional_arg(&method, args, kwargs)?;
        ctx.check_buffer_exporter(prefix)?;
        let prefix = ctx.native_buffer_bytes(prefix)?;
        let data = ctx.bytes_like(receiver)?;
        let result = data.strip_prefix(prefix).unwrap_or(data);
        if result.len() == data.len() && original_receiver == receiver {
            return Ok(receiver);
        }
        let bytes = super::super::copy_ids(result)?;
        return Ok(ctx.heap.bytes_from_data(bytes));
    }
    if matches!(owner, "str" | "bytes" | "bytearray")
        && matches!(name, "strip" | "lstrip" | "rstrip")
    {
        if !kwargs.is_empty() {
            return Err(type_error(format!(
                "{}.{name}() takes no keyword arguments",
                ctx.type_name(original_receiver)
            )));
        }
        if args.len() > 1 {
            return Err(type_error(format!(
                "{name} expected at most 1 argument, got {}",
                args.len()
            )));
        }
        let chars = args.first().copied().filter(|&id| id != NONE);
        if owner == "str" {
            return strip_string(ctx, receiver, name, chars);
        }
        let chars = chars
            .map(|id| match ctx.heap.get(ctx.scalar_value(id)) {
                Value::Array(array) => Ok(array.as_bytes()),
                _ => ctx.bytes_like(id),
            })
            .transpose()?
            .unwrap_or(b" \t\n\x0b\x0c\r");
        let immutable = owner == "bytes";
        let trimmed = crate::objects::strip_bytes(
            ctx.bytes_like(receiver)?,
            chars,
            name != "rstrip",
            name != "lstrip",
            immutable && original_receiver == receiver,
        )?;
        return Ok(match trimmed {
            Some(data) if immutable => ctx.alloc(Value::Bytes(data)),
            Some(data) => ctx.alloc(Value::ByteArray(data)),
            None => receiver,
        });
    }
    if matches!(owner, "bytes" | "bytearray") && name == "splitlines" {
        let [keepends] = bind_method_arguments(name, args, kwargs, ["keepends"], 1)?;
        // Truth conversion can mutate a bytearray receiver before it is split.
        let keep = ctx.optional_truth(keepends, false)?;
        let mut fields = Vec::new();
        let mut start = 0;
        loop {
            let data = ctx.bytes_like(receiver)?;
            let rest = &data[start..];
            if rest.is_empty() {
                break;
            }
            let end = memchr::memchr2(b'\r', b'\n', rest).unwrap_or(rest.len());
            let boundary = match &rest[end..] {
                [b'\r', b'\n', ..] => 2,
                [] => 0,
                _ => 1,
            };
            let field = &rest[..if keep { end + boundary } else { end }];
            let line =
                if owner == "bytes" && original_receiver == receiver && field.len() == data.len() {
                    receiver
                } else {
                    let copy = super::super::copy_ids(field)?;
                    ctx.alloc(if owner == "bytes" {
                        Value::Bytes(copy)
                    } else {
                        Value::ByteArray(copy)
                    })
                };
            fields
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            fields.push(line);
            start += end + boundary;
        }
        return Ok(ctx.alloc(Value::List(fields)));
    }
    if owner == "bytes" && name == "partition" {
        if !kwargs.is_empty() {
            return Err(type_error("bytes.partition() takes no keyword arguments"));
        }
        let &[separator] = args else {
            return Err(type_error(format!(
                "bytes.partition() takes exactly one argument ({} given)",
                args.len()
            )));
        };
        return partition_bytes(ctx, original_receiver, separator);
    }
    if matches!(owner, "bytes" | "bytearray") && matches!(name, "split" | "rsplit") {
        let [sep, max] = super::bind_method_arguments(name, args, kwargs, ["sep", "maxsplit"], 2)?;
        let limit = split_limit(ctx, max)?;
        let separator = sep
            .filter(|&id| id != NONE)
            .map(|id| match ctx.heap.get(ctx.scalar_value(id)) {
                Value::Array(array) => Ok(array.as_bytes()),
                _ => ctx.bytes_like(id),
            })
            .transpose()?;
        let immutable = owner == "bytes";
        let parts = crate::objects::split_bytes(
            ctx.bytes_like(receiver)?,
            separator,
            limit,
            name == "rsplit",
            immutable,
        )?;
        let fields = parts
            .into_iter()
            .map(|part| match part {
                None => receiver,
                Some(bytes) if immutable => ctx.alloc(Value::Bytes(bytes)),
                Some(bytes) => ctx.alloc(Value::ByteArray(bytes)),
            })
            .collect();
        return Ok(ctx.alloc(Value::List(fields)));
    }
    let predicate: Option<fn(&str) -> bool> = match name {
        "isalnum" => Some(crate::objects::python_alnum),
        "isascii" => Some(str::is_ascii),
        "isalpha" => Some(crate::objects::python_isalpha),
        "isdigit" => Some(crate::objects::python_isdigit),
        "isidentifier" => Some(crate::objects::python_isidentifier),
        "islower" => Some(|text| crate::objects::python_islower(text.chars())),
        "isnumeric" => Some(crate::objects::python_isnumeric),
        "isprintable" => Some(crate::objects::python_isprintable),
        "isspace" => Some(crate::objects::python_isspace),
        "istitle" => Some(|text| crate::objects::python_istitle(text.chars())),
        "isupper" => Some(|text| crate::objects::python_isupper(text.chars())),
        _ => None,
    };
    let mapping = match name {
        "lower" if owner == "str" => Some(Case::Lower),
        "casefold" if owner == "str" => Some(Case::Fold),
        "title" => Some(Case::Title),
        "swapcase" => Some(Case::Swap),
        _ => None,
    };
    if predicate.is_some() || mapping.is_some() {
        if !kwargs.is_empty() {
            return Err(type_error(format!(
                "str.{name}() takes no keyword arguments"
            )));
        }
        if !args.is_empty() {
            return Err(type_error(format!(
                "str.{name}() takes no arguments ({} given)",
                args.len()
            )));
        }
    }
    if let Some(case) = mapping {
        let storage = ctx.string_storage(receiver).unwrap();
        let mapped = match ctx.heap.get(storage) {
            Value::Str(text) if text.is_empty() => return ctx.heap.string_literal(""),
            Value::Str(text) => Value::Str(crate::objects::case_map(text, case)?),
            Value::SurrogateStr(points) => {
                Value::SurrogateStr(crate::objects::case_map_points(points, case)?)
            }
            _ => unreachable!(),
        };
        return ctx.heap.fresh_string(mapped);
    }
    if let Some(predicate) = predicate {
        if matches!(
            name,
            "isalnum"
                | "isascii"
                | "isalpha"
                | "isdigit"
                | "isidentifier"
                | "isnumeric"
                | "isprintable"
                | "isspace"
        ) && matches!(
            ctx.heap.get(ctx.scalar_value(receiver)),
            Value::SurrogateStr(_)
        ) {
            // This representation always contains at least one non-ASCII surrogate.
            return Ok(boolean(false));
        }
        if matches!(name, "islower" | "isupper")
            && let Value::SurrogateStr(points) = ctx.heap.get(ctx.scalar_value(receiver))
        {
            // Unpaired surrogates are uncased, so only scalar values contribute.
            let chars = points.iter().copied().filter_map(char::from_u32);
            return Ok(boolean(if name == "islower" {
                crate::objects::python_islower(chars)
            } else {
                crate::objects::python_isupper(chars)
            }));
        }
        if name == "istitle"
            && let Value::SurrogateStr(points) = ctx.heap.get(ctx.scalar_value(receiver))
        {
            // A surrogate ends a cased run, just like the uncased replacement character.
            return Ok(boolean(crate::objects::python_istitle(points.iter().map(
                |&cp| char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER),
            ))));
        }
        return Ok(boolean(predicate(ctx.str_value(receiver)?)));
    }
    if name == "translate" {
        positional(name, kwargs)?;
        arity(name, args, 1, 1)?;
        return translate_str(ctx, receiver, args[0]);
    }
    if name == "join" {
        if owner == "bytes" {
            return bytes_join(
                ctx,
                receiver,
                one_positional_arg("bytes.join", args, kwargs)?,
            );
        }
        positional(name, kwargs)?;
        arity(name, args, 1, 1)?;
        return join_strings(ctx, receiver, JoinInput::Iterable(args[0]));
    }
    if matches!(name, "startswith" | "endswith") {
        positional(name, kwargs)?;
        arity(name, args, 1, 3)?;
        let (start, end) = search_bounds(ctx, args)?;
        enum Window<'a> {
            Utf8(&'a str),
            Points(&'a [u32]),
        }
        let start = start.as_ref();
        let end = end.as_ref();
        let storage = ctx.scalar_value(receiver);
        let part = match ctx.heap.get(storage) {
            Value::Str(text) if ctx.heap.is_ascii_str(storage) => {
                crate::objects::indexed_search_bounds(text.len(), start, end)
                    .map(|bounds| Window::Utf8(&text[bounds]))
            }
            Value::Str(text) => {
                crate::objects::search_window(text, start, end).map(|(part, _)| Window::Utf8(part))
            }
            Value::SurrogateStr(points) => {
                crate::objects::codepoint_search_window(points, start, end)
                    .map(|(part, _)| Window::Points(part))
            }
            _ => return Err(type_error("expected str")),
        };
        let prefix = name == "startswith";
        let tuple = ctx.tuple_items(args[0])?;
        let candidates = tuple.as_deref().unwrap_or(&args[..1]);
        for &id in candidates {
            let Some(storage) = ctx.string_storage(id) else {
                let kind = ctx.type_name(id);
                return Err(type_error(if tuple.is_some() {
                    format!("tuple for {name} must only contain str, not {kind:.100}")
                } else {
                    format!("{name} first arg must be str or a tuple of str, not {kind:.100}")
                }));
            };
            let Some(part) = &part else {
                continue;
            };
            let matched = match (part, ctx.heap.get(storage), prefix) {
                (Window::Utf8(text), Value::Str(needle), true) => text.starts_with(needle.as_str()),
                (Window::Utf8(text), Value::Str(needle), false) => text.ends_with(needle.as_str()),
                // SurrogateStr storage always contains at least one surrogate.
                (Window::Utf8(_), Value::SurrogateStr(_), _) => false,
                (Window::Points(points), Value::SurrogateStr(needle), true) => {
                    points.starts_with(needle)
                }
                (Window::Points(points), Value::SurrogateStr(needle), false) => {
                    points.ends_with(needle)
                }
                (Window::Points(points), Value::Str(needle), _) => {
                    let mut input = points.iter().copied();
                    if prefix {
                        needle.chars().all(|ch| input.next() == Some(u32::from(ch)))
                    } else {
                        needle
                            .chars()
                            .rev()
                            .all(|ch| input.next_back() == Some(u32::from(ch)))
                    }
                }
                _ => unreachable!("validated str affix"),
            };
            if matched {
                return Ok(TRUE);
            }
        }
        return Ok(FALSE);
    }
    if name == "replace" && matches!(owner, "bytes" | "bytearray") {
        if !kwargs.is_empty() {
            return Err(type_error(format!(
                "{owner}.replace() takes no keyword arguments"
            )));
        }
        if args.len() < 2 {
            return Err(type_error(format!(
                "replace expected at least 2 arguments, got {}",
                args.len()
            )));
        }
        if args.len() > 3 {
            return Err(type_error(format!(
                "replace expected at most 3 arguments, got {}",
                args.len()
            )));
        }
        return binary_replace(
            ctx,
            original_receiver,
            args[0],
            args[1],
            args.get(2).copied(),
        );
    }
    if name == "replace" {
        let total = args.len() + kwargs.len();
        if total > 3 {
            let kind = if args.is_empty() { "keyword " } else { "" };
            return Err(type_error(format!(
                "replace() takes at most 3 {kind}arguments ({total} given)"
            )));
        }
        if args.len() < 2 {
            return Err(type_error(format!(
                "replace() takes at least 2 positional arguments ({} given)",
                args.len()
            )));
        }
        let [count] = bind_builtin_arguments(name, &args[2..], kwargs, ["count"], 0)?;
        for (i, &id) in args[..2].iter().enumerate() {
            if ctx.string_storage(id).is_none() {
                let kind = if id == NONE {
                    "None"
                } else {
                    ctx.type_name(id)
                };
                return Err(type_error(format!(
                    "replace() argument {} must be str, not {kind}",
                    i + 1
                )));
            }
        }
        let limit = split_limit(ctx, count)?;
        if [receiver, args[0], args[1]]
            .iter()
            .any(|&id| matches!(ctx.heap.get(ctx.scalar_value(id)), Value::SurrogateStr(_)))
        {
            return replace_points(ctx, receiver, args[0], args[1], limit);
        }
        let value = crate::objects::replace_text(
            ctx.str_value(receiver)?,
            ctx.str_value(args[0])?,
            ctx.str_value(args[1])?,
            limit,
            args[0] == args[1],
            |length| {
                let mut text = String::new();
                text.try_reserve_exact(length).ok()?;
                Some(text)
            },
        )?;
        return Ok(match value {
            Some(text) => ctx.text(text),
            None => ctx.string_storage(receiver).unwrap(),
        });
    }
    if owner == "str" && matches!(name, "removeprefix" | "removesuffix") {
        let method = format!("{}.{name}", ctx.type_name(receiver));
        let affix = one_positional_arg(&method, args, kwargs)?;
        return remove_affix(ctx, receiver, name, affix);
    }
    if owner == "str" && matches!(name, "split" | "rsplit") {
        return split_string(ctx, receiver, name, args, kwargs);
    }
    if owner == "str" && name == "splitlines" {
        let [keep] = bind_method_arguments(name, args, kwargs, ["keepends"], 1)?;
        let keep = ctx.optional_truth(keep, false)?;
        return split_string_lines(ctx, receiver, keep);
    }
    if owner == "str" && matches!(name, "partition" | "rpartition") {
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
        return partition_string(ctx, receiver, *separator, name == "rpartition");
    }
    let source = ctx.str_value(receiver)?.to_owned();
    match name {
        "format" => format_string(ctx, &source, FormatSource::Arguments(args, kwargs)),
        "format_map" => {
            if !kwargs.is_empty() {
                return Err(type_error("str.format_map() takes no keyword arguments"));
            }
            let [mapping] = args else {
                return Err(type_error(format!(
                    "str.format_map() takes exactly one argument ({} given)",
                    args.len()
                )));
            };
            format_string(ctx, &source, FormatSource::Mapping(*mapping))
        }
        "capitalize" | "upper" => {
            positional(name, kwargs)?;
            arity(name, args, 0, 0)?;
            Ok(ctx.text(match name {
                "capitalize" => crate::objects::capitalize(&source)?.into_string(),
                _ => source.to_uppercase(),
            }))
        }
        _ => Err(type_error(format!("unsupported string method {name}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic::ffi::new_context;

    #[test]
    fn unchanged_affixes_preserve_exact_strings_and_copy_subclasses() {
        let mut ctx = new_context(serde_json::json!({"paths": []}), ["imre"]);
        let base = ctx.builtin("str");
        let scope = ctx.scope("Text", None);
        let class = ctx
            .allocate_class("Text".into(), scope, vec![base])
            .unwrap();
        let empty = ctx.text("");
        let missing = ctx.text("absent");
        let exact = ctx.text("αβ".repeat(171));
        let single = ctx.text("a");
        let surrogate = ctx.text_codepoints(vec![0xd800, u32::from('β')]);

        for source in [empty, exact, single, surrogate] {
            let child = ctx.call(class, vec![source], Vec::new()).unwrap();
            for name in ["removeprefix", "removesuffix"] {
                for affix in [empty, missing] {
                    assert_eq!(
                        method(&mut ctx, "str", name, &[source, affix], &[]).unwrap(),
                        source
                    );
                    let copy = method(&mut ctx, "str", name, &[child, affix], &[]).unwrap();
                    assert_eq!(ctx.type_name(copy), "str");
                    assert_eq!(
                        ctx.string_codepoints(copy).unwrap(),
                        ctx.string_codepoints(source).unwrap()
                    );
                    assert_ne!(copy, child);
                    if source != empty {
                        assert_ne!(copy, source);
                        assert_ne!(
                            copy,
                            method(&mut ctx, "str", name, &[child, affix], &[]).unwrap()
                        );
                    } else {
                        assert_eq!(copy, empty);
                    }
                }
            }
        }
        let prefix = ctx.text_codepoints(vec![0xd800]);
        let suffix = ctx.text("β");
        let rest = method(&mut ctx, "str", "removeprefix", &[surrogate, prefix], &[]).unwrap();
        assert_eq!(ctx.str_value(rest).unwrap(), "β");
        let rest = method(&mut ctx, "str", "removesuffix", &[surrogate, suffix], &[]).unwrap();
        assert_eq!(&*ctx.string_codepoints(rest).unwrap(), &[0xd800]);
        let number = ctx.int(17);
        let error = method(&mut ctx, "str", "removeprefix", &[exact, number], &[]).unwrap_err();
        assert_eq!(
            error.message,
            "removeprefix() argument must be str, not int"
        );
    }
}
