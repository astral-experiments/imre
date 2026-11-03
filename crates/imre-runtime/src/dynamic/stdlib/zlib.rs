//! DEFLATE streams for the boxed zlib and gzip modules.
use super::{Context, Error, Id, Result, Value, builtins, os};
use std::{ffi::CStr, mem::MaybeUninit};
use zlib_rs::{DeflateConfig, DeflateFlush, InflateConfig, InflateFlush, ReturnCode};
use zlib_rs::{c_api::z_stream, deflate, inflate as backend_inflate};

mod inflate;
pub(in crate::dynamic) use inflate::{InflateReader, inflate_object};

fn buffer(ctx: &Context, data: Id) -> Result<&[u8]> {
    if let Value::Array(array) = ctx.heap.get(ctx.scalar_value(data)) {
        Ok(array.as_bytes())
    } else {
        ctx.bytes_like(data)
    }
}

pub(super) fn call(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if args.is_empty() && kwargs.len() <= 3 {
        return Err(Error::new(
            "TypeError",
            format!("{method}() takes at least 1 positional argument (0 given)"),
        ));
    }
    let compress = method == "compress";
    let names = if compress {
        ["", "level", "wbits"]
    } else {
        ["", "wbits", "bufsize"]
    };
    let [data, first, second] = builtins::bind_method_arguments(method, args, kwargs, names, 3)?;
    let data = data.expect("required positional argument");
    ctx.check_buffer_exporter(data)?;
    buffer(ctx, data)?;
    let output = ctx.with_buffer_export(data, |ctx| {
        let number = first.map(|id| os::c_int(ctx, id)).transpose()?;
        let (codec, size) = if compress {
            let level = number.unwrap_or(-1);
            let wbits = second
                .map(|id| os::c_int(ctx, id))
                .transpose()?
                .unwrap_or(15);
            (Codec::Compress { level, wbits }, 32 * 1024)
        } else {
            let size = match second {
                Some(id) => crate::signed_index(&ctx.index_integer(id)?).ok_or_else(|| {
                    Error::new(
                        "OverflowError",
                        "Python int too large to convert to C ssize_t",
                    )
                })?,
                None => 16 * 1024,
            };
            let size = usize::try_from(size)
                .map_err(|_| Error::new("ValueError", "bufsize must be non-negative"))?;
            (
                Codec::Decompress {
                    wbits: number.unwrap_or(15),
                },
                size.max(1),
            )
        };
        codec.run(buffer(ctx, data)?, size)
    })?;
    Ok(ctx.alloc(Value::Bytes(output)))
}

/// The gzip facade writes the framing and owns its file object. These primitives
/// advance one raw DEFLATE stream and return just its newly produced bytes.
pub(super) fn gzip_writer(ctx: &mut Context, name: &str, args: &[Id]) -> Result<Id> {
    if name == "writer" {
        let level = os::c_int(ctx, args[0])?;
        if !(-1..=9).contains(&level) {
            return Err(Error::new("ValueError", "Invalid initialization option"));
        }
        let stream = Stream::new(Codec::Compress { level, wbits: -15 })?;
        let writer = crate::Shared::try_new(std::cell::RefCell::new(Some(stream)))
            .map_err(|_| crate::Error::allocation())?;
        return Ok(ctx.alloc(Value::GzipWriter(writer)));
    }
    let Value::GzipWriter(writer) = ctx.heap.get(args[0]) else {
        return Err(Error::new("TypeError", "expected a gzip writer"));
    };
    let writer = writer.clone();
    let (data, flush) = if name == "compress" {
        ctx.check_buffer_exporter(args[1])?;
        (buffer(ctx, args[1])?, DeflateFlush::NoFlush)
    } else {
        let mode = os::c_int(ctx, args[1])?;
        let flush = DeflateFlush::try_from(mode).map_err(|_| {
            Error::new(
                "zlib.error",
                "Error -2 while flushing: inconsistent stream state",
            )
        })?;
        if flush == DeflateFlush::NoFlush {
            return Ok(ctx.alloc(Value::Bytes(Vec::new())));
        }
        (&[][..], flush)
    };
    let mut writer = writer.borrow_mut();
    let stream = writer.as_mut().ok_or_else(|| {
        let action = if name == "compress" {
            "compressing data"
        } else {
            "flushing"
        };
        Error::new(
            "zlib.error",
            format!("Error -2 while {action}: inconsistent stream state"),
        )
    })?;
    let bytes = stream.process(data, Vec::new(), flush)?;
    if flush == DeflateFlush::Finish {
        *writer = None;
    }
    Ok(ctx.alloc(Value::Bytes(bytes)))
}

#[derive(Clone, Copy)]
enum Codec {
    Compress { level: i32, wbits: i32 },
    Decompress { wbits: i32 },
}

impl Codec {
    fn run(self, input: &[u8], initial_size: usize) -> Result<Vec<u8>> {
        // Argument Clinic allocates the initial output before initializing the codec.
        let mut output: Vec<u8> = Vec::new();
        output
            .try_reserve_exact(initial_size)
            .map_err(|_| Error::new("MemoryError", "Unable to allocate output buffer."))?;
        Stream::new(self)?.process(input, output, DeflateFlush::Finish)
    }
}

pub(in crate::dynamic) struct Stream {
    codec: Codec,
    raw: z_stream,
}

impl Stream {
    fn process(
        &mut self,
        input: &[u8],
        mut output: Vec<u8>,
        flush: DeflateFlush,
    ) -> Result<Vec<u8>> {
        let mut input_pos = 0;
        loop {
            if output.len() == output.capacity() {
                output
                    .try_reserve(32 * 1024)
                    .map_err(|_| Error::new("MemoryError", "Unable to allocate output buffer."))?;
            }
            let (status, consumed, produced, output_full) =
                self.step(&input[input_pos..], output.spare_capacity_mut(), flush);
            input_pos += consumed;
            // SAFETY: the stream initialized produced bytes of the supplied spare capacity.
            unsafe { output.set_len(output.len() + produced) };
            match status {
                ReturnCode::StreamEnd => return Ok(output),
                ReturnCode::Ok | ReturnCode::BufError
                    if (consumed != 0 || produced != 0)
                        && (output_full || input_pos < input.len()) =>
                {
                    continue;
                }
                ReturnCode::Ok | ReturnCode::BufError
                    if flush != DeflateFlush::Finish && input_pos == input.len() =>
                {
                    crate::compact_bytes(&mut output);
                    return Ok(output);
                }
                ReturnCode::Ok | ReturnCode::BufError => {
                    return Err(self.error(ReturnCode::BufError));
                }
                other => return Err(self.error(other)),
            }
        }
    }

    fn new(codec: Codec) -> Result<Self> {
        let mut raw = z_stream::default();
        let status = match codec {
            Codec::Compress { level, wbits } => deflate::init(
                &mut raw,
                DeflateConfig {
                    level,
                    window_bits: wbits,
                    ..DeflateConfig::default()
                },
            ),
            Codec::Decompress { wbits } => {
                backend_inflate::init(&mut raw, InflateConfig { window_bits: wbits })
            }
        };
        match status {
            ReturnCode::Ok => Ok(Self { codec, raw }),
            ReturnCode::MemError => Err(mem_error(codec)),
            _ => Err(Error::new(
                "zlib.error",
                match codec {
                    Codec::Compress { .. } => "Bad compression level",
                    Codec::Decompress { .. } => {
                        "Error -2 while preparing to decompress data: inconsistent stream state"
                    }
                },
            )),
        }
    }

    fn step(
        &mut self,
        input: &[u8],
        output: &mut [MaybeUninit<u8>],
        flush: DeflateFlush,
    ) -> (ReturnCode, usize, usize, bool) {
        let count = input.len().min(u32::MAX as usize) as u32;
        let space = output.len().min(u32::MAX as usize) as u32;
        self.raw.next_in = input.as_ptr().cast_mut();
        self.raw.avail_in = count;
        self.raw.next_out = output.as_mut_ptr().cast();
        self.raw.avail_out = space;
        let finish = input.len() == count as usize;
        // SAFETY: new initialized the stream for its codec. input and output cover avail_in
        // and avail_out respectively, are disjoint, and remain live throughout the call.
        let status = unsafe {
            match self.codec {
                Codec::Compress { .. } => deflate::deflate(
                    deflate::DeflateStream::from_stream_mut(&mut self.raw).unwrap(),
                    if finish { flush } else { DeflateFlush::NoFlush },
                ),
                Codec::Decompress { .. } => backend_inflate::inflate(
                    backend_inflate::InflateStream::from_stream_mut(&mut self.raw).unwrap(),
                    match flush {
                        DeflateFlush::Finish if finish => InflateFlush::Finish,
                        DeflateFlush::SyncFlush => InflateFlush::SyncFlush,
                        _ => InflateFlush::NoFlush,
                    },
                ),
            }
        };
        (
            status,
            (count - self.raw.avail_in) as usize,
            (space - self.raw.avail_out) as usize,
            self.raw.avail_out == 0,
        )
    }

    fn error(&self, status: ReturnCode) -> Error {
        if status == ReturnCode::MemError {
            return mem_error(self.codec);
        }
        let verb = match self.codec {
            Codec::Compress { .. } => "compressing",
            Codec::Decompress { .. } => "decompressing",
        };
        let detail = if self.raw.msg.is_null() {
            match status {
                ReturnCode::BufError => "incomplete or truncated stream",
                ReturnCode::DataError => "invalid input data",
                ReturnCode::StreamError => "inconsistent stream state",
                _ => "",
            }
        } else {
            // SAFETY: the backend's msg is a static NUL-terminated diagnostic string.
            unsafe { CStr::from_ptr(self.raw.msg) }.to_str().unwrap()
        };
        let separator = if detail.is_empty() { "" } else { ": " };
        Error::new(
            "zlib.error",
            format!(
                "Error {} while {verb} data{separator}{detail}",
                status as i32
            ),
        )
    }
}

fn mem_error(codec: Codec) -> Error {
    Error::new(
        "MemoryError",
        match codec {
            Codec::Compress { .. } => "Out of memory while compressing data",
            Codec::Decompress { .. } => "Out of memory while decompressing data",
        },
    )
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: only a successfully initialized stream reaches Drop. end frees the codec
        // state once and does not access the input or output regions used by the last step.
        unsafe {
            match self.codec {
                Codec::Compress { .. } => {
                    let _ = deflate::end(
                        deflate::DeflateStream::from_stream_mut(&mut self.raw).unwrap(),
                    );
                }
                Codec::Decompress { .. } => {
                    backend_inflate::end(
                        backend_inflate::InflateStream::from_stream_mut(&mut self.raw).unwrap(),
                    );
                }
            }
        }
    }
}
