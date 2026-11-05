//! Bound methods retain the parsed binary layout and convert arguments in field order.
use std::ops::Range;
use std::rc::Rc;

use num_traits::ToPrimitive;

use super::super::builtins::{bind_builtin_arguments, boolean, one_positional_arg};
use super::super::complex::Complex;
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
    let value = initialized(ctx, args, kwargs)?;
    Ok(ctx.alloc(value))
}

fn initialized(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Value> {
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
    Ok(Value::Struct {
        format: format.to_owned(),
        layout: Some(Rc::new(layout)),
    })
}

/// Read the underlying format spelling and validate the layout before inspecting the buffer.
pub(super) fn parse(ctx: &mut Context, format: Id) -> Result<(&str, StructFormat)> {
    let scalar = ctx.scalar_value(format);
    let scalar = match ctx.heap.get(scalar) {
        Value::Str(text) if text.is_ascii() => scalar,
        Value::Str(_) | Value::SurrogateStr(_) => {
            let bytes = ctx.encode_legacy_codepoints(format, Codec::Ascii, false)?;
            ctx.heap.bytes_from_data(bytes)
        }
        _ => scalar,
    };
    let format = match ctx.heap.get(scalar) {
        Value::Str(text) => text.as_str(),
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
            UnpackedField::Complex(real, imag) => ctx.alloc(Value::Complex(Complex { real, imag })),
            UnpackedField::Byte(byte) => ctx.heap.bytes_from_data([byte]),
            UnpackedField::Bytes(bytes) => ctx.heap.bytes_from_data(bytes),
        })
        .collect();
    ctx.alloc(Value::Tuple(items))
}

pub(super) fn module_iter_unpack(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "_struct.iter_unpack() takes no keyword arguments",
        ));
    }
    let [_, buffer] = args else {
        return Err(Error::new(
            "TypeError",
            format!("iter_unpack expected 2 arguments, got {}", args.len()),
        ));
    };
    let record = new(ctx, &args[..1], &[])?;
    iter_unpack(ctx, record, *buffer)
}

/// A live iterator reads the current layout, including after a successful reinitialization.
fn iterator_layout(ctx: &Context, record: Id) -> &StructFormat {
    let Value::Struct {
        layout: Some(layout),
        ..
    } = ctx.heap.get(ctx.scalar_value(record))
    else {
        unreachable!("iterator holds an initialized Struct")
    };
    layout
}

pub(super) fn iterator_hint(ctx: &Context, id: Id) -> usize {
    let Value::StructUnpackIterator {
        record: Some(record),
        offset,
        length,
        ..
    } = *ctx.heap.get(id)
    else {
        return 0;
    };
    (length - offset)
        .checked_div(iterator_layout(ctx, record).size)
        .unwrap_or(0)
}

fn iter_unpack(ctx: &mut Context, record: Id, source: Id) -> Result<Id> {
    let size = iterator_layout(ctx, record).size;
    if size == 0 {
        return Err(format_error(
            ctx,
            "cannot iteratively unpack with a struct of length 0",
        ));
    }

    let (buffer, exporter) = if let Some(method) = ctx.python_buffer_method(source, true)? {
        (ctx.acquire_python_buffer(source, method, 0)?, Some(source))
    } else {
        (source, None)
    };
    // The iterator exports the original view, so even memoryview.release() must wait.
    let length = ctx.native_buffer_bytes(buffer)?.len();
    if !length.is_multiple_of(size) {
        if let Some(exporter) = exporter {
            ctx.release_python_buffer(exporter, buffer);
        }
        return Err(format_error(
            ctx,
            format!("iterative unpacking requires a buffer of a multiple of {size} bytes"),
        ));
    }
    ctx.heap.note_buffer_view(ctx.view_storage_id(buffer));
    let id = ctx.alloc(Value::StructUnpackIterator {
        record: Some(record),
        buffer: Some(buffer),
        exporter,
        offset: 0,
        length,
    });
    if exporter.is_some() {
        ctx.heap.register_finalizer(id);
    }
    Ok(id)
}

pub(in super::super) fn advance_iterator(ctx: &mut Context, id: Id) -> Result<Option<Id>> {
    let Value::StructUnpackIterator {
        record: Some(record),
        buffer: Some(buffer),
        offset,
        length,
        ..
    } = *ctx.heap.get(id)
    else {
        return Ok(None);
    };
    if offset >= length {
        release_iterator(ctx, id);
        return Ok(None);
    }
    let layout = iterator_layout(ctx, record);
    if layout.size == 0 {
        return Err(format_error(
            ctx,
            "cannot iteratively unpack with a struct of length 0",
        ));
    }
    let bytes = ctx.native_buffer_bytes(buffer)?;
    let end = offset + layout.size;
    let bytes = bytes
        .get(offset..end)
        .ok_or_else(|| Error::new("BufferError", "exported buffer changed size"))?;
    let fields = layout.unpack(bytes);
    let Value::StructUnpackIterator { offset, .. } = ctx.heap.get_mut(id) else {
        unreachable!()
    };
    *offset = end;
    let fields = fields.map_err(|e| format_error(ctx, e))?;
    Ok(Some(unpacked_tuple(ctx, fields)))
}

pub(in super::super) fn release_iterator(ctx: &mut Context, id: Id) {
    let Value::StructUnpackIterator {
        record,
        buffer,
        exporter,
        ..
    } = ctx.heap.get_mut(id)
    else {
        unreachable!()
    };
    // Detach before invoking a Python release hook, which may call next() again.
    let (record, buffer, exporter) = (record.take(), buffer.take(), exporter.take());
    if let Some(record) = record {
        ctx.heap.weak_edge_removed(record);
    }
    let Some(buffer) = buffer else { return };
    ctx.heap.weak_edge_removed(buffer);
    if let Some(exporter) = exporter {
        ctx.heap.weak_edge_removed(exporter);
        if ctx.heap.is_pending_finalizer(id) {
            ctx.heap.take_finalizer(id, true);
        }
        ctx.heap.keep(buffer);
        ctx.heap.keep(exporter);
        ctx.release_python_buffer(exporter, buffer);
    }
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

fn uninitialized() -> Error {
    Error::new("RuntimeError", "Struct object is not initialized")
}

/// Render the stored format even when a subclass overrides the format property.
pub(in crate::dynamic) fn repr(ctx: &Context, receiver: Id) -> Result<String> {
    let Value::Struct { format, layout } = ctx.heap.get(ctx.scalar_value(receiver)) else {
        unreachable!("validated Struct repr receiver")
    };
    layout.as_ref().ok_or_else(uninitialized)?;
    let name = ctx.type_name(receiver).rsplit('.').next().unwrap();
    let quoted = crate::objects::exceptions::string_repr(format);
    Ok(format!("{name}({quoted})"))
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "__repr__" {
        return super::super::builtins::call(ctx, "struct.Struct.__repr__", args, kwargs);
    }
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method Struct.{name}() needs an argument"),
        ));
    };
    let storage = ctx.scalar_value(receiver);
    let Value::Struct { format, layout } = ctx.heap.get(storage) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for '_struct.Struct' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    };
    if name == "__init__" {
        // Install the new format and layout together only after parsing succeeds.
        let value = initialized(ctx, args, kwargs)?;
        *ctx.heap.get_mut(storage) = value;
        return Ok(NONE);
    }
    if name == "size" {
        return Ok(ctx.int(layout.as_ref().map_or(-1, |layout| layout.size as i64)));
    }
    let layout = layout.as_ref().ok_or_else(uninitialized)?;
    if name == "format" {
        return Ok(ctx.text(format.to_owned()));
    }
    if name == "iter_unpack" {
        let buffer = one_positional_arg("Struct.iter_unpack", args, kwargs)?;
        return iter_unpack(ctx, receiver, buffer);
    }
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
            pack_number(ctx, layout, id, code, range, |_, range, word| {
                layout.write_integer(&mut output[range], word);
                Ok(())
            })?;
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
            let field = start + range.start..start + range.end;
            if matches!(code, b's' | b'p') {
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
                pack_number(ctx, layout, id, code, field, |ctx, field, word| {
                    let buffer = ctx.writable_storage_bytes(storage)?;
                    layout.write_integer(&mut buffer[field], word);
                    Ok(())
                })?;
            }
        }
        Ok(NONE)
    })
}

/// Convert each number once, writing the real component before narrowing the imaginary one.
fn pack_number(
    ctx: &mut Context,
    layout: &StructFormat,
    id: Id,
    code: u8,
    range: Range<usize>,
    mut write: impl FnMut(&mut Context, Range<usize>, u64) -> Result<()>,
) -> Result<()> {
    if !matches!(code, b'F' | b'D') {
        let word = match code {
            b'c' => match ctx.heap.get(ctx.scalar_value(id)) {
                Value::Bytes(bytes) if bytes.len() == 1 => u64::from(bytes[0]),
                _ => {
                    return Err(format_error(
                        ctx,
                        "char format requires a bytes object of length 1",
                    ));
                }
            },
            b'?' => u64::from(ctx.truth(id)?),
            b'e' => u64::from(half_bits(ctx, id)?),
            b'f' => u64::from(single_bits(ctx, id)?),
            b'd' => float_value(ctx, id)?.to_bits(),
            _ => {
                return pack_integer(
                    ctx,
                    id,
                    code,
                    range.len(),
                    layout.little_endian,
                    |ctx, word| write(ctx, range, word),
                );
            }
        };
        return write(ctx, range, word);
    }
    let value = complex_value(ctx, id)
        .ok_or_else(|| format_error(ctx, "required argument is not a complex"))?;
    let middle = range.start + range.len() / 2;
    for (part, component) in [
        (range.start..middle, value.real),
        (middle..range.end, value.imag),
    ] {
        let word = if code == b'D' {
            component.to_bits()
        } else if layout.little_endian == cfg!(target_endian = "little") {
            // CPython uses a native float cast for same-endian complex fields, even with '='.
            u64::from((component as f32).to_bits())
        } else {
            u64::from(checked_single_bits(ctx, id, component)?)
        };
        write(ctx, part, word)?;
    }
    Ok(())
}

/// struct uses the numeric protocol and converts any coercion failure to struct.error.
fn complex_value(ctx: &mut Context, id: Id) -> Option<Complex> {
    if let Value::Complex(number) = *ctx.heap.get(ctx.scalar_value(id)) {
        return Some(number);
    }
    let Some(method) = ctx.bound_slot(id, "__complex__").ok()? else {
        return float_value(ctx, id).ok().map(Complex::from);
    };
    let result = ctx.call(method, Vec::new(), Vec::new()).ok()?;
    let scalar = ctx.scalar_value(result);
    let Value::Complex(number) = *ctx.heap.get(scalar) else {
        return None;
    };
    if result != scalar {
        let message = format!(
            "__complex__ returned non-complex (type {:.200}).  The ability to return an instance of a strict subclass of complex is deprecated, and may be removed in a future version of Python.",
            ctx.type_name(result)
        );
        ctx.emit_builtin_warning(
            crate::objects::exceptions::Kind::DeprecationWarning,
            &message,
            1,
        )
        .ok()?;
    }
    Some(number)
}

fn float_overflow(ctx: &Context, id: Id, code: char) -> Error {
    if matches!(ctx.heap.get(ctx.scalar_value(id)), Value::Int(_)) {
        format_error(ctx, "int too large to convert")
    } else {
        Error::new(
            "OverflowError",
            format!("float too large to pack with {code} format"),
        )
    }
}

fn half_bits(ctx: &mut Context, id: Id) -> Result<u16> {
    let value = float_value(ctx, id)?;
    let bits = value.to_bits();
    let sign = ((bits >> 48) as u16) & 0x8000;
    if !value.is_finite() {
        let payload = ((bits >> 42) as u16) & 0x03ff;
        // A NaN whose high payload bits vanish must not turn into infinity.
        let payload = if value.is_nan() && payload == 0 {
            0x0200
        } else {
            payload
        };
        return Ok(sign | 0x7c00 | payload);
    }

    let magnitude = value.abs();
    // 65520 is halfway between the largest finite half and the next power of two.
    if magnitude >= 65520.0 {
        return Err(float_overflow(ctx, id, 'e'));
    }
    // Round in units of one half-precision ULP. Subnormals all use 2^-24.
    let exponent = ((magnitude.to_bits() >> 52) as i32 - 1023).max(-14);
    let units = (magnitude * 2f64.powi(10 - exponent)).round_ties_even() as u16;
    // units includes the leading bit for normals; adding also carries across powers of two.
    let base = ((exponent + 14) as u16) << 10;
    Ok(sign | (base + units))
}

fn single_bits(ctx: &mut Context, id: Id) -> Result<u32> {
    let value = float_value(ctx, id)?;
    checked_single_bits(ctx, id, value)
}

fn checked_single_bits(ctx: &Context, id: Id, value: f64) -> Result<u32> {
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
        return Err(float_overflow(ctx, id, 'f'));
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

fn pack_integer(
    ctx: &mut Context,
    id: Id,
    code: u8,
    width: usize,
    little_endian: bool,
    write: impl FnOnce(&mut Context, u64) -> Result<()>,
) -> Result<()> {
    let signed = matches!(code, b'b' | b'h' | b'i' | b'l' | b'q' | b'n');
    let limit = 1_i128 << (width * 8 - usize::from(signed));
    let min = if signed { -limit } else { 0 };
    let max = limit - 1;
    let range_error = |ctx: &Context| {
        format_error(
            ctx,
            format!(
                "'{}' format requires {min} <= number <= {max}",
                char::from(code)
            ),
        )
    };
    let swapped_long_long =
        matches!(code, b'q' | b'Q') && little_endian != cfg!(target_endian = "little");
    let value = match ctx.index_value(id) {
        Ok(Some(value)) => value,
        Ok(None) => return Err(format_error(ctx, "required argument is not an integer")),
        Err(error) => {
            // The byte-array and pointer packers propagate errors from __index__ itself.
            let overflow = ctx.builtin("OverflowError");
            if !swapped_long_long && code != b'P' && ctx.raised_error_matches(&error, overflow) {
                return Err(range_error(ctx));
            }
            return Err(error);
        }
    };
    let number = value
        .as_small()
        .map(i128::from)
        .or_else(|| value.big().to_i128());
    if let Some(number) = number.filter(|n| (min..=max).contains(n)) {
        return write(ctx, number as u64);
    }
    if code == b'P' {
        let pointer = value
            .to_i64()
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| format_error(ctx, "int too large to convert"))?;
        return write(ctx, pointer as usize as u64);
    }
    if swapped_long_long {
        // PyLong_AsByteArray writes the low bytes before reporting overflow, except
        // when rejecting a negative number for an unsigned field.
        let big = value.big();
        let negative = big.sign() == num_bigint::Sign::Minus;
        if signed || !negative {
            let low = big.magnitude().iter_u64_digits().next().unwrap_or(0);
            write(ctx, if negative { low.wrapping_neg() } else { low })?;
        }
    }
    Err(range_error(ctx))
}
