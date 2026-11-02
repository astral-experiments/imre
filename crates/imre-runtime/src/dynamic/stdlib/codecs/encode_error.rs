//! Resume a boxed native writer's encoder after a registered Python error handler.
use super::{Codec, Context, Error, Id, NONE, Result, Value, handler_builtin, type_error};
use crate::dynamic::value::DecodeState;

pub(super) enum Handler {
    Builtin(&'static str),
    Python(Id),
}

pub(super) fn registered(ctx: &Context, codec: Codec, name: &str) -> Option<Handler> {
    let callback = *ctx.codec_entries.handlers.get(name)?;
    // The C encoders implement these named policies inline, bypassing the registry.
    if matches!(name, "replace" | "ignore" | "xmlcharrefreplace")
        || name == "strict" && !matches!(codec, Codec::Utf8 { .. })
        || name == "backslashreplace" && !matches!(codec, Codec::CodePage(_))
        || name == "surrogatepass" && matches!(codec, Codec::Utf8 { .. })
    {
        return None;
    }
    if let Value::Native(symbol) = ctx.heap.get(callback) {
        for builtin in [
            "strict",
            "ignore",
            "replace",
            "xmlcharrefreplace",
            "backslashreplace",
            "namereplace",
            "surrogateescape",
            "surrogatepass",
        ] {
            if handler_builtin(builtin) == Some(symbol.as_str()) {
                return Some(Handler::Builtin(builtin));
            }
        }
    }
    Some(Handler::Python(callback))
}

fn append(output: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    output
        .try_reserve(bytes.len())
        .map_err(|_| crate::Error::allocation())?;
    output.extend_from_slice(bytes);
    Ok(())
}

// If output is None, test representability without emitting any data.
fn point(codec: Codec, code: u32, output: Option<&mut Vec<u8>>) -> Result<bool> {
    let Some(ch) = char::from_u32(code) else {
        return Ok(false);
    };
    let mut scratch = [0; 4];
    let bytes: &[u8] = if matches!(codec, Codec::Utf8 { .. }) {
        ch.encode_utf8(&mut scratch).as_bytes()
    } else if let Some(byte) = codec.encode_byte(ch) {
        scratch[0] = byte;
        &scratch[..1]
    } else {
        return Ok(false);
    };
    if let Some(output) = output {
        append(output, bytes)?;
    }
    Ok(true)
}

pub(super) fn encode(
    ctx: &mut Context,
    input: Id,
    codec: Codec,
    callback: Id,
    escape_prefix: bool,
) -> Result<Vec<u8>> {
    let source = ctx.string_codepoints(input)?.into_owned();
    let mut output = Vec::new();
    if matches!(codec, Codec::Utf8 { signature: true }) {
        append(&mut output, b"\xef\xbb\xbf")?;
    }
    let (encoding, reason) = if matches!(codec, Codec::Utf8 { .. }) {
        ("utf-8", "surrogates not allowed")
    } else {
        codec.single_byte_error()
    };
    let mark = ctx.heap.temporary_mark();
    ctx.heap.reserve_temporaries(3)?;
    ctx.heap.keep(callback);
    // A Python return and each allocated error detail are rooted in the native caller.
    // Keep only the callback and its reusable exception between iterations.
    let iteration_mark = ctx.heap.temporary_mark();
    let mut exception = None;
    let result = (|| {
        let mut pos = 0;
        while pos < source.len() {
            if point(codec, source[pos], Some(&mut output))? {
                pos += 1;
                continue;
            }
            let mut end = pos + 1;
            while end < source.len() && !point(codec, source[end], None)? {
                end += 1;
            }
            if escape_prefix && !matches!(codec, Codec::CodePage(_)) {
                while pos < end && (0xdc80..=0xdcff).contains(&source[pos]) {
                    append(&mut output, &[source[pos] as u8])?;
                    pos += 1;
                }
                if pos == end {
                    continue;
                }
            }
            let mut error =
                ctx.string_encoding_error(input, encoding, reason, source[pos], pos..end);
            let [enc, _, _, _, why] = *ctx
                .exception_arguments(&error)
                .expect("encoding error arguments")
            else {
                unreachable!()
            };
            let exc = if let Some(exc) = exception {
                ctx.advance_decode_error(exc, pos, end, reason);
                let Value::Raised(active) = ctx.heap.get_mut(exc) else {
                    unreachable!()
                };
                active.message = error.message;
                exc
            } else {
                // The exception reuses writable fields, but args retains its original values.
                error.decode = Some(Box::new(DecodeState {
                    encoding: enc,
                    object: input,
                    start: pos as i64,
                    end: end as i64,
                    reason: why,
                }));
                let exc = ctx.alloc(Value::Raised(error));
                exception = Some(exc);
                exc
            };
            let response = ctx.call(callback, vec![exc], Vec::new())?;
            ctx.heap.keep(response);
            let invalid =
                || type_error("encoding error handler must return (str/bytes, int) tuple");
            let items = ctx.tuple_items(response)?.ok_or_else(invalid)?;
            let [replacement, resume] = items.as_slice() else {
                return Err(invalid());
            };
            let resume = ctx.index_integer(*resume)?.to_i64().ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?;
            let value = ctx.scalar_value(*replacement);
            if !matches!(
                ctx.heap.get(value),
                Value::Bytes(_) | Value::Str(_) | Value::SurrogateStr(_)
            ) {
                return Err(invalid());
            }
            // Encoding resumes in the original string even when the handler rebinds exc.object.
            let resume = if resume < 0 {
                source.len() as i64 + resume
            } else {
                resume
            };
            if !(0..=source.len() as i64).contains(&resume) {
                return Err(Error::new(
                    "IndexError",
                    format!("position {resume} from error handler out of bounds"),
                ));
            }
            if let Value::Bytes(bytes) = ctx.heap.get(value) {
                append(&mut output, bytes)?;
            } else {
                for &code in ctx.string_codepoints(value)?.iter() {
                    if matches!(codec, Codec::Utf8 { .. }) && code > 0x7f
                        || !point(codec, code, Some(&mut output))?
                    {
                        ctx.advance_decode_error(exc, pos, end, reason);
                        return ctx.op("raise", exc, NONE, NONE).map(|_| unreachable!());
                    }
                }
            }
            ctx.heap.release_temporaries_since(iteration_mark);
            ctx.heap.keep(exc);
            pos = resume as usize;
        }
        Ok(output)
    })();
    ctx.heap.release_temporaries_since(mark);
    result
}
