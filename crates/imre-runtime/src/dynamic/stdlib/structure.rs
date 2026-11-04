//! Bound methods retain the parsed binary layout and convert arguments in field order.
use std::rc::Rc;

use num_traits::ToPrimitive;

use super::super::builtins::{bind_builtin_arguments, boolean};
use super::{
    Context, Error, Id, NONE, Result, Value,
    system::{StructFormat, UnpackedField},
};
use crate::objects::Codec;

/// Keep the exception identity shared by the module, compiled layouts, and native errors.
pub(in super::super) fn install(ctx: &mut Context, scope: Id) -> Result<()> {
    let class = if let Some(&class) = ctx.builtins.get("struct.error") {
        class
    } else {
        let fields = ctx.scope("struct", None);
        let module = ctx.text("struct");
        ctx.bind(fields, "__module__", module);
        let qualname = ctx.text("error");
        ctx.bind(fields, "__qualname__", qualname);
        ctx.bind(fields, "__doc__", NONE);
        let base = ctx.builtin("Exception");
        let class = ctx.allocate_class("error".into(), fields, vec![base])?;
        ctx.install_weakref_descriptor(class, fields);
        ctx.builtins.insert("struct.error".into(), class);
        class
    };
    ctx.bind(scope, "error", class);
    Ok(())
}

fn format_error(ctx: &Context, message: impl Into<String>) -> Error {
    let mut error = Error::new("struct.error", message);
    error.class = ctx.builtins["struct.error"];
    error
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
            std::str::from_utf8(data).map_err(|_| format_error(ctx, "bad char in struct format"))?
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
        return Err(format_error(ctx, "embedded null character"));
    }
    Ok((
        format,
        StructFormat::parse(format).map_err(|message| format_error(ctx, message))?,
    ))
}

pub(super) fn unpack(ctx: &mut Context, layout: &StructFormat, buffer: Id) -> Result<Id> {
    let decode = |ctx: &mut Context, view| {
        layout
            .unpack(ctx.native_buffer_bytes(view)?)
            .map_err(|message| format_error(ctx, message))
    };
    let method = match ctx.heap.get(ctx.scalar_value(buffer)) {
        // Preserve the native read for bytes and bytearray subclasses with only a release hook.
        Value::Bytes(_) | Value::ByteArray(_) => ctx.buffer_slot(buffer, "__buffer__").flatten(),
        _ => ctx.python_buffer_method(buffer, true)?,
    };
    let fields = if let Some(method) = method {
        ctx.with_contiguous_python_buffer(buffer, method, 0, decode)
    } else {
        decode(ctx, buffer)
    }?;
    Ok(unpacked_tuple(ctx, fields))
}

fn unpacked_tuple(ctx: &mut Context, fields: Vec<UnpackedField>) -> Id {
    let items = fields
        .into_iter()
        .map(|field| match field {
            UnpackedField::Bool(value) => boolean(value),
            UnpackedField::Integer(word) => ctx.alloc(Value::Int(crate::Int::new(word.into()))),
            UnpackedField::Signed(number) => ctx.int(number),
            UnpackedField::Float(number) => ctx.alloc(Value::Float(number)),
            UnpackedField::Bytes(bytes) => ctx.alloc(Value::Bytes(bytes)),
        })
        .collect();
    ctx.alloc(Value::Tuple(items))
}

pub(super) fn module_unpack_from(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    // Only the format is positional-only. Argument Clinic checks total arity first.
    if args.is_empty() && kwargs.len() <= 3 {
        return Err(Error::new(
            "TypeError",
            "unpack_from() takes at least 1 positional argument (0 given)",
        ));
    }
    let [Some(format), Some(buffer), offset] =
        bind_builtin_arguments("unpack_from", args, kwargs, ["", "buffer", "offset"], 2)?
    else {
        unreachable!("required format and buffer")
    };
    let (_, layout) = parse(ctx, format)?;
    unpack_from(ctx, &layout, buffer, offset)
}

fn unpack_from(
    ctx: &mut Context,
    layout: &StructFormat,
    buffer: Id,
    offset: Option<Id>,
) -> Result<Id> {
    // Acquire the buffer before __index__, then read its current bytes under the same export.
    let decode = |ctx: &mut Context, view| {
        let offset = offset.map_or(Ok(0), |id| super::os::ssize_t(ctx, id))?;
        layout
            .unpack_from(ctx.native_buffer_bytes(view)?, offset)
            .map_err(|message| format_error(ctx, message))
    };
    let fields = if let Some(method) = ctx.python_buffer_method(buffer, true)? {
        ctx.with_contiguous_python_buffer(buffer, method, 0, decode)
    } else {
        ctx.native_buffer_bytes(buffer)?;
        ctx.with_buffer_export(buffer, |ctx| decode(ctx, buffer))
    }?;
    Ok(unpacked_tuple(ctx, fields))
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
    let layout = Rc::clone(layout);
    if name == "unpack_from" {
        let [Some(buffer), offset] =
            bind_builtin_arguments("unpack_from", args, kwargs, ["buffer", "offset"], 1)?
        else {
            unreachable!("required buffer")
        };
        return unpack_from(ctx, &layout, buffer, offset);
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("Struct.{name}() takes no keyword arguments"),
        ));
    }
    if name == "pack_into" {
        return pack_into(ctx, &layout, args);
    }
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

fn byte_field_error(ctx: &Context, code: u8) -> Error {
    format_error(
        ctx,
        format!("argument for '{}' must be a bytes object", char::from(code)),
    )
}

/// Write a byte string into a cleared field. A missing source denotes the destination itself.
fn write_byte_field(
    code: u8,
    buffer: &mut [u8],
    field: std::ops::Range<usize>,
    source: Option<&[u8]>,
) {
    let has_length = code == b'p' && !field.is_empty();
    let start = field.start + usize::from(has_length);
    let capacity = field.end - start;
    let copied = if let Some(bytes) = source {
        let count = capacity.min(bytes.len());
        buffer[start..start + count].copy_from_slice(&bytes[..count]);
        count
    } else {
        buffer.copy_within(..capacity, start);
        capacity
    };
    if has_length {
        buffer[field.start] = copied.min(usize::from(u8::MAX)) as u8;
    }
}

pub(super) fn pack(ctx: &mut Context, layout: &StructFormat, args: &[Id]) -> Result<Id> {
    let count = layout.item_count;
    if args.len() != count {
        return Err(format_error(
            ctx,
            format!(
                "pack expected {count} items for packing (got {})",
                args.len()
            ),
        ));
    }
    let mut output = super::super::builtins::zeroed_bytes(layout.size as i64)?;
    for ((code, range), &id) in layout.items().zip(args) {
        if matches!(code, b's' | b'p') {
            let (Value::Bytes(bytes) | Value::ByteArray(bytes)) =
                ctx.heap.get(ctx.scalar_value(id))
            else {
                return Err(byte_field_error(ctx, code));
            };
            write_byte_field(code, &mut output, range, Some(bytes));
        } else {
            let target = &mut output[range];
            let word = pack_word(ctx, id, code, target.len(), layout.little_endian)?;
            layout.write_integer(target, word);
        }
    }
    Ok(ctx.alloc(Value::Bytes(output)))
}

pub(super) fn pack_into(ctx: &mut Context, layout: &StructFormat, args: &[Id]) -> Result<Id> {
    let [buffer, offset, values @ ..] = args else {
        return Err(format_error(
            ctx,
            if args.is_empty() {
                "pack_into expected buffer argument"
            } else {
                "pack_into expected offset argument"
            },
        ));
    };
    if values.len() != layout.item_count {
        return Err(format_error(
            ctx,
            format!(
                "pack_into expected {} items for packing (got {})",
                layout.item_count,
                values.len()
            ),
        ));
    }
    ctx.with_writable_byte_span(*buffer, "argument", |ctx, storage, span| {
        let index = ctx.index_integer(*offset)?;
        let offset = crate::signed_index(&index).ok_or_else(|| {
            Error::new(
                "IndexError",
                format!(
                    "cannot fit '{}' into an index-sized integer",
                    ctx.type_name(*offset)
                ),
            )
        })?;
        let record = layout
            .record_range(span.len(), offset, true)
            .map_err(|e| format_error(ctx, e))?;
        let start = span.start + record.start;
        // Field conversion callbacks can observe the cleared record and any preceding writes.
        ctx.writable_storage_bytes(storage)?[start..start + layout.size].fill(0);
        for ((code, range), &id) in layout.items().zip(values) {
            let pos = start + range.start;
            if matches!(code, b's' | b'p') {
                let field = pos..start + range.end;
                let source = ctx.scalar_value(id);
                if source == storage {
                    let Value::ByteArray(bytes) = ctx.heap.get_mut(storage) else {
                        return Err(byte_field_error(ctx, code));
                    };
                    write_byte_field(code, bytes, field, None);
                } else {
                    let Some((Value::Bytes(bytes) | Value::ByteArray(bytes), target)) =
                        ctx.heap.source_and_target(source, storage)
                    else {
                        return Err(byte_field_error(ctx, code));
                    };
                    write_byte_field(
                        code,
                        Context::mutable_storage_bytes(target)?,
                        field,
                        Some(bytes),
                    );
                }
            } else {
                let word = pack_word(ctx, id, code, range.len(), layout.little_endian)?;
                layout.write_integer(
                    &mut ctx.writable_storage_bytes(storage)?[pos..start + range.end],
                    word,
                );
            }
        }
        Ok(NONE)
    })
}

fn pack_word(
    ctx: &mut Context,
    id: Id,
    code: u8,
    width: usize,
    little_endian: bool,
) -> Result<u64> {
    match code {
        b'c' => match ctx.heap.get(ctx.scalar_value(id)) {
            Value::Bytes(bytes) if bytes.len() == 1 => Ok(u64::from(bytes[0])),
            _ => Err(format_error(
                ctx,
                "char format requires a bytes object of length 1",
            )),
        },
        b'?' => ctx.truth(id).map(u64::from),
        b'f' => single_bits(ctx, id).map(u64::from),
        b'd' => float_value(ctx, id).map(f64::to_bits),
        _ => integer_field(ctx, id, code, width, little_endian),
    }
}

fn single_bits(ctx: &mut Context, id: Id) -> Result<u32> {
    let value = float_value(ctx, id)?;
    if value.is_nan() {
        // Preserve sign and the high 23 significand bits, including the quiet bit.
        // A payload that disappears in binary32 becomes a quiet NaN, never infinity.
        let bits = value.to_bits();
        let sign = (bits >> 32) as u32 & 0x8000_0000;
        let fraction = (bits >> 29) as u32 & 0x007f_ffff;
        let fraction = if fraction == 0 { 1 << 22 } else { fraction };
        return Ok(sign | f32::INFINITY.to_bits() | fraction);
    }
    let single = value as f32;
    if single.is_infinite() && value.is_finite() {
        if matches!(ctx.heap.get(ctx.scalar_value(id)), Value::Int(_)) {
            return Err(format_error(ctx, "int too large to convert"));
        }
        return Err(Error::new(
            "OverflowError",
            "float too large to pack with f format",
        ));
    }
    Ok(single.to_bits())
}

fn float_value(ctx: &mut Context, id: Id) -> Result<f64> {
    // PyFloat_AsDouble reads float subclasses directly, bypassing their __float__.
    if let Value::Float(value) = *ctx.heap.get(ctx.scalar_value(id)) {
        return Ok(value);
    }
    let Ok(Some(number)) = super::super::builtins::from_numeric(ctx, id) else {
        return Err(format_error(ctx, "required argument is not a float"));
    };
    let Value::Float(value) = *ctx.heap.get(number) else {
        unreachable!("float conversion returned a non-float")
    };
    Ok(value)
}

fn integer_field(
    ctx: &mut Context,
    id: Id,
    code: u8,
    width: usize,
    little_endian: bool,
) -> Result<u64> {
    let value = ctx.index_value(id).and_then(|value| {
        value.ok_or_else(|| format_error(ctx, "required argument is not an integer"))
    });
    if matches!(code, b'b' | b'h' | b'i' | b'l' | b'q') {
        let max = i64::MAX >> ((8 - width) * 8);
        let min = !max;
        let number = match value {
            Ok(value) => value.to_i64(),
            Err(error) => {
                let overflow = ctx.builtin("OverflowError");
                // CPython's non-native-endian q packer propagates __index__ errors.
                if (code == b'q' && little_endian != cfg!(target_endian = "little"))
                    || !ctx.raised_error_matches(&error, overflow)
                {
                    return Err(error);
                }
                None
            }
        };
        return number
            .filter(|n| (min..=max).contains(n))
            .map(|n| n as u64)
            .ok_or_else(|| {
                format_error(
                    ctx,
                    format!(
                        "'{}' format requires {min} <= number <= {max}",
                        char::from(code)
                    ),
                )
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
            .ok_or_else(|| format_error(ctx, "int too large to convert"));
    }
    Err(format_error(
        ctx,
        format!(
            "'{}' format requires 0 <= number <= {max}",
            char::from(code)
        ),
    ))
}
