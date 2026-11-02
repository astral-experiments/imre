use super::super::value::{Hmac, NOT_IMPLEMENTED};
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value, argument, option};
use crate::objects::HashState;
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
            // HMAC accepts mixed-case SHA3_ spellings in addition to the provider's own aliases.
            let name = HashState::algorithms()
                .find(|alg| alg.starts_with("sha3_") && alg.eq_ignore_ascii_case(name))
                .unwrap_or(name);
            let provider = HashState::provider(name)?;
            if let Some(class) = HashState::public_constructor(provider.name) {
                ctx.builtin(class)
            } else if provider.name == "md5" {
                ctx.builtin("hashlib.md5")
            } else {
                super::attribute(ctx, "hashlib", provider.name)?
            }
        }
        Value::Module { .. } => ctx.lookup_attr(digest, "new")?,
        _ => digest,
    };

    // Invoke the supplied factory for both states, including lazy wheel functions.
    let outer = ctx.call(factory, Vec::new(), Vec::new())?;
    if matches!(ctx.heap.get(outer), Value::Hash(hash) if hash.lock().is_xof()) {
        return Err(Error::new("ValueError", "no reason supplied"));
    }
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

/// Called for strings and built-in functions, before Python tries its other providers.
pub(super) fn digest_native(ctx: &mut Context, args: &[Id]) -> Result<Id> {
    let key = argument(args, 0)?;
    let msg = argument(args, 1)?;
    let digest = argument(args, 2)?;
    // Argument Clinic acquires both buffers before it checks the supplied constructor.
    ctx.check_buffer_exporter(key)?;
    let key = ctx.native_buffer_bytes(key)?;
    ctx.check_buffer_exporter(msg)?;
    let msg = ctx.native_buffer_bytes(msg)?;
    if i32::try_from(key.len()).is_err() || i32::try_from(msg.len()).is_err() {
        return Err(Error::new("OverflowError", "HMAC input is too long"));
    }
    let name = match ctx.heap.get(ctx.scalar_value(digest)) {
        Value::Str(name) => name.as_str(),
        Value::Native(symbol)
            if ctx.builtins.get(symbol) == Some(&digest)
                && let Some(name) = symbol.strip_prefix("_hashlib.openssl_")
                && super::hashlib::digest::NAMES.contains(&name) =>
        {
            name
        }
        // These five public built-ins are separate native objects in Imre.
        // Try their configured provider; a failed lookup calls the supplied object in Python.
        Value::Native(symbol)
            if let Some(name) = symbol.strip_prefix("hashlib.")
                && matches!(name, "sha1" | "sha224" | "sha256" | "sha384" | "sha512") =>
        {
            name
        }
        _ => return Ok(NOT_IMPLEMENTED),
    };
    if let Some(provider) = super::hashlib::openssl::provider()
        && let Some(mac) = provider.hmac(name, key, msg)?
    {
        return Ok(ctx.alloc(Value::Bytes(mac)));
    }
    Ok(NOT_IMPLEMENTED)
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

/// A saved comparison function keeps its originating extension module alive across reimports.
pub(super) fn bind_compare(ctx: &mut Context, module: Id, scope: Id, symbol: &str) {
    let callable = ctx.builtin(symbol);
    let function = ctx.alloc(Value::Method {
        callable,
        receiver: module,
        explicit: false,
    });
    ctx.bind(scope, symbol.rsplit('.').next().unwrap(), function);
}

pub(super) fn compare(
    ctx: &Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    // The module-bound native method supplies its own receiver.
    let args = &args[1..];
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    let &[left, right] = args else {
        let function = name.rsplit('.').next().unwrap();
        return Err(Error::new(
            "TypeError",
            format!("{function} expected 2 arguments, got {}", args.len()),
        ));
    };
    let equal = if let (Some(a), Some(b)) = (ctx.string_storage(left), ctx.string_storage(right)) {
        let (Value::Str(left), Value::Str(right)) = (ctx.heap.get(a), ctx.heap.get(b)) else {
            return Err(Error::new(
                "TypeError",
                "comparing strings with non-ASCII characters is not supported",
            ));
        };
        compare_text(left, right).map_err(|message| Error::new("TypeError", message))?
    } else {
        let supports_buffer = |id| {
            matches!(
                ctx.heap.get(ctx.scalar_value(id)),
                Value::Bytes(_)
                    | Value::ByteArray(_)
                    | Value::Array(_)
                    | Value::MemoryView { .. }
                    | Value::BytesIoBuffer { .. }
            )
        };
        if !supports_buffer(left) && !supports_buffer(right) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "unsupported operand types(s) or combination of types: '{:.100}' and '{:.100}'",
                    ctx.type_name(left),
                    ctx.type_name(right)
                ),
            ));
        }
        ctx.check_buffer_exporter(left)?;
        let left = ctx.native_buffer_bytes(left)?;
        ctx.check_buffer_exporter(right)?;
        let right = ctx.native_buffer_bytes(right)?;
        compare_bytes(left, right)
    };
    Ok(if equal { TRUE } else { FALSE })
}
