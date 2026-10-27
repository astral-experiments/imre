//! Runtime-loaded callers share the typed backend's Base64 codecs and Adler-32 checksum.
use super::{Context, Error, Id, Result, Value};

fn input(ctx: &Context, value: Id, decode: bool) -> Result<&[u8]> {
    if !decode {
        return ctx.bytes_like(value);
    }
    if let Ok(text) = ctx.str_value(value) {
        return if text.is_ascii() {
            Ok(text.as_bytes())
        } else {
            Err(Error::new(
                "ValueError",
                "string argument should contain only ASCII characters",
            ))
        };
    }
    ctx.bytes_like(value).map_err(|_| {
        Error::new(
            "TypeError",
            format!(
                "argument should be bytes, buffer or ASCII string, not '{}'",
                ctx.type_name(value)
            ),
        )
    })
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
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

/// Apply zlib's positional argument and buffer conversions before the shared checksum.
pub(super) fn adler32(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "zlib.adler32() takes no keyword arguments",
        ));
    }
    let (data, seed) = match args {
        [data] => (*data, crate::Int::from_usize(1)),
        [data, seed] => {
            ctx.bytes_like(*data)?;
            let seed = ctx.with_buffer_export(*data, |ctx| ctx.index_integer(*seed))?;
            (*data, seed)
        }
        [] => {
            return Err(Error::new(
                "TypeError",
                "adler32 expected at least 1 argument, got 0",
            ));
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                format!("adler32 expected at most 2 arguments, got {}", args.len()),
            ));
        }
    };
    let checksum = crate::objects::adler32(ctx.bytes_like(data)?, &seed);
    Ok(ctx.int(i64::from(checksum)))
}
