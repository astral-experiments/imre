//! Bound methods retain the parsed binary layout and convert arguments in field order.
use std::rc::Rc;

use num_traits::ToPrimitive;

use super::{
    Context, Error, Id, Result, Value,
    system::{StructFormat, UnpackedField},
};
use crate::objects::Codec;

fn format_error(message: impl Into<String>) -> Error {
    Error::new("struct.error", message)
}

pub(super) fn new(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let count = args.len() + kwargs.len();
    if count > 1 {
        return Err(Error::new(
            "TypeError",
            format!("Struct() takes at most 1 argument ({count} given)"),
        ));
    }
    let format = args
        .first()
        .copied()
        .or_else(|| {
            kwargs
                .iter()
                .find_map(|(k, id)| (k == "format").then_some(*id))
        })
        .ok_or_else(|| {
            Error::new(
                "TypeError",
                "Struct() missing required argument 'format' (pos 1)",
            )
        })?;
    let (format, layout) = parse(ctx, format)?;
    Ok(ctx.alloc(Value::Struct {
        format: format.to_owned(),
        layout: Rc::new(layout),
    }))
}

/// Read the underlying format spelling and validate the layout before inspecting the buffer.
pub(super) fn parse(ctx: &Context, format: Id) -> Result<(&str, StructFormat)> {
    let format = match ctx.heap.get(ctx.scalar_value(format)) {
        Value::Str(text) => {
            if !text.is_ascii() {
                Codec::Ascii.encode(text, None, || {
                    crate::StrOwner::try_new(crate::Str::from(text.as_str()))
                        .map_err(|_| crate::Error::memory_error())
                })?;
            }
            text.as_str()
        }
        Value::Bytes(data) => {
            std::str::from_utf8(data).map_err(|_| format_error("bad char in struct format"))?
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "Struct() argument 1 must be a str or bytes object, not {}",
                    ctx.type_name(format)
                ),
            ));
        }
    };
    if format.contains('\0') {
        return Err(format_error("embedded null character"));
    }
    Ok((format, StructFormat::parse(format).map_err(format_error)?))
}

pub(super) fn unpack(ctx: &mut Context, layout: &StructFormat, buffer: Id) -> Result<Id> {
    let fields = layout
        .unpack(ctx.bytes_like(buffer)?)
        .map_err(format_error)?;
    let items = fields
        .into_iter()
        .map(|field| match field {
            UnpackedField::Integer(word) => ctx.alloc(Value::Int(crate::Int::new(word.into()))),
            UnpackedField::Signed(number) => ctx.int(number),
            UnpackedField::Float(number) => ctx.alloc(Value::Float(number)),
            UnpackedField::Bytes(bytes) => ctx.alloc(Value::Bytes(bytes)),
        })
        .collect();
    Ok(ctx.alloc(Value::Tuple(items)))
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&receiver, args) = args.split_first().expect("bound Struct method");
    let Value::Struct { layout, .. } = ctx.heap.get(receiver) else {
        unreachable!("bound Struct method")
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("Struct.{name}() takes no keyword arguments"),
        ));
    }
    let layout = Rc::clone(layout);
    if name == "unpack" {
        let [id] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "Struct.unpack() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        return unpack(ctx, &layout, *id);
    }
    pack(ctx, &layout, args)
}

pub(super) fn pack(ctx: &mut Context, layout: &StructFormat, args: &[Id]) -> Result<Id> {
    let count = layout.item_count;
    if args.len() != count {
        return Err(format_error(format!(
            "pack expected {count} items for packing (got {})",
            args.len()
        )));
    }
    let mut output = super::super::builtins::zeroed_bytes(layout.size as i64)?;
    for ((code, range), &id) in layout.items().zip(args) {
        let target = &mut output[range];
        if code == b's' {
            let (Value::Bytes(bytes) | Value::ByteArray(bytes)) =
                ctx.heap.get(ctx.scalar_value(id))
            else {
                return Err(format_error("argument for 's' must be a bytes object"));
            };
            let length = target.len().min(bytes.len());
            target[..length].copy_from_slice(&bytes[..length]);
        } else {
            let word = if code == b'd' {
                double_field(ctx, id)?.to_bits()
            } else {
                integer_field(ctx, id, code, target.len())?
            };
            layout.write_integer(target, word);
        }
    }
    Ok(ctx.alloc(Value::Bytes(output)))
}

fn double_field(ctx: &mut Context, id: Id) -> Result<f64> {
    // PyFloat_AsDouble reads float subclasses directly, bypassing their __float__.
    if let Value::Float(value) = *ctx.heap.get(ctx.scalar_value(id)) {
        return Ok(value);
    }
    let Ok(Some(number)) = super::super::builtins::from_numeric(ctx, id) else {
        return Err(format_error("required argument is not a float"));
    };
    let Value::Float(value) = *ctx.heap.get(number) else {
        unreachable!("float conversion returned a non-float")
    };
    Ok(value)
}

fn integer_field(ctx: &mut Context, id: Id, code: u8, width: usize) -> Result<u64> {
    let value = ctx
        .index_value(id)
        .and_then(|value| value.ok_or_else(|| format_error("required argument is not an integer")));
    if matches!(code, b'i' | b'l') {
        let max = i64::MAX >> ((8 - width) * 8);
        let min = !max;
        let number = match value {
            Ok(value) => value.to_i64(),
            Err(error) => {
                let overflow = ctx.builtin("OverflowError");
                if !ctx.raised_error_matches(&error, overflow) {
                    return Err(error);
                }
                None
            }
        };
        return number
            .filter(|n| (min..=max).contains(n))
            .map(|n| n as u64)
            .ok_or_else(|| {
                format_error(format!(
                    "'{}' format requires {min} <= number <= {max}",
                    char::from(code)
                ))
            });
    }
    let value = value?;
    let number = value.big().to_u64();
    let max = u64::MAX >> ((8 - width) * 8);
    if let Some(n) = number.filter(|&n| n <= max) {
        return Ok(n);
    }
    if code == b'P' {
        return value
            .to_i64()
            .and_then(|v| isize::try_from(v).ok())
            .map(|v| v as usize as u64)
            .ok_or_else(|| format_error("int too large to convert"));
    }
    Err(format_error(format!(
        "'{}' format requires 0 <= number <= {max}",
        char::from(code)
    )))
}
