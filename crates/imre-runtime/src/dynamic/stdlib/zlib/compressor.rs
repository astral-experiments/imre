//! Incremental DEFLATE compression for zlib.compressobj and the gzip writer.
use super::{
    Codec, Context, DeflateConfig, DeflateFlush, Error, Id, Result, ReturnCode, Stream, Value,
    buffer, builtins, deflate, os, stream_error, z_stream,
};
use std::{cell::RefCell, mem::MaybeUninit};

pub(in crate::dynamic) enum Compressor {
    Active(Stream),
    // deflateEnd releases the codec state but leaves the last backend diagnostic available.
    Finished(&'static str),
}

impl Compressor {
    fn state_error(&self, action: &str) -> Error {
        let detail = match self {
            Self::Active(stream) => stream.error_detail(ReturnCode::StreamError),
            Self::Finished(detail) => detail,
        };
        stream_error(ReturnCode::StreamError, action, detail)
    }
}

fn allocate(ctx: &mut Context, stream: Stream) -> Result<Id> {
    let handle = crate::Shared::try_new(RefCell::new(Compressor::Active(stream)))
        .map_err(|_| crate::Error::allocation())?;
    Ok(ctx.alloc(Value::ZlibCompressor(handle)))
}

fn create(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [level, method, wbits, memory, strategy, dictionary] = builtins::bind_method_arguments(
        "compressobj",
        args,
        kwargs,
        ["level", "method", "wbits", "memLevel", "strategy", "zdict"],
        6,
    )?;
    let defaults = DeflateConfig::default();
    let mut integer = |arg: Option<Id>, default: i32| -> Result<i32> {
        Ok(arg
            .map(|id| os::c_int(ctx, id))
            .transpose()?
            .unwrap_or(default))
    };
    let level = integer(level, defaults.level)?;
    let method = integer(method, defaults.method as i32)?;
    let window_bits = integer(wbits, defaults.window_bits)?;
    let mem_level = integer(memory, defaults.mem_level)?;
    let strategy = integer(strategy, defaults.strategy as i32)?;
    let dictionary = dictionary
        .map(|id| {
            ctx.check_buffer_exporter(id)?;
            buffer(ctx, id)
        })
        .transpose()?;
    if dictionary.is_some_and(|data| data.len() > u32::MAX as usize) {
        return Err(Error::new(
            "OverflowError",
            "zdict length does not fit in an unsigned int",
        ));
    }
    let invalid = || Error::new("ValueError", "Invalid initialization option");
    let config = DeflateConfig {
        level,
        window_bits,
        mem_level,
        method: method.try_into().map_err(|_| invalid())?,
        strategy: strategy.try_into().map_err(|_| invalid())?,
    };
    let mut stream = Stream::new(Codec::Compress { config }).map_err(|e| {
        if e.kind == "MemoryError" {
            Error::new(
                "MemoryError",
                "Can't allocate memory for compression object",
            )
        } else {
            invalid()
        }
    })?;
    if let Some(dictionary) = dictionary {
        // SAFETY: new initialized the stream with a deflate configuration.
        let raw = unsafe { deflate::DeflateStream::from_stream_mut(&mut stream.raw).unwrap() };
        if deflate::set_dictionary(raw, dictionary) != ReturnCode::Ok {
            return Err(Error::new("ValueError", "Invalid dictionary"));
        }
    }
    allocate(ctx, stream)
}

fn copy_stream(stream: &mut Stream) -> Result<Stream> {
    let mut raw = MaybeUninit::<z_stream>::uninit();
    // SAFETY: DeflateStream and z_stream have identical layouts. On success deflateCopy
    // initializes independent compressor state that will be released by Stream::drop.
    let status = unsafe {
        deflate::copy(
            &mut *raw
                .as_mut_ptr()
                .cast::<MaybeUninit<deflate::DeflateStream>>(),
            deflate::DeflateStream::from_stream_mut(&mut stream.raw).unwrap(),
        )
    };
    match status {
        ReturnCode::Ok => Ok(Stream {
            codec: stream.codec,
            raw: unsafe { raw.assume_init() },
        }),
        ReturnCode::MemError => Err(Error::new(
            "MemoryError",
            "Can't allocate memory for compression object",
        )),
        _ => Err(Error::new("ValueError", "Inconsistent stream state")),
    }
}

pub(in crate::dynamic) fn compressor(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "new" {
        return create(ctx, args, kwargs);
    }
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new("TypeError", "expected a zlib compressor"));
    };
    let Value::ZlibCompressor(writer) = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected a zlib compressor"));
    };
    let writer = writer.clone();
    if matches!(name, "copy" | "__copy__" | "__deepcopy__") {
        if name == "__deepcopy__" {
            positional(name, args, kwargs)?;
        } else if !args.is_empty() || !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no arguments"),
            ));
        }
        let mut writer = writer.borrow_mut();
        let Compressor::Active(stream) = &mut *writer else {
            return Err(Error::new("ValueError", "Inconsistent stream state"));
        };
        return allocate(ctx, copy_stream(stream)?);
    }
    let (data, flush, action) = if name == "compress" {
        let data = positional(name, args, kwargs)?;
        ctx.check_buffer_exporter(data)?;
        (
            buffer(ctx, data)?,
            DeflateFlush::NoFlush,
            "compressing data",
        )
    } else {
        let [mode] = builtins::bind_method_arguments(name, args, kwargs, [""], 1)?;
        let mode = mode
            .map(|id| os::c_int(ctx, id))
            .transpose()?
            .unwrap_or(DeflateFlush::Finish as i32);
        let flush =
            DeflateFlush::try_from(mode).map_err(|_| writer.borrow().state_error("flushing"))?;
        if flush == DeflateFlush::NoFlush {
            return Ok(ctx.alloc(Value::Bytes(Vec::new())));
        }
        (&[][..], flush, "flushing")
    };
    let mut writer = writer.borrow_mut();
    let Compressor::Active(stream) = &mut *writer else {
        return Err(writer.state_error(action));
    };
    let bytes = stream.process(data, Vec::new(), flush)?;
    if flush == DeflateFlush::Finish {
        *writer = Compressor::Finished(stream.error_detail(ReturnCode::StreamError));
    }
    Ok(ctx.alloc(Value::Bytes(bytes)))
}

fn positional(name: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if args.is_empty() && kwargs.len() <= 1 {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes exactly 1 positional argument (0 given)"),
        ));
    }
    let [value] = builtins::bind_method_arguments(name, args, kwargs, [""], 1)?;
    Ok(value.expect("required positional argument"))
}
