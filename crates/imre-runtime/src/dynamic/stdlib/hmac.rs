use super::super::value::Hmac;
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value, argument, option};
use crate::objects::hmac::{compare_bytes, compare_text};

fn hash_call(ctx: &mut Context, hash: Id, method: &str, args: Vec<Id>) -> Result<Id> {
    let callable = ctx.lookup_attr(hash, method)?;
    ctx.call(callable, args, Vec::new())
}

fn pad(ctx: &Context, key: Id, block_size: usize, byte: u8) -> Result<Value> {
    let value = ctx.heap.get(key);
    let (Value::Bytes(key) | Value::ByteArray(key)) = value else {
        return Err(Error::new("TypeError", "digest must return bytes"));
    };
    let len = block_size.max(key.len());
    let mut padded = Vec::new();
    padded
        .try_reserve_exact(len)
        .map_err(|_| Error::new("MemoryError", "cannot allocate HMAC pad"))?;
    padded.extend(key.iter().map(|&value| value ^ byte));
    padded.resize(len, byte);
    Ok(match value {
        Value::ByteArray(_) => Value::ByteArray(padded),
        _ => Value::Bytes(padded),
    })
}

pub(super) fn new(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let mut key = option(args, 0, kwargs, "key")?
        .ok_or_else(|| Error::new("TypeError", "missing required argument 'key'"))?;
    let msg = option(args, 1, kwargs, "msg")?.filter(|&id| id != NONE);
    let digest = option(args, 2, kwargs, "digestmod")?.unwrap_or(NONE);
    if !matches!(ctx.heap.get(key), Value::Bytes(_) | Value::ByteArray(_)) {
        return Err(Error::new(
            "TypeError",
            "HMAC key must be bytes or bytearray",
        ));
    }
    if !ctx.truth(digest)? {
        return Err(Error::new(
            "TypeError",
            "Missing required argument 'digestmod'.",
        ));
    }
    let factory = match ctx.heap.get(digest) {
        Value::Str(name) => {
            let name = name.to_ascii_lowercase();
            super::attribute(ctx, "hashlib", &name)
                .map_err(|_| Error::new("ValueError", format!("unsupported hash type {name}")))?
        }
        Value::Module { .. } => ctx.lookup_attr(digest, "new")?,
        _ => digest,
    };

    // Invoke the supplied factory for both states, including lazy wheel functions.
    let outer = ctx.call(factory, Vec::new(), Vec::new())?;
    let inner = ctx.call(factory, Vec::new(), Vec::new())?;
    let digest_size = ctx.lookup_attr(inner, "digest_size")?;
    let block_size = match ctx.lookup_attr(inner, "block_size") {
        Ok(size) if ctx.as_integer(size)? >= 16 => size,
        Ok(_) => ctx.int(64),
        Err(err) if err.kind == "AttributeError" => ctx.int(64),
        Err(err) => return Err(err),
    };
    let block_len = usize::try_from(ctx.as_integer(block_size)?)
        .map_err(|_| Error::new("OverflowError", "HMAC block size is too large"))?;
    if super::hashlib::buffer(ctx, key)?.len() > block_len {
        let hash = ctx.call(factory, vec![key], Vec::new())?;
        key = hash_call(ctx, hash, "digest", Vec::new())?;
    }
    // Both pads reflect the key before a digest update can mutate it.
    let outer_pad = pad(ctx, key, block_len, 0x5c)?;
    let inner_pad = pad(ctx, key, block_len, 0x36)?;
    let outer_pad = ctx.alloc(outer_pad);
    hash_call(ctx, outer, "update", vec![outer_pad])?;
    let inner_pad = ctx.alloc(inner_pad);
    hash_call(ctx, inner, "update", vec![inner_pad])?;
    if let Some(msg) = msg {
        hash_call(ctx, inner, "update", vec![msg])?;
    }
    Ok(ctx.alloc(Value::Hmac(Hmac {
        inner,
        outer,
        digest_size,
        block_size,
    })))
}

pub(super) fn method(ctx: &mut Context, name: &str, args: &[Id]) -> Result<Id> {
    let Value::Hmac(mac) = *ctx.heap.get(argument(args, 0)?) else {
        unreachable!("bound HMAC method");
    };
    match name {
        "hmac.HMAC.update" => {
            hash_call(ctx, mac.inner, "update", vec![argument(args, 1)?])?;
            Ok(NONE)
        }
        "hmac.HMAC.copy" => {
            let inner = hash_call(ctx, mac.inner, "copy", Vec::new())?;
            let outer = hash_call(ctx, mac.outer, "copy", Vec::new())?;
            Ok(ctx.alloc(Value::Hmac(Hmac {
                inner,
                outer,
                ..mac
            })))
        }
        "hmac.HMAC.digest" | "hmac.HMAC.hexdigest" => {
            let outer = hash_call(ctx, mac.outer, "copy", Vec::new())?;
            let inner_digest = hash_call(ctx, mac.inner, "digest", Vec::new())?;
            hash_call(ctx, outer, "update", vec![inner_digest])?;
            hash_call(
                ctx,
                outer,
                if name == "hmac.HMAC.digest" {
                    "digest"
                } else {
                    "hexdigest"
                },
                Vec::new(),
            )
        }
        _ => unreachable!("registered HMAC method"),
    }
}

pub(super) fn compare(ctx: &Context, args: &[Id]) -> Result<Id> {
    let (left, right) = (
        ctx.heap.get(argument(args, 0)?),
        ctx.heap.get(argument(args, 1)?),
    );
    let equal = match (left, right) {
        (Value::Bytes(a) | Value::ByteArray(a), Value::Bytes(b) | Value::ByteArray(b)) => {
            compare_bytes(a, b)
        }
        (Value::Str(a), Value::Str(b)) => {
            compare_text(a, b).map_err(|message| Error::new("TypeError", message))?
        }
        (Value::Str(_), Value::Bytes(_) | Value::ByteArray(_))
        | (Value::Bytes(_) | Value::ByteArray(_), Value::Str(_)) => {
            return Err(Error::new(
                "TypeError",
                "a bytes-like object is required, not 'str'",
            ));
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                "compare_digest requires two byte buffers or ASCII strings",
            ));
        }
    };
    Ok(if equal { TRUE } else { FALSE })
}
