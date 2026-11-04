//! DEFLATE streams for the boxed zlib and gzip modules.
use super::{Context, Error, Id, Result, Value, builtins, os};
use std::{ffi::CStr, mem::MaybeUninit};
use zlib_rs::{DeflateConfig, DeflateFlush, InflateConfig, InflateFlush, ReturnCode};
use zlib_rs::{c_api::z_stream, deflate, inflate};

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

pub(in crate::dynamic) struct Decompressor {
    stream: Option<Stream>,
    eof: bool,
    unused_data: Vec<u8>,
    unconsumed_tail: Vec<u8>,
}

fn copy_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(data.len())
        .map_err(|_| crate::Error::allocation())?;
    copy.extend_from_slice(data);
    Ok(copy)
}

fn size_arg(ctx: &mut Context, value: Option<Id>, default: isize) -> Result<isize> {
    let Some(value) = value else {
        return Ok(default);
    };
    crate::signed_index(&ctx.index_integer(value)?).ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

pub(in crate::dynamic) fn decompressor(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "new" {
        let [wbits, dictionary] =
            builtins::bind_method_arguments("decompressobj", args, kwargs, ["wbits", "zdict"], 2)?;
        let wbits = wbits.map(|n| os::c_int(ctx, n)).transpose()?.unwrap_or(15);
        if let Some(id) = dictionary {
            ctx.check_buffer_exporter(id)?;
            if !matches!(
                ctx.heap.get(ctx.scalar_value(id)),
                Value::Bytes(_)
                    | Value::ByteArray(_)
                    | Value::Array(_)
                    | Value::MemoryView { .. }
                    | Value::BytesIoBuffer { .. }
            ) {
                return Err(Error::new(
                    "TypeError",
                    "zdict argument must support the buffer protocol",
                ));
            }
        }
        let mut stream = Stream::new(Codec::Decompress { wbits }).map_err(|e| {
            if e.kind == "zlib.error" {
                Error::new("ValueError", "Invalid initialization option")
            } else {
                e
            }
        })?;
        if wbits < 0
            && let Some(id) = dictionary
        {
            stream.set_dictionary(buffer(ctx, id)?)?;
        }
        let state = Decompressor {
            stream: Some(stream),
            eof: false,
            unused_data: Vec::new(),
            unconsumed_tail: Vec::new(),
        };
        return state.allocate(ctx, dictionary);
    }

    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method Decompress.{name}() needs an argument"),
        ));
    };
    let Value::ZlibDecompressor { state, dictionary } = ctx.heap.get(receiver) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for 'zlib.Decompress' objects doesn't apply to a '{}' object",
                ctx.builtin_type_name(receiver)
                    .unwrap_or_else(|| ctx.type_name(receiver))
            ),
        ));
    };
    let (state, dictionary) = (state.clone(), *dictionary);
    let bytes = match name {
        "copy" | "__copy__" => {
            if !args.is_empty() || !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!("{name}() takes no arguments"),
                ));
            }
            let copy = state.borrow_mut().snapshot()?;
            return copy.allocate(ctx, dictionary);
        }
        "eof" => return Ok(builtins::boolean(state.borrow().eof)),
        "unused_data" => copy_bytes(&state.borrow().unused_data)?,
        "unconsumed_tail" => copy_bytes(&state.borrow().unconsumed_tail)?,
        "decompress" => {
            if args.is_empty() && kwargs.len() <= 2 {
                return Err(Error::new(
                    "TypeError",
                    "decompress() takes at least 1 positional argument (0 given)",
                ));
            }
            let [data, limit] =
                builtins::bind_method_arguments(name, args, kwargs, ["", "max_length"], 2)?;
            let data = data.expect("required positional argument");
            ctx.check_buffer_exporter(data)?;
            buffer(ctx, data)?;
            ctx.with_buffer_export(data, |ctx| {
                let limit = usize::try_from(size_arg(ctx, limit, 0)?)
                    .map_err(|_| Error::new("ValueError", "max_length must be non-negative"))?;
                let limit = if limit == 0 { usize::MAX } else { limit };
                state.borrow_mut().decode(
                    Some(buffer(ctx, data)?),
                    limit.min(16 * 1024),
                    limit,
                    || dictionary.map(|id| buffer(ctx, id)).transpose(),
                )
            })?
        }
        "flush" => {
            let [size] = builtins::bind_method_arguments(name, args, kwargs, [""], 1)?;
            let size = size_arg(ctx, size, 16 * 1024)?;
            if size <= 0 {
                return Err(Error::new("ValueError", "length must be greater than zero"));
            }
            state
                .borrow_mut()
                .decode(None, size as usize, usize::MAX, || Ok(None))?
        }
        _ => unreachable!("registered decompressor operation"),
    };
    Ok(ctx.alloc(Value::Bytes(bytes)))
}

impl Decompressor {
    fn allocate(self, ctx: &mut Context, dictionary: Option<Id>) -> Result<Id> {
        let state = crate::Shared::try_new(std::cell::RefCell::new(self))
            .map_err(|_| crate::Error::allocation())?;
        Ok(ctx.alloc(Value::ZlibDecompressor { state, dictionary }))
    }

    fn snapshot(&mut self) -> Result<Self> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| Error::new("ValueError", "Inconsistent stream state"))?;
        let mut dest = MaybeUninit::uninit();
        // inflate::copy requires a non-null output pointer even with zero available bytes.
        let mut scratch = 0;
        stream.raw.next_out = &raw mut scratch;
        // SAFETY: the source owns an initialized inflater, and dest is separate storage.
        let status = unsafe {
            inflate::copy(
                &mut dest,
                inflate::InflateStream::from_stream_ref(&stream.raw).unwrap(),
            )
        };
        stream.raw.next_out = std::ptr::null_mut();
        match status {
            ReturnCode::Ok => {}
            ReturnCode::MemError => {
                return Err(Error::new(
                    "MemoryError",
                    "Can't allocate memory for decompression object",
                ));
            }
            _ => return Err(Error::new("ValueError", "Inconsistent stream state")),
        }
        // SAFETY: a successful copy owns an independent state and window. zlib-rs
        // guarantees InflateStream has the layout of z_stream.
        let mut raw = unsafe { dest.as_ptr().cast::<z_stream>().read() };
        raw.next_out = std::ptr::null_mut();
        Ok(Self {
            stream: Some(Stream {
                codec: stream.codec,
                raw,
            }),
            eof: self.eof,
            unused_data: copy_bytes(&self.unused_data)?,
            unconsumed_tail: copy_bytes(&self.unconsumed_tail)?,
        })
    }

    /// With no new input, flush drains the tail retained by the previous call.
    fn decode<'a>(
        &mut self,
        data: Option<&[u8]>,
        initial_size: usize,
        limit: usize,
        dictionary: impl Fn() -> Result<Option<&'a [u8]>>,
    ) -> Result<Vec<u8>> {
        let flushing = data.is_none();
        let input = data.unwrap_or(&self.unconsumed_tail);
        let mut output: Vec<u8> = Vec::new();
        output
            .try_reserve_exact(initial_size)
            .map_err(|_| crate::Error::allocation())?;
        let Some(stream) = &mut self.stream else {
            if flushing {
                return Ok(Vec::new());
            }
            self.unconsumed_tail = copy_bytes(input)?;
            return Err(Error::new(
                "zlib.error",
                "Error -2 while decompressing data: inconsistent stream state",
            ));
        };
        let flush = if flushing {
            DeflateFlush::Finish
        } else {
            DeflateFlush::SyncFlush
        };
        let mut pos = 0;
        let status = loop {
            let len = output.len();
            if len == output.capacity() {
                output
                    .try_reserve((limit - len).min(32 * 1024))
                    .map_err(|_| crate::Error::allocation())?;
            }
            let spare = output.spare_capacity_mut();
            let n = spare.len().min(limit - len);
            let (status, consumed, produced, full) =
                stream.step(&input[pos..], &mut spare[..n], flush);
            pos += consumed;
            // SAFETY: step initialized exactly produced bytes in output's spare capacity.
            unsafe { output.set_len(len + produced) };
            if status == ReturnCode::NeedDict
                && let Some(dict) = dictionary()?
            {
                stream.set_dictionary(dict)?;
                continue;
            }
            if !matches!(status, ReturnCode::Ok | ReturnCode::BufError)
                || output.len() == limit
                || (produced == 0 && consumed == 0)
                || (!full && pos == input.len())
            {
                break status;
            }
        };
        let tail = &input[pos..];
        if status == ReturnCode::StreamEnd {
            self.unused_data
                .try_reserve(tail.len())
                .map_err(|_| crate::Error::allocation())?;
            self.unused_data.extend_from_slice(tail);
        }
        if status != ReturnCode::StreamEnd || !self.unconsumed_tail.is_empty() {
            self.unconsumed_tail = copy_bytes(tail)?;
        }
        if status == ReturnCode::StreamEnd {
            self.eof = true;
            if flushing {
                self.stream = None;
            }
        } else if !flushing && !matches!(status, ReturnCode::Ok | ReturnCode::BufError) {
            return Err(stream.error(status));
        }
        crate::compact_bytes(&mut output);
        Ok(output)
    }
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
                inflate::init(&mut raw, InflateConfig { window_bits: wbits })
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

    fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<()> {
        if dictionary.len() > u32::MAX as usize {
            return Err(Error::new(
                "OverflowError",
                "zdict length does not fit in an unsigned int",
            ));
        }
        // SAFETY: only initialized decompressors accept a dictionary.
        let stream = unsafe { inflate::InflateStream::from_stream_mut(&mut self.raw).unwrap() };
        let status = inflate::set_dictionary(stream, dictionary);
        if status != ReturnCode::Ok {
            return Err(self.error_action(status, "setting zdict"));
        }
        Ok(())
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
                Codec::Decompress { .. } => inflate::inflate(
                    inflate::InflateStream::from_stream_mut(&mut self.raw).unwrap(),
                    if !finish {
                        InflateFlush::NoFlush
                    } else if flush == DeflateFlush::Finish {
                        InflateFlush::Finish
                    } else {
                        InflateFlush::SyncFlush
                    },
                ),
            }
        };
        let result = (
            status,
            (count - self.raw.avail_in) as usize,
            (space - self.raw.avail_out) as usize,
            self.raw.avail_out == 0,
        );
        self.raw.next_in = std::ptr::null();
        self.raw.next_out = std::ptr::null_mut();
        self.raw.avail_in = 0;
        self.raw.avail_out = 0;
        result
    }

    fn error(&self, status: ReturnCode) -> Error {
        let verb = match self.codec {
            Codec::Compress { .. } => "compressing data",
            Codec::Decompress { .. } => "decompressing data",
        };
        self.error_action(status, verb)
    }

    fn error_action(&self, status: ReturnCode, action: &str) -> Error {
        if status == ReturnCode::MemError {
            return mem_error(self.codec);
        }
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
            format!("Error {} while {action}{separator}{detail}", status as i32),
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
                    inflate::end(inflate::InflateStream::from_stream_mut(&mut self.raw).unwrap());
                }
            }
        }
    }
}
