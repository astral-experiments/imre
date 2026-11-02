//! Stateless codec calls preserve the Python input and result when the API permits it.
use super::{
    Context, Error, Id, NONE, Result, Value, decode_bytes, encode_text, error_name,
    lookup_transform, raw_unicode_escape, type_error, wide_unicode,
};
use crate::objects::{Codec, FixedUnicode, exceptions::Kind};

/// C string arguments arrive at the codec as an exact str, even for str subclasses.
fn spelling(ctx: &Context, value: Option<Id>, default: &str) -> Result<String> {
    let name = value
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or(default);
    if name.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    Ok(name.to_owned())
}

fn shortcut(key: &str) -> bool {
    matches!(
        key,
        "utf8"
            | "utf_8"
            | "utf16"
            | "utf_16"
            | "utf32"
            | "utf_32"
            | "ascii"
            | "us_ascii"
            | "latin1"
            | "latin_1"
            | "iso_8859_1"
            | "iso8859_1"
    )
}

/// Only an error from the callable acquires the CPython codec note.
fn codec_error(ctx: &mut Context, error: Error, operation: &str, encoding: &str) -> Error {
    let id = ctx.raised_instance(error);
    ctx.heap.keep(id);
    let action = if operation == "encode" {
        "encoding"
    } else {
        "decoding"
    };
    let note = ctx.text(format!("{action} with '{encoding}' codec failed"));
    if let Err(mut failure) = ctx.add_exception_note(id, note) {
        failure.context = id;
        return failure;
    }
    let Value::Raised(error) = ctx.heap.get(id) else {
        unreachable!("annotated codec error")
    };
    let mut error = error.clone();
    error.tracing().instance = Some(id);
    error
}

/// Resolve tuple slots directly; a tuple subclass may expose different attributes.
fn apply(
    ctx: &mut Context,
    entry: Id,
    operation: &str,
    input: Id,
    encoding: &str,
    errors: Option<Id>,
    text: bool,
) -> Result<Id> {
    ctx.heap.keep(entry);
    if text && !matches!(ctx.heap.get(entry), Value::Tuple(_)) {
        match ctx.lookup_attr(entry, "_is_text_encoding") {
            Ok(flag) if !ctx.truth(flag)? => {
                return Err(Error::new(
                    "LookupError",
                    format!(
                        "'{encoding}' is not a text encoding; use codecs.{operation}() to handle arbitrary codecs"
                    ),
                ));
            }
            Err(e) if !ctx.is_attribute_error(&e) => return Err(e),
            _ => {}
        }
    }
    let callable = if matches!(ctx.heap.get(entry), Value::CodecInfo { .. }) {
        ctx.lookup_attr(entry, operation)?
    } else {
        let items = ctx.tuple_items(entry)?.expect("verified codec tuple");
        items[usize::from(operation == "decode")]
    };
    let mut args = vec![input];
    if let Some(errors) = errors {
        let policy = spelling(ctx, Some(errors), "strict")?;
        args.push(ctx.text(policy));
    }
    let result = ctx
        .call(callable, args, Vec::new())
        .map_err(|error| codec_error(ctx, error, operation, encoding))?;
    ctx.tuple_items(result)?
        .filter(|items| items.len() == 2)
        .map(|items| items[0])
        .ok_or_else(|| {
            type_error(if operation == "encode" {
                "encoder must return a tuple (object, integer)"
            } else {
                "decoder must return a tuple (object,integer)"
            })
        })
}

/// Generic transforms accept arbitrary objects and return the first result unchanged.
pub(in crate::dynamic::stdlib) fn transform(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [input, encoding, errors] = super::super::builtins::bind_builtin_arguments(
        name,
        args,
        kwargs,
        ["obj", "encoding", "errors"],
        1,
    )?;
    let input = input.expect("the binder requires obj");
    for (label, arg) in [("encoding", encoding), ("errors", errors)] {
        if let Some(arg) = arg
            && ctx.string_storage(arg).is_none()
        {
            let kind = if arg == NONE {
                "None"
            } else {
                ctx.type_name(arg)
            };
            return Err(type_error(format!(
                "{name}() argument '{label}' must be str, not {kind}"
            )));
        }
    }
    let encoding = spelling(ctx, encoding, "utf-8")?;
    error_name(ctx, errors)?;
    let key = crate::objects::normalize_codec(&encoding)?;
    let wide = wide_unicode::encoding(&key);
    let native = name == "decode" && (wide.is_some() || key == "raw_unicode_escape");
    match lookup_transform(ctx, &encoding, native)? {
        Some(entry) => apply(ctx, entry, name, input, &encoding, errors, false),
        None => native_decode(ctx, wide, input, errors),
    }
}

/// Python-visible encoding preserves bytes subclass identities from custom encoders.
pub(in crate::dynamic) fn encode_text_object(
    ctx: &mut Context,
    input: Id,
    encoding: Option<Id>,
    errors: Option<Id>,
) -> Result<Id> {
    let name = spelling(ctx, encoding, "utf-8")?;
    error_name(ctx, errors)?;
    let key = crate::objects::normalize_codec(&name)?;
    let entry = if shortcut(&key) {
        None
    } else {
        lookup_transform(ctx, &name, FixedUnicode::by_key(&key).is_some())?
    };
    let Some(entry) = entry else {
        let data = encode_text(ctx, input, encoding, errors)?;
        return Ok(ctx.alloc(Value::Bytes(data)));
    };
    let result = apply(ctx, entry, "encode", input, &name, errors, true)?;
    match ctx.heap.get(ctx.scalar_value(result)) {
        Value::Bytes(_) => Ok(result),
        Value::ByteArray(_) => {
            ctx.emit_builtin_warning(Kind::RuntimeWarning, &format!(
                "encoder {name} returned bytearray instead of bytes; use codecs.encode() to encode to arbitrary types"
            ), 1)?;
            let data = ctx.bytes_like(result)?.to_vec();
            Ok(ctx.alloc(Value::Bytes(data)))
        }
        _ => Err(type_error(format!(
            "'{name}' encoder returned '{}' instead of 'bytes'; use codecs.encode() to encode to arbitrary types",
            ctx.type_name(result)
        ))),
    }
}

/// The default search provider implements wide and raw escape decoding without a CodecInfo.
fn native_decode(
    ctx: &mut Context,
    wide: Option<(FixedUnicode, bool)>,
    input: Id,
    errors: Option<Id>,
) -> Result<Id> {
    let policy = error_name(ctx, errors)?.to_owned();
    ctx.with_buffer_export(input, |ctx| match wide {
        Some((format, bom)) => wide_unicode::decode(ctx, input, &policy, format, bom),
        None => raw_unicode_escape::decode(ctx, input, &policy),
    })
}

/// bytes.decode supplies an anonymous, readonly byte view to a registered text codec.
pub(in crate::dynamic) fn decode_named(
    ctx: &mut Context,
    input: Id,
    encoding: Option<Id>,
    errors: Option<Id>,
) -> Result<Id> {
    let name = spelling(ctx, encoding, "utf-8")?;
    error_name(ctx, errors)?;
    let key = crate::objects::normalize_codec(&name)?;
    if ctx.bytes_like(input)?.is_empty() {
        return Ok(ctx.text(""));
    }
    let wide = wide_unicode::encoding(&key);
    if shortcut(&key) {
        return if wide.is_some() {
            native_decode(ctx, wide, input, errors)
        } else {
            decode_bytes(ctx, Codec::by_key(&key, &name)?, input, errors)
        };
    }
    ctx.with_buffer_export(input, |ctx| {
        let native = wide.is_some() || key == "raw_unicode_escape";
        let Some(entry) = lookup_transform(ctx, &name, native)? else {
            return native_decode(ctx, wide, input, errors);
        };
        let storage = ctx.scalar_value(input);
        let view = if let Value::ByteArray(bytes) = ctx.heap.get(storage) {
            ctx.ownerless_byte_view(storage, 0, bytes.len(), true)?
        } else {
            let owner = if matches!(ctx.heap.get(storage), Value::Bytes(_)) {
                storage
            } else {
                let bytes = ctx.bytes_like(input)?.to_vec();
                ctx.alloc(Value::Bytes(bytes))
            };
            ctx.alloc(Value::MemoryView {
                owner: Some(owner),
                anonymous: true,
                layout: None,
                readonly: true,
                hash: None,
            })
        };
        let result = apply(ctx, entry, "decode", view, &name, errors, true)?;
        if ctx.string_storage(result).is_none() {
            return Err(type_error(format!("'{name}' decoder returned '{}' instead of 'str'; use codecs.decode() to decode to arbitrary types", ctx.type_name(result))));
        }
        Ok(result)
    })
}
