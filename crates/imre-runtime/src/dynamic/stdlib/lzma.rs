//! One-shot XZ and LZMA-alone decompression for the public lzma module.
use liblzma::stream::{Action, Error as CodecError, Status, Stream};
use num_traits::{Signed, ToPrimitive};

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
    let memlimit = match memlimit {
        Some(id) if id != NONE => {
            if format == 3 {
                return Err(Error::new(
                    "ValueError",
                    "Cannot specify memory limit with FORMAT_RAW",
                ));
            }
            let number = ctx.index_integer(id)?;
            let number = number.big();
            if number.is_negative() {
                return Err(Error::new("ValueError", "Cannot convert negative int"));
            }
            number
                .to_u64()
                .ok_or_else(|| Error::new("OverflowError", "Python int too large for C uint64_t"))?
        }
        _ => u64::MAX,
    };
    let filters = filters.unwrap_or(NONE);
    if format == 3 {
        return Err(if filters == NONE {
            Error::new("ValueError", "Must specify filters for FORMAT_RAW")
        } else {
            Error::new(
                "NotImplementedError",
                "raw LZMA filter chains are not supported",
            )
        });
    }
    if filters != NONE {
        return Err(Error::new(
            "ValueError",
            "Cannot specify filters except with FORMAT_RAW",
        ));
    }
    let decoder: Decoder = match format {
        0 => |limit| Stream::new_auto_decoder(limit, 0),
        1 => |limit| Stream::new_stream_decoder(limit, 0),
        2 => Stream::new_lzma_decoder,
        _ => {
            return Err(Error::new(
                "ValueError",
                format!("Invalid container format: {format}"),
            ));
        }
    };
    ctx.check_buffer_exporter(data)?;
    let output = decompress(ctx.native_buffer_bytes(data)?, decoder, memlimit)?;
    Ok(ctx.alloc(Value::Bytes(output)))
}

type Decoder = fn(u64) -> std::result::Result<Stream, CodecError>;

fn decompress(mut input: &[u8], decoder: Decoder, memlimit: u64) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut first = true;
    loop {
        let start = output.len();
        let mut stream = decoder(memlimit).map_err(codec_error)?;
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
    let message = match err {
        CodecError::Data => "Corrupt input data",
        CodecError::Format => "Input format not supported by decoder",
        CodecError::Options => "Invalid or unsupported options",
        CodecError::MemLimit => "Memory usage limit exceeded",
        CodecError::Mem => return Error::new("MemoryError", ""),
        CodecError::UnsupportedCheck => "Unsupported integrity check",
        CodecError::NoCheck | CodecError::Program => "Internal error",
    };
    Error::new(LZMA_ERROR, message)
}
