//! Box the entries and stateless decoders of the native codec registry.
use std::collections::HashMap;

mod bytes_escape;
mod raw_unicode_escape;
mod surrogate_utf8;

use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::{Codec, CodecInfo, DecodeIssue, Object, StrEncoder, Utf16Order};

#[derive(Default)]
pub(in crate::dynamic) struct Registry {
    entries: HashMap<*const Object, Id>,
    handlers: HashMap<String, Id>,
    // Built-in and module-level decoders are shared across alias families.
    functions: HashMap<&'static str, Id>,
}

impl Registry {
    pub(in crate::dynamic) fn roots(&self) -> impl Iterator<Item = &Id> {
        // Each decoder is also reachable from its registry entry.
        self.entries.values().chain(self.handlers.values())
    }
}

fn type_error(message: impl Into<String>) -> Error {
    Error::new("TypeError", message)
}

/// Built-in handler objects are shared between the module and the per-interpreter registry.
fn handler_builtin(name: &str) -> Option<&'static str> {
    Some(match name {
        "strict" => "codecs.strict_errors",
        "ignore" => "codecs.ignore_errors",
        "replace" => "codecs.replace_errors",
        "xmlcharrefreplace" => "codecs.xmlcharrefreplace_errors",
        "backslashreplace" => "codecs.backslashreplace_errors",
        "namereplace" => "codecs.namereplace_errors",
        "surrogateescape" => "codecs.surrogateescape",
        "surrogatepass" => "codecs.surrogatepass",
        _ => return None,
    })
}

fn handler(ctx: &mut Context, name: &str) -> Result<Id> {
    if let Some(&id) = ctx.codec_entries.handlers.get(name) {
        return Ok(id);
    }
    if let Some(builtin) = handler_builtin(name) {
        return Ok(ctx.builtin(builtin));
    }
    Err(crate::objects::unknown_handler(name).into())
}

pub(super) fn error_registry(
    ctx: &mut Context,
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "_codecs.{function}() takes no keyword arguments"
        )));
    }
    let (name, callback) = match (function, args) {
        ("lookup_error", [name]) => (*name, None),
        ("register_error", [name, callback]) => (*name, Some(*callback)),
        ("lookup_error", _) => {
            return Err(type_error(format!(
                "_codecs.lookup_error() takes exactly one argument ({} given)",
                args.len()
            )));
        }
        _ => {
            return Err(type_error(format!(
                "register_error expected 2 arguments, got {}",
                args.len()
            )));
        }
    };
    let Value::Str(name_str) = ctx.heap.get(ctx.scalar_value(name)) else {
        let kind = if name == NONE {
            "None"
        } else {
            ctx.type_name(name)
        };
        let arg = if callback.is_some() {
            "argument 1"
        } else {
            "argument"
        };
        return Err(type_error(format!(
            "{function}() {arg} must be str, not {kind}"
        )));
    };
    if name_str.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    let name = name_str.clone();
    if let Some(callback) = callback {
        if !ctx.callable(callback) {
            return Err(type_error("handler must be callable"));
        }
        ctx.codec_entries
            .handlers
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        ctx.codec_entries.handlers.insert(name, callback);
        Ok(NONE)
    } else {
        handler(ctx, &name)
    }
}

/// Invoke a built-in handler retrieved by lookup_error, including under a registered alias.
pub(super) fn builtin_handler(
    ctx: &mut Context,
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "{function}() takes no keyword arguments"
        )));
    }
    let [exception] = args else {
        return Err(type_error(format!(
            "{function}() takes exactly one argument ({} given)",
            args.len()
        )));
    };
    let name = function.strip_suffix("_errors").unwrap_or(function);
    if name == "strict" {
        if !matches!(ctx.heap.get(*exception), Value::Raised(_)) {
            return Err(type_error("codec must pass exception instance"));
        }
        return ctx.op("raise", *exception, NONE, NONE);
    }
    let kind = ctx.type_name(*exception);
    if kind != "UnicodeDecodeError" || matches!(name, "xmlcharrefreplace" | "namereplace") {
        return Err(type_error(format!(
            "don't know how to handle {kind} in error callback"
        )));
    }
    let object = ctx.lookup_attr(*exception, "object")?;
    let start = ctx.lookup_attr(*exception, "start")?;
    let end = ctx.lookup_attr(*exception, "end")?;
    let (start, end) = (ctx.as_integer(start)?, ctx.as_integer(end)?);
    let pass_utf8 = if name == "surrogatepass" {
        let encoding = ctx.lookup_attr(*exception, "encoding")?;
        let encoding = ctx.str_value(encoding)?;
        ["utf-8", "utf_8", "utf8", "cp65001"]
            .iter()
            .any(|key| encoding.eq_ignore_ascii_case(key))
    } else {
        false
    };
    let Value::Bytes(bytes) = ctx.heap.get(ctx.scalar_value(object)) else {
        return Err(type_error(
            "UnicodeError 'object' attribute must be a bytes",
        ));
    };
    let len = bytes.len() as i64;
    let end = end.max(1).min(len) as usize;
    let start = start.max(0).min((len - 1).max(0)) as usize;
    let bad = bytes.get(start..end).unwrap_or_default();
    if name == "surrogatepass" {
        // The validator stops at the leading byte. The handler inspects the
        // complete three-byte representation, even when end - start == 1.
        if pass_utf8
            && let Some([0xed, b @ 0xa0..=0xbf, c @ 0x80..=0xbf]) = bytes.get(start..start + 3)
        {
            let codepoint = 0xd000 + (u32::from(*b & 0x3f) << 6) + u32::from(*c & 0x3f);
            let text = ctx.text_codepoints(vec![codepoint]);
            let index = ctx.alloc(Value::Int(crate::Int::from_usize(start + 3)));
            return Ok(ctx.alloc(Value::Tuple(vec![text, index])));
        }
        return ctx.op("raise", *exception, NONE, NONE);
    }
    if name == "surrogateescape" && bad.first().is_none_or(u8::is_ascii) {
        return ctx.op("raise", *exception, NONE, NONE);
    }
    if name == "surrogateescape" {
        // CPython's handler consumes at most four consecutive non-ASCII bytes.
        let replacement: Vec<_> = bad
            .iter()
            .take(4)
            .take_while(|b| !b.is_ascii())
            .map(|&b| 0xdc00 + u32::from(b))
            .collect();
        let next = start + replacement.len();
        let text = ctx.text_codepoints(replacement);
        let index = ctx.alloc(Value::Int(crate::Int::from_usize(next)));
        return Ok(ctx.alloc(Value::Tuple(vec![text, index])));
    }
    let mut replacement = String::new();
    crate::objects::decoding_error(&mut replacement, Some(name), bad, || unreachable!())?;
    let value = ctx.text(replacement);
    let position = ctx.alloc(Value::Int(crate::Int::from_usize(end)));
    Ok(ctx.alloc(Value::Tuple(vec![value, position])))
}

pub(super) fn lookup(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error("_codecs.lookup() takes no keyword arguments"));
    }
    let [name] = args else {
        return Err(type_error(format!(
            "_codecs.lookup() takes exactly one argument ({} given)",
            args.len()
        )));
    };
    let Value::Str(name) = ctx.heap.get(*name) else {
        let kind = if *name == NONE {
            "None"
        } else {
            ctx.type_name(*name)
        };
        return Err(type_error(format!(
            "lookup() argument must be str, not {kind}"
        )));
    };
    let entry = CodecInfo::lookup(name)?;
    let key = entry.as_ptr();
    if let Some(&id) = ctx.codec_entries.entries.get(&key) {
        return Ok(id);
    }
    ctx.codec_entries
        .entries
        .try_reserve(1)
        .map_err(|_| crate::Error::allocation())?;
    let codec = entry.codec();
    let decode = if matches!(codec, Codec::CodePage(_)) {
        // The charmap Codec.decode method belongs to this entry's codec instance.
        ctx.alloc(Value::CodecDecoder(codec))
    } else if let Some(&id) = ctx.codec_entries.functions.get(codec.name()) {
        id
    } else {
        ctx.codec_entries
            .functions
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        let id = ctx.alloc(Value::CodecDecoder(codec));
        ctx.codec_entries.functions.insert(codec.name(), id);
        id
    };
    let id = ctx.alloc(Value::CodecInfo { entry, decode });
    ctx.codec_entries.entries.insert(key, id);
    Ok(id)
}

pub(in crate::dynamic) fn encode_text(
    ctx: &mut Context,
    id: Id,
    encoding_id: Option<Id>,
    errors_id: Option<Id>,
) -> Result<Vec<u8>> {
    let encoding = encoding_id
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("utf-8");
    let errors = errors_id
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("strict");
    let encoder = StrEncoder::lookup(encoding, errors)?;
    if let Ok(text) = ctx.str_value(id) {
        return Ok(encoder.encode_str(text, errors)?);
    }
    let errors = errors.to_owned();
    match encoder {
        StrEncoder::Utf16(order) => encode_utf16_codepoints(ctx, id, order, &errors),
        StrEncoder::Registered(Codec::Utf8 { signature }) => {
            ctx.encode_utf8_codepoints(id, signature, &errors)
        }
        StrEncoder::Registered(codec)
            if matches!(errors.as_str(), "strict" | "surrogateescape")
                && !ctx.codec_entries.handlers.contains_key(&errors) =>
        {
            ctx.encode_single_byte_codepoints(id, codec, errors == "surrogateescape")
        }
        StrEncoder::Registered(codec)
            if matches!(errors.as_str(), "ignore" | "replace" | "backslashreplace") =>
        {
            // Apply the built-in handler to lone surrogates, then encode the scalar result.
            let replaced = ctx.encode_utf8_codepoints(id, false, &errors)?;
            let text = std::str::from_utf8(&replaced).expect("these handlers produce scalar text");
            Ok(StrEncoder::Registered(codec).encode_str(text, &errors)?)
        }
        _ => Err(Error::new(
            "NotImplementedError",
            "encoding non-scalar strings with this codec is unsupported",
        )),
    }
}

/// UTF-16 reports one surrogate at a time, even for consecutive high and low surrogates.
fn encode_utf16_codepoints(
    ctx: &mut Context,
    id: Id,
    order: Utf16Order,
    errors: &str,
) -> Result<Vec<u8>> {
    let points = ctx.string_codepoints(id)?;
    let mut bytes = Vec::new();
    for (i, &point) in points.iter().enumerate() {
        if let Some(ch) = char::from_u32(point) {
            order.append(&mut bytes, ch.encode_utf16(&mut [0; 2]).iter().copied())?;
            continue;
        }
        if ctx.codec_entries.handlers.contains_key(errors) {
            return Err(Error::new(
                "NotImplementedError",
                "custom UTF-16 encoding error handlers are unsupported",
            ));
        }
        match errors {
            "strict" | "surrogateescape" => {
                return Err(ctx.string_encoding_error(
                    id,
                    order.name(),
                    "surrogates not allowed",
                    point,
                    i..i + 1,
                ));
            }
            "ignore" => (),
            "replace" => order.append(&mut bytes, [u16::from(b'?')])?,
            "surrogatepass" => order.append(&mut bytes, [point as u16])?,
            "backslashreplace" | "namereplace" => order.append(
                &mut bytes,
                compact_str::format_compact!("\\u{point:04x}").encode_utf16(),
            )?,
            "xmlcharrefreplace" => order.append(
                &mut bytes,
                compact_str::format_compact!("&#{point};").encode_utf16(),
            )?,
            _ => return Err(crate::objects::unknown_handler(errors).into()),
        }
    }
    Ok(bytes)
}

/// codecs.encode/decode use the registered codec directly, bypassing methods on the input.
pub(super) fn transform(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [input, encoding, errors] = super::builtins::bind_builtin_arguments(
        name,
        args,
        kwargs,
        ["obj", "encoding", "errors"],
        1,
    )?;
    let input = input.expect("the binder requires obj");
    for (label, arg) in [("encoding", encoding), ("errors", errors)] {
        if let Some(arg) = arg {
            if ctx.string_storage(arg).is_none() {
                let kind = if arg == NONE {
                    "None"
                } else {
                    ctx.type_name(arg)
                };
                return Err(type_error(format!(
                    "{name}() argument '{label}' must be str, not {kind}"
                )));
            }
            if ctx.str_value(arg)?.contains('\0') {
                return Err(Error::new("ValueError", "embedded null character"));
            }
        }
    }
    if name == "decode" {
        return decode_named(ctx, ctx.scalar_value(input), encoding, errors);
    }
    let spelling = encoding
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("utf-8");
    let codec = CodecInfo::lookup(spelling)?.codec();
    if ctx.string_storage(input).is_none() {
        let encoder = match codec {
            Codec::Utf8 { .. } => "utf_8",
            Codec::Ascii => "ascii",
            Codec::Latin1 => "latin_1",
            Codec::CodePage(_) => "charmap",
        };
        return Err(type_error(format!(
            "{encoder}_encode() argument 1 must be str, not {}",
            ctx.type_name(input)
        )));
    }
    let result = encode_text(ctx, input, encoding, errors)?;
    Ok(ctx.alloc(Value::Bytes(result)))
}

/// Boxed decoding can represent lone surrogates from raw Unicode escapes.
pub(in crate::dynamic) fn decode_named(
    ctx: &mut Context,
    input: Id,
    encoding: Option<Id>,
    errors: Option<Id>,
) -> Result<Id> {
    let spelling = encoding
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("utf-8");
    if spelling.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    let key = crate::objects::normalize_codec(spelling)?;
    if key == "raw_unicode_escape" {
        let name = error_name(ctx, errors)?.to_owned();
        return ctx.with_buffer_export(input, |ctx| raw_unicode_escape::decode(ctx, input, &name));
    }
    let codec = Codec::by_key(&key, spelling)?;
    decode_bytes(ctx, codec, input, errors)
}

fn error_name(ctx: &Context, errors: Option<Id>) -> Result<&str> {
    let name = errors
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("strict");
    if name.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    Ok(name)
}

/// Regular codecs share the typed implementation. Registered Python callbacks reenter the native heap.
pub(in crate::dynamic) fn decode_bytes(
    ctx: &mut Context,
    codec: Codec,
    input: Id,
    errors: Option<Id>,
) -> Result<Id> {
    let name = error_name(ctx, errors)?;
    if let Codec::Utf8 { signature } = codec
        && name == "surrogateescape"
    {
        let bytes = ctx.bytes_like(input)?;
        let bytes = if signature {
            bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)
        } else {
            bytes
        };
        if let Ok(text) = std::str::from_utf8(bytes) {
            return Ok(ctx.text(text.to_owned()));
        }
        let points = crate::objects::interpreter_stdlib::fsdecode(bytes).collect();
        return Ok(ctx.text_codepoints(points));
    }
    if let Codec::Utf8 { signature } = codec
        && uses_python_handler(ctx, name)
    {
        let name = name.to_owned();
        return ctx.with_buffer_export(input, |ctx| registered_utf8(ctx, input, &name, signature));
    }
    let result = codec
        .decode(ctx.bytes_like(input)?, Some(name))
        .map_err(|err| ctx.decode_error(err))?;
    Ok(ctx.text(result))
}

pub(super) fn uses_python_handler(ctx: &Context, name: &str) -> bool {
    name == "surrogatepass"
        || !matches!(name, "ignore" | "replace" | "surrogateescape")
            && (handler_builtin(name).is_none() || ctx.codec_entries.handlers.contains_key(name))
}

/// URL decoding creates bytes for a single percent-decoded portion of a component.
pub(super) fn decode_slice(
    ctx: &mut Context,
    codec: Codec,
    data: &[u8],
    errors: &str,
) -> Result<String> {
    if let Codec::Utf8 { signature } = codec
        && uses_python_handler(ctx, errors)
    {
        if let Ok(valid) = std::str::from_utf8(data) {
            return Ok(if signature {
                valid.strip_prefix('\u{feff}').unwrap_or(valid)
            } else {
                valid
            }
            .to_owned());
        }
        let input = ctx.alloc(Value::Bytes(data.to_vec()));
        let result = registered_utf8(ctx, input, errors, signature)?;
        Ok(ctx.str_value(result)?.to_owned())
    } else {
        codec
            .decode(data, Some(errors))
            .map_err(|e| ctx.decode_error(e))
    }
}

fn registered_utf8(ctx: &mut Context, input: Id, name: &str, signature: bool) -> Result<Id> {
    let data = ctx.bytes_like(input)?;
    // The utf-8-sig codec passes the slice after the BOM to utf_8_decode.
    let data = if signature {
        data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data)
    } else {
        data
    };
    let mut text = String::new();
    let Some(issue) = crate::objects::utf8_prefix(&mut text, data, 0)? else {
        return Ok(ctx.text(text));
    };
    let points = text.chars().map(u32::from).collect();
    let error = issue.error(data);
    resume_decode(ctx, name, points, error, |points, data, position| {
        text.clear();
        let issue = crate::objects::utf8_prefix(&mut text, data, position)?;
        points
            .try_reserve(text.len())
            .map_err(|_| crate::Error::allocation())?;
        points.extend(text.chars().map(u32::from));
        Ok(issue)
    })
}

/// Resume from the position and possibly changed byte object supplied by the handler.
fn resume_decode(
    ctx: &mut Context,
    name: &str,
    mut points: Vec<u32>,
    error: crate::Error,
    mut prefix: impl FnMut(&mut Vec<u32>, &[u8], usize) -> Result<Option<DecodeIssue>>,
) -> Result<Id> {
    let raised = ctx.decode_error(error);
    if raised.decode.is_none() {
        return Err(raised);
    }
    let exception = ctx.alloc(Value::Raised(raised));
    let callback = handler(ctx, name)?;
    // One decode retains its first handler even if a callback replaces the registry entry.
    ctx.heap.keep(callback);
    loop {
        let response = ctx.call(callback, vec![exception], Vec::new())?;
        let invalid = || type_error("decoding error handler must return (str, int) tuple");
        let values = ctx.tuple_items(response)?.ok_or_else(invalid)?;
        let [replacement, newpos] = values.as_slice() else {
            return Err(invalid());
        };
        let (replacement, newpos) = (*replacement, *newpos);
        let replacement = ctx.string_codepoints(replacement).map_err(|_| invalid())?;
        points
            .try_reserve(replacement.len())
            .map_err(|_| crate::Error::allocation())?;
        points.extend_from_slice(&replacement);
        let position = ctx
            .index_integer(newpos)?
            .to_i64()
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?;
        let input = ctx.lookup_attr(exception, "object")?;
        let Value::Bytes(data) = ctx.heap.get(ctx.scalar_value(input)) else {
            return Err(type_error(
                "UnicodeError 'object' attribute must be a bytes",
            ));
        };
        let length = data.len() as isize;
        let position = if position < 0 {
            length + position
        } else {
            position
        };
        if !(0..=length).contains(&position) {
            return Err(Error::new(
                "IndexError",
                format!("position {position} from error handler out of bounds"),
            ));
        }
        let Some(issue) = prefix(&mut points, data, position as usize)? else {
            return Ok(ctx.text_codepoints(points));
        };
        ctx.advance_decode_error(exception, issue.start, issue.end, issue.reason);
    }
}

fn positional_decoder_args(
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<(Id, Option<Id>)> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "_codecs.{function}() takes no keyword arguments"
        )));
    }
    match args {
        [input] => Ok((*input, None)),
        [input, errors] => Ok((*input, Some(*errors))),
        [] => Err(type_error(format!(
            "{function} expected at least 1 argument, got 0"
        ))),
        _ => Err(type_error(format!(
            "{function} expected at most 2 arguments, got {}",
            args.len()
        ))),
    }
}

pub(super) fn escape_decode(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (input, errors) = positional_decoder_args("escape_decode", args, kwargs)?;
    ctx.with_buffer_export(input, |ctx| bytes_escape::decode(ctx, input, errors))
}

pub(in crate::dynamic) fn decode(
    ctx: &mut Context,
    codec: Codec,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let function = match codec {
        Codec::Latin1 => "latin_1_decode",
        Codec::Ascii => "ascii_decode",
        Codec::Utf8 { .. } => "utf_8_decode",
        Codec::CodePage(_) => "charmap_decode",
    };
    let (input, errors) = match codec {
        Codec::Latin1 | Codec::Ascii => positional_decoder_args(function, args, kwargs)?,
        _ => {
            let bound = matches!(codec, Codec::CodePage(_));
            let wrapper = if bound { "Codec.decode" } else { "decode" };
            let mut input = args.first().copied();
            let mut errors = args.get(1).copied();
            for (key, value) in kwargs {
                let slot = match key.as_str() {
                    "input" => &mut input,
                    "errors" => &mut errors,
                    _ => {
                        return Err(type_error(format!(
                            "{wrapper}() got an unexpected keyword argument '{key}'"
                        )));
                    }
                };
                if slot.replace(*value).is_some() {
                    return Err(type_error(format!(
                        "{wrapper}() got multiple values for argument '{key}'"
                    )));
                }
            }
            if args.len() > 2 {
                let offset = usize::from(bound);
                return Err(type_error(format!(
                    "{wrapper}() takes from {} to {} positional arguments but {} were given",
                    1 + offset,
                    2 + offset,
                    args.len() + offset
                )));
            }
            (
                input.ok_or_else(|| {
                    type_error(format!(
                        "{wrapper}() missing 1 required positional argument: 'input'"
                    ))
                })?,
                errors,
            )
        }
    };
    let consumed = ctx.bytes_like(input)?.len();
    let policy = match errors {
        None | Some(NONE) => None,
        Some(id) => {
            let Value::Str(name) = ctx.heap.get(ctx.scalar_value(id)) else {
                return Err(type_error(format!(
                    "{function}() argument 2 must be str or None, not {}",
                    ctx.type_name(id)
                )));
            };
            if name.contains('\0') {
                return Err(Error::new("ValueError", "embedded null character"));
            }
            Some(id)
        }
    };
    let text = decode_bytes(ctx, codec, input, policy)?;
    let length = ctx.alloc(Value::Int(crate::Int::from_usize(consumed)));
    Ok(ctx.alloc(Value::Tuple(vec![text, length])))
}
