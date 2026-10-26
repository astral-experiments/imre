//! Box the entries and stateless decoders of the native codec registry.
use std::collections::HashMap;

use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::{Codec, CodecInfo, Object};

#[derive(Default)]
pub(in crate::dynamic) struct Registry {
    entries: HashMap<*const Object, Id>,
    // Built-in and module-level decoders are shared across alias families.
    functions: HashMap<&'static str, Id>,
}

impl Registry {
    pub(in crate::dynamic) fn roots(&self) -> impl Iterator<Item = &Id> {
        // Each decoder is also reachable from its registry entry.
        self.entries.values()
    }
}

fn type_error(message: impl Into<String>) -> Error {
    Error::new("TypeError", message)
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
    let data = match ctx.heap.get(input) {
        Value::Bytes(data) | Value::ByteArray(data) => data,
        _ => {
            return Err(type_error(format!(
                "a bytes-like object is required, not '{}'",
                ctx.type_name(input)
            )));
        }
    };
    let policy = match errors {
        None | Some(NONE) => None,
        Some(id) => {
            let Value::Str(name) = ctx.heap.get(id) else {
                return Err(type_error(format!(
                    "{function}() argument 2 must be str or None, not {}",
                    ctx.type_name(id)
                )));
            };
            if name.contains('\0') {
                return Err(Error::new("ValueError", "embedded null character"));
            }
            Some(name.as_str())
        }
    };
    let consumed = data.len();
    let decoded = codec.decode(data, policy)?;
    let text = ctx.text(decoded);
    let length = ctx.alloc(Value::Int(crate::Int::from_usize(consumed)));
    Ok(ctx.alloc(Value::Tuple(vec![text, length])))
}
