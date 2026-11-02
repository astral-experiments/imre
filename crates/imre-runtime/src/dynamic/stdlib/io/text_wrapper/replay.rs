//! Reconstruct a seekable decoder state at the current character, including pending newlines.
use super::{Coder, Context, Cookie, Error, Id, Reader, RefCell, Result, Wrapper, decode_chunk};

fn overflow() -> Error {
    Error::new("OverflowError", "text stream position out of range")
}

fn count(
    ctx: &mut Context,
    input: &RefCell<Reader>,
    decoder: Coder,
    newline: Option<&str>,
    bytes: &[u8],
    end: bool,
) -> Result<usize> {
    let chunk = input.borrow_mut().chunk(bytes, decoder.codec, end)?;
    let mark = ctx.heap.temporary_mark();
    let result = decode_chunk(ctx, input, decoder, newline, chunk, end);
    ctx.heap.release_temporaries_since(mark);
    if let Err(error) = &result {
        for id in error.roots() {
            ctx.heap.keep(id);
        }
    }
    input.borrow_mut().consume_chunk();
    result
}

impl Wrapper {
    pub(super) fn position(ctx: &mut Context, receiver: Id, end: Id) -> Result<Id> {
        let w = Self::from_receiver(ctx, receiver)?;
        if !w.readable.get() || w.input.borrow().snapshot.is_none() {
            return Ok(end);
        }
        // Coercing a raw position can run Python and consume more text or replace this stream.
        let offset = ctx
            .index_integer(end)?
            .to_i64()
            .ok_or_else(|| Error::new("OverflowError", "int too big to convert"))?;
        let w = Self::from_receiver(ctx, receiver)?;
        let input = w.input.borrow();
        let Some(snapshot) = input.snapshot.clone() else {
            return Ok(ctx.int(offset));
        };
        let needed = input.used_in_chunk();
        let ratio = input.bytes_per_char;
        let seen = input.seen;
        drop(input);

        let bytes = snapshot.input.as_slice();
        let cookie = Cookie {
            start: offset
                .checked_sub(bytes.len() as i64)
                .ok_or_else(overflow)?,
            flags: snapshot.flags,
            ..Cookie::default()
        };
        if needed == 0 {
            return Ok(cookie.value(ctx));
        }

        let decoder = w.decoder.get();
        ctx.heap.keep(decoder.errors);
        let newline = w.decoder_newline.get();
        let replay = RefCell::new(Reader::default());
        replay.borrow_mut().seen = seen;

        let result = reconstruct(ctx, &replay, decoder, newline, cookie, bytes, needed, ratio);
        // Restoring IncrementalNewlineDecoder's saved flags leaves its newline
        // history as observed during the replay, even if decoding failed.
        w.input.borrow_mut().seen = replay.borrow().seen;
        result.map(|cookie| cookie.value(ctx))
    }
}

fn reconstruct(
    ctx: &mut Context,
    replay: &RefCell<Reader>,
    decoder: Coder,
    newline: Option<&str>,
    mut cookie: Cookie,
    bytes: &[u8],
    mut needed: usize,
    ratio: f64,
) -> Result<Cookie> {
    // Find a safe prefix, close to the logical position, with no partial code unit.
    let mut prefix = ((ratio * needed as f64) as usize).min(bytes.len());
    let mut back = 1usize;
    loop {
        replay
            .borrow_mut()
            .set_state(cookie, decoder.codec, newline);
        if prefix == 0 {
            break;
        }
        let chars = count(ctx, replay, decoder, newline, &bytes[..prefix], false)?;
        let state = replay.borrow();
        if chars <= needed {
            if state.tail.is_empty() {
                cookie.flags = state.flags(decoder.codec, newline);
                needed -= chars;
                break;
            }
            prefix = prefix.saturating_sub(state.tail.len());
            back = 1;
        } else {
            prefix = prefix.saturating_sub(back);
            back = back.saturating_mul(2);
        }
    }
    cookie.start = cookie
        .start
        .checked_add(prefix as i64)
        .ok_or_else(overflow)?;
    let mut chars = 0;
    for byte in &bytes[prefix..] {
        if needed == 0 {
            break;
        }
        chars += count(
            ctx,
            replay,
            decoder,
            newline,
            std::slice::from_ref(byte),
            false,
        )?;
        cookie.feed = cookie.feed.checked_add(1).ok_or_else(overflow)?;
        let state = replay.borrow();
        if state.tail.is_empty() && chars <= needed {
            cookie.start = cookie
                .start
                .checked_add(i64::from(cookie.feed))
                .ok_or_else(overflow)?;
            cookie.flags = state.flags(decoder.codec, newline);
            cookie.feed = 0;
            needed -= chars;
            chars = 0;
        }
        if chars >= needed {
            break;
        }
    }
    if chars < needed {
        chars += count(ctx, replay, decoder, newline, &[], true)?;
        if chars < needed {
            return Err(Error::new(
                "OSError",
                "can't reconstruct logical file position",
            ));
        }
        cookie.eof = true;
    }
    cookie.skip = i32::try_from(needed).map_err(|_| overflow())?;
    Ok(cookie)
}
