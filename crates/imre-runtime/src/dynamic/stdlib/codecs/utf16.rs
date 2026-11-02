//! Box the stateful UTF-16 decoder used to select an incremental stream's byte order.
use super::{Context, Id, NONE, Result, Utf16Order, Value, resume_decode, type_error};

fn data(ctx: &Context, input: Id) -> Result<&[u8]> {
    match ctx.buffer_storage(input)? {
        Value::Array(array) => Ok(array.as_bytes()),
        _ => ctx.bytes_like(input),
    }
}

pub(in crate::dynamic::stdlib) fn extended(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(
            "_codecs.utf_16_ex_decode() takes no keyword arguments",
        ));
    }
    if !(1..=4).contains(&args.len()) {
        let bound = if args.is_empty() {
            "at least 1 argument"
        } else {
            "at most 4 arguments"
        };
        return Err(type_error(format!(
            "utf_16_ex_decode expected {bound}, got {}",
            args.len()
        )));
    }
    let input = args[0];
    ctx.check_buffer_exporter(input)?;
    data(ctx, input)?;
    ctx.with_buffer_export(input, |ctx| {
        let errors = match args.get(1).copied() {
            None | Some(NONE) => "strict",
            Some(id) if ctx.string_storage(id).is_some() => {
                if matches!(ctx.heap.get(ctx.scalar_value(id)), Value::SurrogateStr(_)) {
                    ctx.encode_utf8_codepoints(id, false, "strict")?;
                }
                super::error_name(ctx, Some(id))?
            }
            Some(id) => {
                return Err(type_error(format!(
                    "utf_16_ex_decode() argument 2 must be str or None, not {}",
                    ctx.type_name(id)
                )));
            }
        }
        .to_owned();
        let byteorder = args
            .get(2)
            .map(|&id| super::super::os::c_int(ctx, id))
            .transpose()?
            .unwrap_or(0);
        let final_chunk = args
            .get(3)
            .map(|&id| ctx.truth(id))
            .transpose()?
            .unwrap_or(false);
        let (text, consumed, byteorder) = decode(ctx, input, &errors, byteorder, final_chunk)?;
        let consumed = ctx.alloc(Value::Int(crate::Int::from_usize(consumed)));
        let order = ctx.int(i64::from(byteorder));
        Ok(ctx.alloc(Value::Tuple(vec![text, consumed, order])))
    })
}

/// Boxed named codecs and the incremental wrapper share BOM detection and callback state.
pub(super) fn decode(
    ctx: &mut Context,
    input: Id,
    errors: &str,
    byteorder: i32,
    final_chunk: bool,
) -> Result<(Id, usize, i32)> {
    let bytes = data(ctx, input)?;
    let size = bytes.len();
    let (byteorder, start) = match (byteorder, bytes.first_chunk::<2>()) {
        (0, Some([0xff, 0xfe])) => (-1, 2),
        (0, Some([0xfe, 0xff])) => (1, 2),
        _ => (byteorder, 0),
    };
    let order = Utf16Order::from_sign(byteorder);
    let mut points = Vec::new();
    let (mut consumed, issue) = order.decode_prefix(&mut points, bytes, start, final_chunk)?;
    let text = if let Some(issue) = issue {
        let error = issue.error(bytes);
        resume_decode(ctx, errors, points, error, |points, bytes, offset| {
            let (end, issue) = order.decode_prefix(points, bytes, offset, final_chunk)?;
            consumed = end;
            Ok(issue)
        })?
    } else {
        ctx.text_codepoints(points)
    };
    // In final mode the C wrapper reports the original buffer size, even when the
    // handler replaced exc.object. Incremental mode reports the resumed input offset.
    let consumed = if final_chunk { size } else { consumed };
    Ok((text, consumed, byteorder))
}
