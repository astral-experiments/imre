//! Base64 codecs and zlib checksums for runtime-loaded callers.
use super::{Context, Error, Id, Result, Value};

fn input(ctx: &Context, value: Id, decode: bool) -> Result<&[u8]> {
    if !decode {
        return ctx.native_buffer_bytes(value);
    }
    if let Some(storage) = ctx.string_storage(value) {
        // Surrogate-containing strings are Unicode inputs, even though they have no UTF-8 view.
        return match ctx.heap.get(storage) {
            Value::Str(text) if text.is_ascii() => Ok(text.as_bytes()),
            _ => Err(Error::new(
                "ValueError",
                "string argument should contain only ASCII characters",
            )),
        };
    }
    ctx.native_buffer_bytes(value).map_err(|_| {
        let name = ctx.type_name(value);
        let name = &name[..name.floor_char_boundary(100)];
        Error::new(
            "TypeError",
            format!("argument should be bytes, buffer or ASCII string, not '{name}'"),
        )
    })
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if matches!(name, "a2b_hex" | "unhexlify") {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("binascii.{name}() takes no keyword arguments"),
            ));
        }
        let [data] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "binascii.{name}() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        let result = crate::objects::unhexlify(input(ctx, *data, true)?)?;
        return Ok(ctx.alloc(Value::Bytes(result)));
    }
    let count = args.len() + kwargs.len();
    if count > 2 {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes at most 2 arguments ({count} given)"),
        ));
    }
    let [data] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes exactly 1 positional argument ({} given)",
                args.len()
            ),
        ));
    };
    let decode = name == "a2b_base64";
    let flag = if decode { "strict_mode" } else { "newline" };
    if let Some((key, _)) = kwargs.iter().find(|(key, _)| key != flag) {
        return Err(Error::new(
            "TypeError",
            format!("{name}() got an unexpected keyword argument '{key}'"),
        ));
    }
    // Argument Clinic converts the input before invoking the flag's __bool__.
    let option = if let Some((_, value)) = kwargs.first() {
        input(ctx, *data, decode)?;
        ctx.truth_with_buffer_export(*data, *value)?
    } else {
        !decode
    };
    let bytes = input(ctx, *data, decode)?;
    let result = if decode {
        crate::objects::binascii_decode(bytes, option)?
    } else {
        crate::objects::binascii_encode(bytes, option)?
    };
    Ok(ctx.alloc(Value::Bytes(result)))
}

/// Convert the exported buffer before the seed, as required by the zlib checksum functions.
pub(super) fn checksum(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("zlib.{method}() takes no keyword arguments"),
        ));
    }
    let (data, seed) = match args {
        [data] => (
            *data,
            crate::Int::from_usize(usize::from(method == "adler32")),
        ),
        [data, seed] => {
            ctx.bytes_like(*data)?;
            let seed = ctx.with_buffer_export(*data, |ctx| ctx.index_integer(*seed))?;
            (*data, seed)
        }
        [] => {
            return Err(Error::new(
                "TypeError",
                format!("{method} expected at least 1 argument, got 0"),
            ));
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                format!("{method} expected at most 2 arguments, got {}", args.len()),
            ));
        }
    };
    let bytes = ctx.bytes_like(data)?;
    let checksum = if method == "crc32" {
        crate::objects::crc32(bytes, &seed)
    } else {
        crate::objects::adler32(bytes, &seed)
    };
    Ok(ctx.int(i64::from(checksum)))
}
