//! Bind native URL quoting functions for runtime-imported modules.
use super::*;

fn octets(ctx: &Context, id: Id) -> Option<&[u8]> {
    match ctx.heap.get(ctx.scalar_value(id)) {
        Value::Bytes(b) | Value::ByteArray(b) => Some(b),
        _ => None,
    }
}

fn safe_bytes(ctx: &mut Context, safe: Option<Id>, default: &'static [u8]) -> Result<Vec<u8>> {
    let Some(id) = safe else {
        return Ok(default.to_vec());
    };
    if ctx.string_storage(id).is_some_and(|storage| storage != id) {
        let ascii = ctx.text("ascii");
        let ignore = ctx.text("ignore");
        let value = encode_text(ctx, id, ascii, ignore)?;
        return octets(ctx, value).map(<[u8]>::to_vec).ok_or_else(|| {
            Error::new(
                "TypeError",
                format!("can't concat {} to bytes", ctx.type_name(value)),
            )
        });
    }
    if let Value::Str(s) = ctx.heap.get(id) {
        return Ok(s.bytes().filter(u8::is_ascii).collect());
    }
    if let Some(bytes) = octets(ctx, id)
        && ctx.slot(id, "__iter__").is_none()
    {
        return Ok(bytes.iter().copied().filter(u8::is_ascii).collect());
    }
    let mut bytes = Vec::new();
    for item in ctx.items(id)? {
        let n = ctx.as_int(item)?;
        if n.cmp(&crate::Int::from(128)).is_lt() {
            let byte = n
                .to_i64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| Error::new("ValueError", "bytes must be in range(0, 256)"))?;
            bytes.push(byte);
        }
    }
    Ok(bytes)
}

fn option<'a>(ctx: &'a Context, value: Option<Id>, name: &str) -> Result<Option<&'a str>> {
    value
        .filter(|&v| v != NONE)
        .map(|id| {
            ctx.str_value(id).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "encode() argument '{name}' must be str, not {}",
                        ctx.type_name(id)
                    ),
                )
            })
        })
        .transpose()
}

/// Invoke a string subclass's encoder with the original Python arguments.
fn encode_text(ctx: &mut Context, text: Id, encoding: Id, errors: Id) -> Result<Id> {
    let method = ctx.lookup_attr(text, "encode")?;
    ctx.call(method, vec![encoding, errors], Vec::new())
}

/// Let bytes subclasses observe the same stripping, decoding, and iteration as urllib.
fn quote_boxed_bytes(ctx: &mut Context, source: Id, safe: Option<Id>) -> Result<Id> {
    if !ctx.truth(source)? {
        return Ok(ctx.text(""));
    }
    let safe = safe_bytes(ctx, safe, b"/")?;
    let mut allowed =
        b"-.0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ_abcdefghijklmnopqrstuvwxyz~".to_vec();
    allowed
        .try_reserve(safe.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    allowed.extend(safe.iter().copied());
    let allowed = ctx.alloc(Value::Bytes(allowed));
    let rstrip = ctx.lookup_attr(source, "rstrip")?;
    let stripped = ctx.call(rstrip, vec![allowed], Vec::new())?;
    if !ctx.truth(stripped)? {
        let decode = ctx.lookup_attr(source, "decode")?;
        return ctx.call(decode, Vec::new(), Vec::new());
    }
    let len = ctx.builtin("len");
    let length = ctx.call(len, vec![source], Vec::new())?;
    let n = ctx.as_int(length)?.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            "cannot fit 'int' into an index-sized integer",
        )
    })?;
    let set = ctx.builtin("frozenset");
    let allowed = ctx.call(set, vec![allowed], Vec::new())?;
    let mut result = String::new();
    if n < 200_000 {
        quote_items(ctx, source, allowed, &mut result)?;
    } else {
        let step = n.isqrt();
        for start in (0..n).step_by(step as usize) {
            let roots = ctx.heap.temporary_mark();
            let first = ctx.int(start);
            let stop = ctx.int(start.saturating_add(step));
            let slice = ctx.alloc(Value::Slice([first, stop, NONE]));
            let part = ctx.get_item(source, slice)?;
            quote_items(ctx, part, allowed, &mut result)?;
            ctx.heap.release_temporaries_since(roots);
        }
    }
    Ok(ctx.text(result))
}

/// Quote yielded values before requesting the next item from a user iterator.
fn quote_items(ctx: &mut Context, source: Id, allowed: Id, result: &mut String) -> Result<()> {
    let iterable = if ctx.slot(source, "__iter__").is_some() {
        source
    } else {
        ctx.scalar_value(source)
    };
    let iterator = ctx.iterator(iterable)?;
    ctx.heap.reserve_temporaries(1)?;
    ctx.heap.keep(iterator);
    let chr = ctx.builtin("chr");
    let format = ctx.builtin("format");
    let spec = ctx.text("02X");
    let roots = ctx.heap.temporary_mark();
    while let Some(item) = ctx.next_item(iterator)? {
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(item);
        let safe = ctx.contains(item, allowed)?;
        let text = if safe {
            ctx.call(chr, vec![item], Vec::new())?
        } else {
            ctx.call(format, vec![item, spec], Vec::new())?
        };
        let text = ctx.str_value(text)?;
        result
            .try_reserve(text.len().saturating_add(usize::from(!safe)))
            .map_err(|_| Error::new("MemoryError", ""))?;
        if !safe {
            result.push('%');
        }
        result.push_str(text);
        ctx.heap.release_temporaries_since(roots);
    }
    Ok(())
}

pub(super) fn quote(
    ctx: &mut Context,
    name: &str,
    id: Id,
    safe: Option<Id>,
    encoding: Option<Id>,
    errors: Option<Id>,
) -> Result<Id> {
    let storage = ctx.string_storage(id);
    let boxed_text = storage.is_some_and(|v| v != id);
    let boxed_bytes = ctx.scalar_value(id) != id && octets(ctx, id).is_some();
    let boxed_safe = safe.is_some_and(|v| matches!(ctx.heap.get(v), Value::Instance { .. }));
    if name == "quote_plus" && (boxed_text || boxed_bytes || boxed_safe) {
        let space = ctx.text(" ");
        let safe = safe.unwrap_or_else(|| ctx.text(""));
        let byte_space = ctx.alloc(Value::Bytes(vec![b' ']));
        if (storage.is_some() && !ctx.contains(space, id)?)
            || (matches!(ctx.heap.get(ctx.scalar_value(id)), Value::Bytes(_))
                && !ctx.contains(byte_space, id)?)
        {
            return quote(ctx, "quote", id, Some(safe), encoding, errors);
        }
        let suffix = if ctx.string_storage(safe).is_some() {
            space
        } else {
            byte_space
        };
        let safe = ctx.binary("+", safe, suffix)?;
        let quoted = quote(ctx, "quote", id, Some(safe), encoding, errors)?;
        let replace = ctx.lookup_attr(quoted, "replace")?;
        let plus = ctx.text("+");
        return ctx.call(replace, vec![space, plus], Vec::new());
    }
    if boxed_text && name != "quote_from_bytes" {
        if !ctx.truth(id)? {
            return Ok(id);
        }
        let encoding = encoding
            .filter(|&v| v != NONE)
            .unwrap_or_else(|| ctx.text("utf-8"));
        let errors = errors
            .filter(|&v| v != NONE)
            .unwrap_or_else(|| ctx.text("strict"));
        let bytes = encode_text(ctx, id, encoding, errors)?;
        return quote(ctx, "quote_from_bytes", bytes, safe, None, None);
    }
    let text = storage.is_some();
    if text && name != "quote_from_bytes" && ctx.str_value(id)?.is_empty() {
        return Ok(id);
    }
    if !text && encoding.is_some_and(|v| v != NONE) {
        return Err(Error::new(
            "TypeError",
            "quote() doesn't support 'encoding' for bytes",
        ));
    }
    if !text && errors.is_some_and(|v| v != NONE) {
        return Err(Error::new(
            "TypeError",
            "quote() doesn't support 'errors' for bytes",
        ));
    }
    if (name == "quote_from_bytes" || !text) && octets(ctx, id).is_none() {
        return Err(Error::new("TypeError", "quote_from_bytes() expected bytes"));
    }
    if boxed_bytes {
        return quote_boxed_bytes(ctx, id, safe);
    }
    if octets(ctx, id).is_some_and(|v| v.is_empty()) {
        return Ok(ctx.text(""));
    }
    let plus = name == "quote_plus";
    let safe = safe_bytes(ctx, safe, if plus { b"" } else { b"/" })?;
    let encoding = option(ctx, encoding, "encoding")?;
    let errors = option(ctx, errors, "errors")?;
    let source = if text {
        UrlInput::Text(ctx.str_value(id)?)
    } else {
        UrlInput::Bytes(octets(ctx, id).unwrap())
    };
    let value = urlquote::quote_component(source, &safe, encoding, errors, plus)?;
    Ok(ctx.text(value))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match name {
        "quote_from_bytes" => {
            let [source, safe] = bind(name, args, kwargs, ["bs", "safe"])?;
            quote(ctx, name, required(source, name, "bs")?, safe, None, None)
        }
        _ => {
            let [source, safe, encoding, errors] =
                bind(name, args, kwargs, ["string", "safe", "encoding", "errors"])?;
            quote(
                ctx,
                name,
                required(source, name, "string")?,
                safe,
                encoding,
                errors,
            )
        }
    }
}
