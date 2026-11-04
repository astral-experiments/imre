//! Binary-to-text codecs and zlib checksums for runtime-loaded callers.
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
    ctx.native_buffer_bytes(value)
        .map_err(|_| ascii_buffer_error(ctx, value))
}

fn ascii_buffer_error(ctx: &Context, value: Id) -> Error {
    let name = ctx.type_name(value);
    let name = &name[..name.floor_char_boundary(100)];
    Error::new(
        "TypeError",
        format!("argument should be bytes, buffer or ASCII string, not '{name}'"),
    )
}

/// Acquire the ASCII string or contiguous buffer before calling the header flag's truth test.
fn a2b_qp(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(data), header] = crate::dynamic::builtins::bind_builtin_arguments(
        "a2b_qp",
        args,
        kwargs,
        ["data", "header"],
        1,
    )?
    else {
        unreachable!()
    };
    let decode = |ctx: &mut Context, view| {
        let header = ctx.optional_truth(header, false)?;
        Ok(crate::objects::decode_quoted_printable(
            input(ctx, view, true)?,
            header,
        )?)
    };
    let bytes = if ctx.string_storage(data).is_none()
        && let Some(method) = ctx.python_buffer_method(data, true)?
    {
        let view = ctx
            .acquire_python_buffer(data, method, 0)
            .and_then(|view| ctx.view_bytes(view).map(|_| view))
            .map_err(|_| ascii_buffer_error(ctx, data))?;
        ctx.with_python_buffer_export(data, view, |ctx| decode(ctx, view))?
    } else {
        input(ctx, data, true)?;
        ctx.with_buffer_export(data, |ctx| decode(ctx, data))?
    };
    Ok(ctx.alloc(Value::Bytes(bytes)))
}

/// Leave subclass operations, unfamiliar alphabets and custom truth tests to the Python helper.
pub(super) fn base32(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    decode: bool,
) -> Result<Id> {
    use crate::objects::base32::{ALPHABETS, decode as decode32, encode};
    let (data, alphabet, casefold) = match (args, decode) {
        ([data, alphabet], false) if kwargs.is_empty() => (*data, *alphabet, false),
        ([data, alphabet, flag], true) if kwargs.is_empty() => {
            let Value::Bool(flag) = ctx.heap.get(*flag) else {
                return Ok(super::NONE);
            };
            (*data, *alphabet, *flag)
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                "unexpected arguments to Base32 codec",
            ));
        }
    };
    let (Value::Bytes(data) | Value::ByteArray(data)) = ctx.heap.get(data) else {
        return Ok(super::NONE);
    };
    let Value::Bytes(alphabet) = ctx.heap.get(alphabet) else {
        return Ok(super::NONE);
    };
    let hex = if alphabet.as_slice() == ALPHABETS[0] {
        false
    } else if alphabet.as_slice() == ALPHABETS[1] {
        true
    } else {
        return Ok(super::NONE);
    };
    let output = if decode {
        decode32(data, hex, casefold)?
    } else {
        encode(data, hex)?
    };
    Ok(ctx.alloc(Value::Bytes(output)))
}

/// Return None when Python must run an overridden buffer operation or truth test.
pub(super) fn encode85(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [buffer, alphabet, pad, foldnuls, foldspaces] = args else {
        return Err(Error::new(
            "TypeError",
            "encode() requires five positional arguments",
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "encode() takes no keyword arguments",
        ));
    }
    let (Value::Bytes(data) | Value::ByteArray(data)) = ctx.heap.get(*buffer) else {
        return Ok(super::NONE);
    };
    let Value::Bytes(alphabet) = ctx.heap.get(*alphabet) else {
        return Ok(super::NONE);
    };
    let Ok(alphabet) = alphabet.as_slice().try_into() else {
        return Ok(super::NONE);
    };
    let (Value::Bool(pad), Value::Bool(foldnuls), Value::Bool(foldspaces)) = (
        ctx.heap.get(*pad),
        ctx.heap.get(*foldnuls),
        ctx.heap.get(*foldspaces),
    ) else {
        return Ok(super::NONE);
    };
    let output = crate::objects::base85_encode(data, alphabet, *pad, *foldnuls, *foldspaces)?;
    Ok(ctx.alloc(Value::Bytes(output)))
}

/// Encode a contiguous buffer as uppercase hex, as in binascii.hexlify(s).upper().
pub(super) fn b16encode(ctx: &mut Context, data: Id) -> Result<Id> {
    let hex = if let Some(method) = ctx.python_buffer_method(data, true)? {
        // binascii.hexlify requests PyBUF_SIMPLE from Python buffer exporters.
        ctx.with_contiguous_python_buffer(data, method, 0, |ctx, view| {
            Ok(crate::objects::hash_hex(ctx.view_bytes(view)?)?)
        })?
    } else {
        crate::objects::hash_hex(input(ctx, data, false)?)?
    };
    let mut encoded = hex.into_string().into_bytes();
    encoded.make_ascii_uppercase();
    Ok(ctx.alloc(Value::Bytes(encoded)))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "a2b_qp" {
        return a2b_qp(ctx, args, kwargs);
    }
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

/// Hold a contiguous buffer export while converting the checksum's optional initial value.
pub(super) fn checksum(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    let method = name.rsplit('.').next().unwrap();
    let (data, initial) = match args {
        [data] => (*data, None),
        [data, seed] => (*data, Some(*seed)),
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
    let calculate = |ctx: &mut Context, view| {
        let seed = match initial {
            Some(value) => ctx.index_integer(value)?,
            None => crate::Int::from_usize(usize::from(method == "adler32")),
        };
        let bytes = ctx.native_buffer_bytes(view)?;
        Ok(if method == "adler32" {
            crate::objects::adler32(bytes, &seed)
        } else {
            crate::objects::crc32(bytes, &seed)
        })
    };
    let checksum = if name == "binascii.crc32"
        && let Some(export) = ctx.python_buffer_method(data, true)?
    {
        ctx.with_contiguous_python_buffer(data, export, 0, calculate)?
    } else {
        ctx.native_buffer_bytes(data)?;
        ctx.with_buffer_export(data, |ctx| calculate(ctx, data))?
    };
    Ok(ctx.int(i64::from(checksum)))
}
