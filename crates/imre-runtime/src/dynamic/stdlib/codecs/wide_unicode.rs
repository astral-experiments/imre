//! UTF-16 and UTF-32 decoding into Python code points.
use super::{Context, Id, Result, Value, resume_decode, type_error};
use crate::objects::{FixedUnicode, Utf16Order};
use FixedUnicode::{Utf16, Utf32Be, Utf32Le};
use Utf16Order::{Big, Little};

/// Both wide codec families use the signed byte-order argument from the CPython API.
fn arguments(function: &str, args: &[Id], kwargs: &[(String, Id)], maximum: usize) -> Result<()> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "_codecs.{function}() takes no keyword arguments"
        )));
    }
    if args.is_empty() {
        return Err(type_error(format!(
            "{function} expected at least 1 argument, got 0"
        )));
    }
    if args.len() > maximum {
        return Err(type_error(format!(
            "{function} expected at most {maximum} arguments, got {}",
            args.len()
        )));
    }
    Ok(())
}

fn order(ctx: &mut Context, arg: Option<&Id>) -> Result<i32> {
    arg.map(|&id| super::super::os::c_int(ctx, id))
        .transpose()
        .map(Option::unwrap_or_default)
}

/// Fixed-order functions and their ordered counterparts have distinct argument and result shapes.
pub(in crate::dynamic) enum WideFunction {
    FixedEncode(FixedUnicode),
    FixedDecode(FixedUnicode),
    OrderedEncode(FixedUnicode),
    ExtendedDecode(FixedUnicode),
}

impl WideFunction {
    pub(in crate::dynamic) fn named(function: &str) -> Option<Self> {
        Some(match function {
            "utf_16_encode" => Self::OrderedEncode(Utf16(Little)),
            "utf_16_ex_decode" => Self::ExtendedDecode(Utf16(Little)),
            "utf_32_encode" => Self::OrderedEncode(Utf32Le),
            "utf_32_ex_decode" => Self::ExtendedDecode(Utf32Le),
            "utf_16_le_encode" => Self::FixedEncode(Utf16(Little)),
            "utf_16_le_decode" => Self::FixedDecode(Utf16(Little)),
            "utf_16_be_encode" => Self::FixedEncode(Utf16(Big)),
            "utf_16_be_decode" => Self::FixedDecode(Utf16(Big)),
            "utf_32_le_encode" => Self::FixedEncode(Utf32Le),
            "utf_32_le_decode" => Self::FixedDecode(Utf32Le),
            "utf_32_be_encode" => Self::FixedEncode(Utf32Be),
            "utf_32_be_decode" => Self::FixedDecode(Utf32Be),
            _ => return None,
        })
    }

    pub(in crate::dynamic::stdlib) fn call(
        self,
        ctx: &mut Context,
        function: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        match self {
            Self::FixedEncode(format) | Self::OrderedEncode(format) => {
                let ordered = matches!(self, Self::OrderedEncode(_));
                arguments(function, args, kwargs, if ordered { 3 } else { 2 })?;
                let input = args[0];
                let length = super::encoder_input_length(ctx, function, input)?;
                let errors = policy(ctx, function, args.get(1).copied())?;
                let byteorder = if ordered {
                    order(ctx, args.get(2))?
                } else {
                    format.byte_order()
                };
                let errors = super::error_name(ctx, errors)?.to_owned();
                let bytes = super::encode_fixed(ctx, input, format, &errors, byteorder)?;
                let data = ctx.heap.bytes_from_data(bytes);
                let consumed = ctx.alloc(Value::Int(crate::Int::from_usize(length)));
                Ok(ctx.alloc(Value::Tuple(vec![data, consumed])))
            }
            Self::FixedDecode(format) | Self::ExtendedDecode(format) => {
                let extended = matches!(self, Self::ExtendedDecode(_));
                let maximum = if extended { 4 } else { 3 };
                arguments(function, args, kwargs, maximum)?;
                let input = args[0];
                ctx.check_buffer_exporter(input)?;
                ctx.native_buffer_bytes(input)?;
                ctx.with_buffer_export(input, |ctx| {
                    let errors = policy(ctx, function, args.get(1).copied())?;
                    let errors = super::error_name(ctx, errors)?.to_owned();
                    let byteorder = if extended {
                        order(ctx, args.get(2))?
                    } else {
                        format.byte_order()
                    };
                    let final_chunk = args
                        .get(maximum - 1)
                        .map(|&id| ctx.truth(id))
                        .transpose()?
                        .unwrap_or(false);
                    let (text, consumed, byteorder) =
                        decode_chunk(ctx, input, &errors, format, byteorder, final_chunk)?;
                    let consumed = ctx.alloc(Value::Int(crate::Int::from_usize(consumed)));
                    let mut result = vec![text, consumed];
                    if extended {
                        result.push(ctx.int(i64::from(byteorder)));
                    }
                    Ok(ctx.alloc(Value::Tuple(result)))
                })
            }
        }
    }
}

fn policy(ctx: &mut Context, function: &str, arg: Option<Id>) -> Result<Option<Id>> {
    if let Some(id) = arg
        && matches!(ctx.heap.get(ctx.scalar_value(id)), Value::SurrogateStr(_))
    {
        ctx.encode_utf8_codepoints(id, false, "strict")?;
    }
    super::entry_error_policy(ctx, function, arg)
}

pub(super) fn decode(
    ctx: &mut Context,
    input: Id,
    errors: &str,
    format: FixedUnicode,
    bom: bool,
) -> Result<Id> {
    let order = if bom { 0 } else { format.byte_order() };
    decode_chunk(ctx, input, errors, format, order, true).map(|(text, _, _)| text)
}

/// Decode complete units and report both the consumed bytes and the detected byte order.
/// An order of zero remains undecided until a BOM is consumed.
pub(in crate::dynamic::stdlib) fn decode_chunk(
    ctx: &mut Context,
    input: Id,
    errors: &str,
    format: FixedUnicode,
    order: i32,
    final_chunk: bool,
) -> Result<(Id, usize, i32)> {
    let data = ctx.native_buffer_bytes(input)?;
    let (order, start) = if order == 0 {
        match (format.width(), data) {
            (2, [0xff, 0xfe, ..]) => (-1, 2),
            (2, [0xfe, 0xff, ..]) => (1, 2),
            (4, [0xff, 0xfe, 0, 0, ..]) => (-1, 4),
            (4, [0, 0, 0xfe, 0xff, ..]) => (1, 4),
            _ => (order, 0),
        }
    } else {
        (order, 0)
    };
    let format = format.with_order(order);
    let size = data.len();
    let mut points = Vec::new();
    let (mut consumed, issue) = format.decode_prefix(&mut points, data, start, final_chunk)?;
    let text = if let Some(issue) = issue {
        let error = issue.error(data);
        resume_decode(ctx, errors, points, error, |out, bytes, pos| {
            let (end, issue) = format.decode_prefix(out, bytes, pos, final_chunk)?;
            consumed = end;
            Ok(issue)
        })?
    } else {
        ctx.text_codepoints(points)
    };
    Ok((text, if final_chunk { size } else { consumed }, order))
}

/// surrogatepass accepts UTF-name variants but does not consult arbitrary codec aliases.
pub(super) fn surrogate_encoding(name: &str) -> Option<FixedUnicode> {
    let (prefix, rest) = name.split_at_checked(3)?;
    if !prefix.eq_ignore_ascii_case("utf") {
        return None;
    }
    let rest = rest.strip_prefix(['-', '_']).unwrap_or(rest);
    let (width, end) = rest.split_at_checked(2)?;
    let little = if end.is_empty() {
        cfg!(target_endian = "little")
    } else {
        let end = end.strip_prefix(['-', '_']).unwrap_or(end);
        if end.eq_ignore_ascii_case("le") {
            true
        } else if end.eq_ignore_ascii_case("be") {
            false
        } else {
            return None;
        }
    };
    match (width, little) {
        ("16", true) => Some(Utf16(Little)),
        ("16", false) => Some(Utf16(Big)),
        ("32", true) => Some(Utf32Le),
        ("32", false) => Some(Utf32Be),
        _ => None,
    }
}
