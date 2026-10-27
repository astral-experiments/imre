//! Bind native URL quoting functions for runtime-imported modules.
use super::*;

fn octets(ctx: &Context, id: Id) -> Option<&[u8]> {
    match ctx.heap.get(id) {
        Value::Bytes(b) | Value::ByteArray(b) => Some(b),
        _ => None,
    }
}

fn safe_bytes(ctx: &mut Context, safe: Option<Id>, default: &'static [u8]) -> Result<Vec<u8>> {
    let Some(id) = safe else {
        return Ok(default.to_vec());
    };
    match ctx.heap.get(id) {
        Value::Str(s) => return Ok(s.as_bytes().to_vec()),
        Value::Bytes(v) | Value::ByteArray(v) => return Ok(v.clone()),
        _ => {}
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

pub(super) fn quote(
    ctx: &mut Context,
    name: &str,
    id: Id,
    safe: Option<Id>,
    encoding: Option<Id>,
    errors: Option<Id>,
) -> Result<Id> {
    let text = matches!(ctx.heap.get(id), Value::Str(_));
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
