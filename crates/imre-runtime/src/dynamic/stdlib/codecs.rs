//! Box the entries and stateless decoders of the native codec registry.
use std::collections::HashMap;

use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::{Codec, CodecInfo, Object};

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
    let Value::Bytes(bytes) = ctx.heap.get(object) else {
        return Err(type_error(
            "UnicodeError 'object' attribute must be a bytes",
        ));
    };
    let len = bytes.len() as i64;
    let end = end.max(1).min(len) as usize;
    let start = start.max(0).min((len - 1).max(0)) as usize;
    let bad = bytes.get(start..end).unwrap_or_default();
    if name == "surrogatepass"
        || (name == "surrogateescape" && bad.first().is_none_or(u8::is_ascii))
    {
        return ctx.op("raise", *exception, NONE, NONE);
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

/// Regular codecs share the typed implementation. Registered Python callbacks reenter the native heap.
pub(in crate::dynamic) fn decode_bytes(
    ctx: &mut Context,
    codec: Codec,
    input: Id,
    errors: Option<Id>,
) -> Result<String> {
    let name = errors
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("strict");
    if name.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    if let Codec::Utf8 { signature } = codec
        && uses_python_handler(ctx, name)
    {
        let name = name.to_owned();
        if matches!(ctx.heap.get(input), Value::ByteArray(_)) {
            ctx.bytearray_exports.push(input);
            let result = registered_utf8(ctx, input, &name, signature);
            ctx.bytearray_exports.pop();
            return result;
        }
        return registered_utf8(ctx, input, &name, signature);
    }
    codec
        .decode(ctx.bytes_like(input)?, Some(name))
        .map_err(|err| ctx.decode_error(err))
}

pub(super) fn uses_python_handler(ctx: &Context, name: &str) -> bool {
    !matches!(name, "ignore" | "replace" | "surrogateescape")
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
        registered_utf8(ctx, input, errors, signature)
    } else {
        codec
            .decode(data, Some(errors))
            .map_err(|e| ctx.decode_error(e))
    }
}

fn registered_utf8(ctx: &mut Context, input: Id, name: &str, signature: bool) -> Result<String> {
    let data = ctx.bytes_like(input)?;
    // The utf-8-sig codec passes the slice after the BOM to utf_8_decode.
    let data = if signature {
        data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data)
    } else {
        data
    };
    let mut text = String::new();
    let Some(issue) = crate::objects::utf8_prefix(&mut text, data, 0)? else {
        return Ok(text);
    };
    let raised = ctx.decode_error(issue.error(data));
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
        let values = match ctx.heap.get(ctx.scalar_value(response)) {
            Value::Tuple(values) | Value::NamedTuple { values, .. } => values,
            _ => return Err(invalid()),
        };
        let [replacement, newpos] = values.as_slice() else {
            return Err(invalid());
        };
        let (replacement, newpos) = (*replacement, *newpos);
        let Value::Str(replacement) = ctx.heap.get(ctx.scalar_value(replacement)) else {
            return Err(invalid());
        };
        text.try_reserve(replacement.len())
            .map_err(|_| crate::Error::allocation())?;
        text.push_str(replacement);
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
        let Value::Bytes(data) = ctx.heap.get(input) else {
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
        let Some(issue) = crate::objects::utf8_prefix(&mut text, data, position as usize)? else {
            return Ok(text);
        };
        ctx.advance_decode_error(exception, issue.start, issue.end, issue.reason);
    }
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
        Codec::Latin1 | Codec::Ascii => {
            if !kwargs.is_empty() {
                return Err(type_error(format!(
                    "_codecs.{function}() takes no keyword arguments"
                )));
            }
            match args {
                [input] => (*input, None),
                [input, errors] => (*input, Some(*errors)),
                [] => {
                    return Err(type_error(format!(
                        "{function} expected at least 1 argument, got 0"
                    )));
                }
                _ => {
                    return Err(type_error(format!(
                        "{function} expected at most 2 arguments, got {}",
                        args.len()
                    )));
                }
            }
        }
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
    let decoded = decode_bytes(ctx, codec, input, policy)?;
    let text = ctx.text(decoded);
    let length = ctx.alloc(Value::Int(crate::Int::from_usize(consumed)));
    Ok(ctx.alloc(Value::Tuple(vec![text, length])))
}
