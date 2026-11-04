//! LZMA codecs used by the public lzma module and ZIP method 14.
mod filters;
mod streams;

pub(in crate::dynamic) use filters::properties;
use liblzma::stream::{Action, Error as CodecError, Status, Stream};
use streams::DecoderOptions;
pub(in crate::dynamic) use streams::{
    Compressor, Decompressor, compressor, decompressor, decompressor_attr,
};

use super::{Context, Error, Id, NONE, Result, Value, builtins, os};

const LZMA_ERROR: &str = "_lzma.LZMAError";

pub(super) fn call(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [data, format, memlimit, filters] = builtins::bind(
        method,
        args,
        kwargs,
        ["data", "format", "memlimit", "filters"],
    )?;
    let data = data.ok_or_else(|| {
        Error::new(
            "TypeError",
            "decompress() missing 1 required positional argument: 'data'",
        )
    })?;
    let format = format
        .map(|id| os::c_int(ctx, id))
        .transpose()?
        .unwrap_or(0);
    let options = DecoderOptions::new(ctx, format, memlimit, filters.unwrap_or(NONE))?;
    let output = streams::with_input(ctx, data, |ctx, view| {
        decompress(ctx.native_buffer_bytes(view)?, &options)
    })?;
    Ok(ctx.alloc(Value::Bytes(output)))
}

fn decompress(mut input: &[u8], options: &DecoderOptions) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut first = true;
    loop {
        let start = output.len();
        let mut stream = options.stream(0)?;
        match decode_member(&mut stream, input, &mut output) {
            Ok(true) => {}
            Ok(false) => {
                return Err(Error::new(
                    LZMA_ERROR,
                    "Compressed data ended before the end-of-stream marker was reached",
                ));
            }
            Err(err) if !first && err.kind.as_ref() == LZMA_ERROR => {
                // CPython discards a failing trailing member, including any partial output.
                output.truncate(start);
                return Ok(output);
            }
            Err(err) => return Err(err),
        }
        input = &input[stream.total_in() as usize..];
        if input.is_empty() {
            return Ok(output);
        }
        first = false;
    }
}

/// Returns whether the decoder reached the end-of-stream marker.
fn decode_member(stream: &mut Stream, input: &[u8], output: &mut Vec<u8>) -> Result<bool> {
    loop {
        if output.len() == output.capacity() {
            output
                .try_reserve(32 * 1024)
                .map_err(|_| Error::new("MemoryError", ""))?;
        }
        let remaining = &input[stream.total_in() as usize..];
        let status = stream
            .process_vec(remaining, output, Action::Run)
            .map_err(codec_error)?;
        if status == Status::StreamEnd {
            return Ok(true);
        }
        if stream.total_in() as usize == input.len() && output.len() < output.capacity() {
            return Ok(false);
        }
        if status == Status::MemNeeded {
            return Err(Error::new(LZMA_ERROR, "Insufficient buffer space"));
        }
    }
}

fn codec_error(err: CodecError) -> Error {
    crate::objects::lzma::codec_error(err).into()
}
