//! Stateful zlib decompression for the boxed API.
use super::{
    Codec, Context, DeflateFlush, Error, Id, MaybeUninit, Result, ReturnCode, Stream, Value,
    backend_inflate, buffer, builtins, os, z_stream,
};
use std::cell::RefCell;

pub(in crate::dynamic) struct InflateReader {
    stream: Option<Stream>,
    zdict: Option<Id>,
    unused_data: Id,
    unconsumed_tail: Id,
    eof: bool,
}

impl InflateReader {
    pub(in crate::dynamic) fn references(&self) -> impl Iterator<Item = Id> {
        [self.unused_data, self.unconsumed_tail]
            .into_iter()
            .chain(self.zdict)
    }

    fn allocate(self, ctx: &mut Context) -> Result<Id> {
        let handle =
            crate::Shared::try_new(RefCell::new(self)).map_err(|_| crate::Error::allocation())?;
        Ok(ctx.alloc(Value::ZlibReader(handle)))
    }

    fn new(ctx: &mut Context, wbits: i32, zdict: Option<Id>) -> Result<Self> {
        if let Some(dict) = zdict {
            ctx.check_buffer_exporter(dict)?;
            // Wrapped streams acquire the dictionary only when the decoder asks
            // for it, so mutations made after construction must remain visible.
            if !matches!(
                ctx.heap.get(ctx.scalar_value(dict)),
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
        let mut stream = Stream::new(Codec::Decompress { wbits }).map_err(|error| {
            if error.kind == "MemoryError" {
                Error::new(
                    "MemoryError",
                    "Can't allocate memory for decompression object",
                )
            } else {
                Error::new("ValueError", "Invalid initialization option")
            }
        })?;
        // inflateCopy requires a non-null next_out even on a fresh stream.
        // step installs a real output slice before any bytes can be written.
        stream.raw.next_out = std::ptr::dangling_mut();
        if let Some(dict) = zdict
            && wbits < 0
        {
            set_dictionary(&mut stream, buffer(ctx, dict)?)?;
        }
        let empty = ctx.alloc(Value::Bytes(Vec::new()));
        Ok(Self {
            stream: Some(stream),
            zdict,
            unused_data: empty,
            unconsumed_tail: empty,
            eof: false,
        })
    }

    fn copy(&self) -> Result<Self> {
        let stream = self
            .stream
            .as_ref()
            .ok_or_else(|| Error::new("ValueError", "Inconsistent stream state"))?;
        let mut raw = MaybeUninit::<z_stream>::uninit();
        // SAFETY: zlib-rs documents InflateStream and z_stream as layout-compatible.
        // The source owns initialized inflate state. On success copy initializes the
        // destination with independently allocated state and a separate window.
        let status = unsafe {
            backend_inflate::copy(
                &mut *raw
                    .as_mut_ptr()
                    .cast::<MaybeUninit<backend_inflate::InflateStream>>(),
                backend_inflate::InflateStream::from_stream_ref(&stream.raw).unwrap(),
            )
        };
        match status {
            ReturnCode::Ok => Ok(Self {
                // SAFETY: copy succeeded and transferred the new state's ownership here.
                stream: Some(Stream {
                    codec: stream.codec,
                    raw: unsafe { raw.assume_init() },
                }),
                zdict: self.zdict,
                unused_data: self.unused_data,
                unconsumed_tail: self.unconsumed_tail,
                eof: self.eof,
            }),
            ReturnCode::MemError => Err(Error::new(
                "MemoryError",
                "Can't allocate memory for decompression object",
            )),
            _ => Err(Error::new("ValueError", "Inconsistent stream state")),
        }
    }

    fn inflate(
        &mut self,
        ctx: &Context,
        input: &[u8],
        limit: usize,
        initial_size: usize,
        flush: bool,
    ) -> Result<(Vec<u8>, usize, ReturnCode)> {
        let mut output = Vec::new();
        output
            .try_reserve_exact(initial_size.min(limit))
            .map_err(|_| crate::Error::allocation())?;
        let mut consumed = 0;
        let Some(stream) = self.stream.as_mut() else {
            return Ok((output, consumed, ReturnCode::StreamError));
        };
        let mode = if flush {
            DeflateFlush::Finish
        } else {
            DeflateFlush::SyncFlush
        };
        let status = loop {
            if output.len() == output.capacity() {
                output
                    .try_reserve((32 * 1024).min(limit - output.len()))
                    .map_err(|_| crate::Error::allocation())?;
            }
            let available = limit - output.len();
            let spare = output.spare_capacity_mut();
            let size = spare.len().min(available);
            let (status, read, written, full) =
                stream.step(&input[consumed..], &mut spare[..size], mode);
            // Independent copies must never inherit transient input or output pointers.
            stream.raw.next_in = std::ptr::null();
            stream.raw.avail_in = 0;
            stream.raw.next_out = std::ptr::dangling_mut();
            stream.raw.avail_out = 0;
            consumed += read;
            // SAFETY: step initialized exactly written bytes of spare capacity.
            unsafe { output.set_len(output.len() + written) };
            if let Some(dict) = self.zdict
                && status == ReturnCode::NeedDict
                && !flush
            {
                set_dictionary(stream, buffer(ctx, dict)?)?;
                continue;
            }
            if !matches!(status, ReturnCode::Ok | ReturnCode::BufError)
                || output.len() == limit
                || (read == 0 && written == 0)
                || (!full && consumed == input.len())
            {
                break status;
            }
        };
        crate::compact_bytes(&mut output);
        Ok((output, consumed, status))
    }
}

fn decode(
    ctx: &mut Context,
    handle: &RefCell<InflateReader>,
    data: Id,
    limit: usize,
    initial_size: usize,
    flush: bool,
) -> Result<Id> {
    let input = buffer(ctx, data)?;
    let (output, consumed, status) =
        handle
            .borrow_mut()
            .inflate(ctx, input, limit, initial_size, flush)?;
    let rest = &input[consumed..];
    let (unused, tail) = {
        let reader = handle.borrow();
        let old_tail = !buffer(ctx, reader.unconsumed_tail)?.is_empty();
        let unused = if status == ReturnCode::StreamEnd && !rest.is_empty() {
            Some(join_bytes(buffer(ctx, reader.unused_data)?, rest)?)
        } else {
            None
        };
        let tail = if old_tail || (!rest.is_empty() && status != ReturnCode::StreamEnd) {
            Some(join_bytes(&[], rest)?)
        } else {
            None
        };
        (unused, tail)
    };
    if let Some(unused) = unused {
        let id = ctx.alloc(Value::Bytes(unused));
        handle.borrow_mut().unused_data = id;
    }
    if let Some(tail) = tail {
        let id = ctx.alloc(Value::Bytes(tail));
        handle.borrow_mut().unconsumed_tail = id;
    }
    if status == ReturnCode::StreamEnd {
        let mut reader = handle.borrow_mut();
        reader.eof = true;
        if flush {
            reader.stream = None;
        }
    } else if !flush && !matches!(status, ReturnCode::Ok | ReturnCode::BufError) {
        return Err(handle.borrow().stream.as_ref().map_or_else(
            || {
                Error::new(
                    "zlib.error",
                    "Error -2 while decompressing data: inconsistent stream state",
                )
            },
            |stream| stream.error(status),
        ));
    }
    Ok(ctx.alloc(Value::Bytes(output)))
}

fn join_bytes(previous: &[u8], rest: &[u8]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(
            previous
                .len()
                .checked_add(rest.len())
                .ok_or_else(crate::Error::allocation)?,
        )
        .map_err(|_| crate::Error::allocation())?;
    bytes.extend_from_slice(previous);
    bytes.extend_from_slice(rest);
    Ok(bytes)
}

fn set_dictionary(stream: &mut Stream, dictionary: &[u8]) -> Result<()> {
    u32::try_from(dictionary.len()).map_err(|_| {
        Error::new(
            "OverflowError",
            "zdict length does not fit in an unsigned int",
        )
    })?;
    // SAFETY: stream was initialized by inflate::init and is exclusively borrowed.
    let raw = unsafe { backend_inflate::InflateStream::from_stream_mut(&mut stream.raw).unwrap() };
    match backend_inflate::set_dictionary(raw, dictionary) {
        ReturnCode::Ok => Ok(()),
        ReturnCode::DataError => Err(Error::new(
            "zlib.error",
            "Error -3 while setting zdict: invalid input data",
        )),
        code => Err(stream.error(code)),
    }
}

fn ssize(ctx: &mut Context, id: Id) -> Result<isize> {
    crate::signed_index(&ctx.index_integer(id)?).ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

pub(in crate::dynamic) fn inflate_object(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if method == "decompressobj" {
        let [wbits, dict] =
            builtins::bind_builtin_arguments(method, args, kwargs, ["wbits", "zdict"], 0)?;
        let wbits = wbits
            .map(|id| os::c_int(ctx, id))
            .transpose()?
            .unwrap_or(15);
        return InflateReader::new(ctx, wbits, dict)?.allocate(ctx);
    }
    let (receiver, args) = builtins::method_receiver_args(
        ctx,
        crate::dynamic::value::ZLIB_READER_TYPE,
        method,
        args,
        &[],
    )?;
    let Value::ZlibReader(handle) = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected a zlib decompressor"));
    };
    let handle = handle.clone();
    match method {
        "decompress" => {
            if args.is_empty() && kwargs.len() <= 2 {
                return Err(Error::new(
                    "TypeError",
                    "decompress() takes at least 1 positional argument (0 given)",
                ));
            }
            let [data, max_length] =
                builtins::bind_method_arguments(method, args, kwargs, ["", "max_length"], 2)?;
            let data = data.expect("checked positional argument");
            ctx.check_buffer_exporter(data)?;
            buffer(ctx, data)?;
            ctx.with_buffer_export(data, |ctx| {
                let limit = max_length
                    .map(|id| ssize(ctx, id))
                    .transpose()?
                    .unwrap_or(0);
                let limit = usize::try_from(limit)
                    .map_err(|_| Error::new("ValueError", "max_length must be non-negative"))?;
                decode(
                    ctx,
                    &handle,
                    data,
                    if limit == 0 { usize::MAX } else { limit },
                    32 * 1024,
                    false,
                )
            })
        }
        "flush" => {
            let [length] = builtins::bind_method_arguments(method, args, kwargs, [""], 1)?;
            let length = length
                .map(|id| ssize(ctx, id))
                .transpose()?
                .unwrap_or(16 * 1024);
            if length <= 0 {
                return Err(Error::new("ValueError", "length must be greater than zero"));
            }
            let tail = handle.borrow().unconsumed_tail;
            decode(ctx, &handle, tail, usize::MAX, length as usize, true)
        }
        "copy" | "__copy__" | "__deepcopy__" => {
            if method == "__deepcopy__" {
                if args.is_empty() && kwargs.len() <= 1 {
                    return Err(Error::new(
                        "TypeError",
                        "__deepcopy__() takes exactly 1 positional argument (0 given)",
                    ));
                }
                builtins::bind_method_arguments(method, args, kwargs, [""], 1)?;
            } else if !args.is_empty() || !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!("{method}() takes no arguments"),
                ));
            }
            let copy = handle.borrow().copy()?;
            copy.allocate(ctx)
        }
        "eof" => Ok(builtins::boolean(handle.borrow().eof)),
        "unused_data" => Ok(handle.borrow().unused_data),
        "unconsumed_tail" => Ok(handle.borrow().unconsumed_tail),
        _ => unreachable!("unknown decompressor operation"),
    }
}
