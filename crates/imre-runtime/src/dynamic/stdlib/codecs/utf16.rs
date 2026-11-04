//! Box the stateful UTF-16 decoder used to select an incremental stream's byte order.
use super::{Context, Id, Result, Utf16Order, resume_decode};

fn data(ctx: &Context, input: Id) -> Result<&[u8]> {
    ctx.native_buffer_bytes(input)
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
