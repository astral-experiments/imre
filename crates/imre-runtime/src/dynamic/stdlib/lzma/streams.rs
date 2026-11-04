//! Stateful encoders and single-member decoders for Python's LZMA stream objects.
use super::{
    Context, Error, Id, LZMA_ERROR, NONE, Result, Value, builtins, codec_error, filters, os,
};
use liblzma::stream::{
    Action, Check, Error as CodecError, Filters, LzmaOptions, PRESET_DEFAULT, Status, Stream,
    TELL_ANY_CHECK, TELL_NO_CHECK,
};
use num_traits::{Signed, ToPrimitive};
use std::cell::RefCell;

const CHUNK: usize = 32 * 1024;
const CHECK_UNKNOWN: u8 = 16;

pub(in crate::dynamic) struct Compressor(Option<Stream>);
pub(in crate::dynamic) struct Decompressor {
    stream: Stream,
    check: u8,
    xz_check_byte: u8,
    eof: bool,
    needs_input: bool,
    // Unconsumed input. After EOF, the same tail is exposed as unused_data.
    pending: Vec<u8>,
    offset: usize,
}

pub(super) enum DecoderOptions {
    Auto(u64),
    Xz(u64),
    Alone(u64),
    Raw(Filters),
}

impl DecoderOptions {
    pub(super) fn new(ctx: &mut Context, format: i32, limit: Option<Id>, spec: Id) -> Result<Self> {
        let memlimit = if let Some(id) = limit.filter(|&id| id != NONE) {
            if format == 3 {
                return Err(Error::new(
                    "ValueError",
                    "Cannot specify memory limit with FORMAT_RAW",
                ));
            }
            let value = ctx.index_integer(id)?;
            let value = value.big();
            if value.is_negative() {
                return Err(Error::new("ValueError", "Cannot convert negative int"));
            }
            value
                .to_u64()
                .ok_or_else(|| Error::new("OverflowError", "Python int too large for C uint64_t"))?
        } else {
            u64::MAX
        };
        if format != 3 && spec != NONE {
            return Err(Error::new(
                "ValueError",
                "Cannot specify filters except with FORMAT_RAW",
            ));
        }
        match format {
            0 => Ok(Self::Auto(memlimit)),
            1 => Ok(Self::Xz(memlimit)),
            2 => Ok(Self::Alone(memlimit)),
            3 if spec != NONE => Ok(Self::Raw(filters::decoder_chain(ctx, spec)?)),
            3 => Err(Error::new(
                "ValueError",
                "Must specify filters for FORMAT_RAW",
            )),
            _ => Err(invalid_format(format)),
        }
    }
    pub(super) fn stream(&self, flags: u32) -> Result<Stream> {
        match self {
            Self::Auto(limit) => Stream::new_auto_decoder(*limit, flags),
            Self::Xz(limit) => Stream::new_stream_decoder(*limit, flags),
            Self::Alone(limit) => Stream::new_lzma_decoder(*limit),
            Self::Raw(filters) => Stream::new_raw_decoder(filters),
        }
        .map_err(codec_error)
    }
}

fn invalid_format(format: i32) -> Error {
    Error::new("ValueError", format!("Invalid container format: {format}"))
}

fn encoder(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Stream> {
    let [format, check, preset, spec] = builtins::bind_method_arguments(
        "LZMACompressor",
        args,
        kwargs,
        ["format", "check", "preset", "filters"],
        4,
    )?;
    let format = format
        .map(|id| os::c_int(ctx, id))
        .transpose()?
        .unwrap_or(1);
    let check = check
        .map(|id| os::c_int(ctx, id))
        .transpose()?
        .unwrap_or(-1);
    if format != 1 && !matches!(check, 0 | -1) {
        return Err(Error::new(
            "ValueError",
            "Integrity checks are only supported by FORMAT_XZ",
        ));
    }
    let spec = spec.unwrap_or(NONE);
    let preset = match preset.filter(|&id| id != NONE) {
        None => PRESET_DEFAULT,
        Some(_) if spec != NONE => {
            return Err(Error::new(
                "ValueError",
                "Cannot specify both preset and filter chain",
            ));
        }
        Some(id) => {
            let value = ctx.index_integer(id)?;
            let value = value.big();
            if value.is_negative() {
                return Err(Error::new("ValueError", "Cannot convert negative int"));
            }
            value
                .to_u32()
                .ok_or_else(|| Error::new("OverflowError", "Python int too large for C uint32_t"))?
        }
    };
    match format {
        1 => {
            let check = match check {
                0 => Check::None,
                1 => Check::Crc32,
                -1 | 4 => Check::Crc64,
                10 => Check::Sha256,
                0..=15 => return Err(codec_error(CodecError::UnsupportedCheck)),
                _ => return Err(codec_error(CodecError::Program)),
            };
            if spec == NONE {
                Stream::new_easy_encoder(preset, check)
            } else {
                Stream::new_stream_encoder(&filters::chain(ctx, spec)?, check)
            }
        }
        2 => {
            let options = if spec == NONE {
                LzmaOptions::new_preset(preset).map_err(|_| {
                    Error::new(LZMA_ERROR, format!("Invalid compression preset: {preset}"))
                })?
            } else {
                filters::alone(ctx, spec)?
            };
            Stream::new_lzma_encoder(&options)
        }
        3 if spec != NONE => Stream::new_raw_encoder(&filters::chain(ctx, spec)?),
        3 => {
            return Err(Error::new(
                "ValueError",
                "Must specify filters for FORMAT_RAW",
            ));
        }
        _ => return Err(invalid_format(format)),
    }
    .map_err(codec_error)
}

// Acquire the data before converting any later argument via Python's __index__.
// While the export is held, callbacks can modify its bytes but cannot resize it.
pub(super) fn with_input<T>(
    ctx: &mut Context,
    data: Id,
    consume: impl FnOnce(&mut Context, Id) -> Result<T>,
) -> Result<T> {
    if let Some(method) = ctx.python_buffer_method(data, true)? {
        ctx.with_contiguous_python_buffer(data, method, 0, consume)
    } else {
        ctx.native_buffer_bytes(data)?;
        ctx.with_buffer_export(data, |ctx| consume(ctx, data))
    }
}

fn compress(stream: &mut Stream, mut input: &[u8], action: Action) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        if output.len() == output.capacity() {
            output
                .try_reserve(CHUNK)
                .map_err(|_| Error::new("MemoryError", ""))?;
        }
        let start = stream.total_in();
        let status = stream
            .process_vec(input, &mut output, action)
            .map_err(codec_error)?;
        input = &input[(stream.total_in() - start) as usize..];
        if status == Status::StreamEnd || (matches!(action, Action::Run) && input.is_empty()) {
            return Ok(output);
        }
        if status == Status::MemNeeded {
            return Err(Error::new(LZMA_ERROR, "Insufficient buffer space"));
        }
    }
}

pub(in crate::dynamic) fn compressor(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "new" {
        let stream = encoder(ctx, args, kwargs)?;
        let handle = crate::Shared::try_new(RefCell::new(Compressor(Some(stream))))
            .map_err(|_| crate::Error::allocation())?;
        return Ok(ctx.alloc(Value::LzmaCompressor(handle)));
    }
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method LZMACompressor.{name}() needs an argument"),
        ));
    };
    let Value::LzmaCompressor(handle) = ctx.heap.get(receiver) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for '_lzma.LZMACompressor' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    };
    let handle = handle.clone();
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("LZMACompressor.{name}() takes no keyword arguments"),
        ));
    }
    let output = match name {
        "compress" => {
            let [data] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "LZMACompressor.compress() takes exactly one argument ({} given)",
                        args.len()
                    ),
                ));
            };
            with_input(ctx, *data, |ctx, view| {
                let input = ctx.native_buffer_bytes(view)?;
                let mut writer = handle.borrow_mut();
                let stream = writer
                    .0
                    .as_mut()
                    .ok_or_else(|| Error::new("ValueError", "Compressor has been flushed"))?;
                compress(stream, input, Action::Run)
            })?
        }
        "flush" => {
            if !args.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "LZMACompressor.flush() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            let mut stream = handle
                .borrow_mut()
                .0
                .take()
                .ok_or_else(|| Error::new("ValueError", "Repeated call to flush()"))?;
            compress(&mut stream, &[], Action::Finish)?
        }
        _ => return Err(Error::new("AttributeError", name)),
    };
    Ok(ctx.alloc(Value::Bytes(output)))
}

impl Decompressor {
    fn decode(&mut self, data: &[u8], limit: usize) -> Result<Vec<u8>> {
        if self.eof {
            return Err(Error::new("EOFError", "Already at end of stream"));
        }
        let mut pending = std::mem::take(&mut self.pending);
        let offset = std::mem::take(&mut self.offset);
        let buffered = offset < pending.len();
        let input = if buffered {
            // Advance an offset while draining output one byte at a time. Compact
            // only when a caller supplies more input as well.
            if !data.is_empty() {
                pending.drain(..offset);
                pending
                    .try_reserve(data.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                pending.extend_from_slice(data);
                &pending[..]
            } else {
                &pending[offset..]
            }
        } else {
            data
        };
        let start = self.stream.total_in();
        let output = self.decode_slice(input, limit)?;
        let consumed = (self.stream.total_in() - start) as usize;
        let tail = &input[consumed..];
        self.needs_input = !self.eof && tail.is_empty() && output.len() < limit;
        if !tail.is_empty() {
            if buffered {
                self.offset = pending.len() - tail.len();
                self.pending = pending;
            } else {
                self.pending
                    .try_reserve_exact(tail.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                self.pending.extend_from_slice(tail);
            }
        }
        Ok(output)
    }

    fn decode_slice(&mut self, mut input: &[u8], limit: usize) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        loop {
            let len = output.len();
            let space = CHUNK.min(limit - len);
            output
                .try_reserve(space)
                .map_err(|_| Error::new("MemoryError", ""))?;
            output.resize(len + space, 0);
            let start = self.stream.total_in();
            let before = self.stream.total_out();
            let status = self.stream.process(input, &mut output[len..], Action::Run);
            let consumed = (self.stream.total_in() - start) as usize;
            let written = (self.stream.total_out() - before) as usize;
            output.truncate(len + written);
            // liblzma reports when the integrity check is known. Its safe wrapper
            // does not expose lzma_get_check. Byte 7 of a validated XZ header
            // holds the Check ID and can arrive in a separate call.
            if start <= 7 && self.stream.total_in() > 7 {
                self.xz_check_byte = input[(7 - start) as usize];
            }
            input = &input[consumed..];
            match status {
                Ok(Status::StreamEnd) => {
                    self.eof = true;
                    break;
                }
                Ok(Status::GetCheck) => self.check = self.xz_check_byte,
                Err(CodecError::NoCheck) => self.check = 0,
                Err(err) => return Err(codec_error(err)),
                Ok(Status::MemNeeded) if !input.is_empty() || written == space => {
                    return Err(Error::new(LZMA_ERROR, "Insufficient buffer space"));
                }
                Ok(_) => {}
            }
            if output.len() == limit || (input.is_empty() && written < space) {
                break;
            }
        }
        Ok(output)
    }
}

pub(in crate::dynamic) fn decompressor(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "new" {
        let [format, memlimit, spec] = builtins::bind_method_arguments(
            "LZMADecompressor",
            args,
            kwargs,
            ["format", "memlimit", "filters"],
            3,
        )?;
        let format = format
            .map(|id| os::c_int(ctx, id))
            .transpose()?
            .unwrap_or(0);
        let options = DecoderOptions::new(ctx, format, memlimit, spec.unwrap_or(NONE))?;
        let stream = options.stream(TELL_ANY_CHECK | TELL_NO_CHECK)?;
        let state = Decompressor {
            stream,
            check: if format < 2 { CHECK_UNKNOWN } else { 0 },
            xz_check_byte: 0,
            eof: false,
            needs_input: true,
            pending: Vec::new(),
            offset: 0,
        };
        let handle =
            crate::Shared::try_new(RefCell::new(state)).map_err(|_| crate::Error::allocation())?;
        return Ok(ctx.alloc(Value::LzmaDecompressor(handle)));
    }
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method LZMADecompressor.{name}() needs an argument"),
        ));
    };
    let Value::LzmaDecompressor(handle) = ctx.heap.get(receiver) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for '_lzma.LZMADecompressor' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    };
    let handle = handle.clone();
    let [data, limit] =
        builtins::bind_builtin_arguments(name, args, kwargs, ["data", "max_length"], 1)?;
    let data = data.expect("data is required");
    let output = with_input(ctx, data, |ctx, view| {
        let limit = limit
            .map(|id| os::ssize_t(ctx, id))
            .transpose()?
            .unwrap_or(-1);
        let input = ctx.native_buffer_bytes(view)?;
        handle
            .borrow_mut()
            .decode(input, usize::try_from(limit).unwrap_or(usize::MAX))
    })?;
    Ok(ctx.alloc(Value::Bytes(output)))
}

pub(in crate::dynamic) fn decompressor_attr(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
) -> Result<Id> {
    let Value::LzmaDecompressor(handle) = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected an LZMA decompressor"));
    };
    let handle = handle.clone();
    let state = handle.borrow();
    match name {
        "check" => Ok(ctx.int(i64::from(state.check))),
        "eof" => Ok(builtins::boolean(state.eof)),
        "needs_input" => Ok(builtins::boolean(state.needs_input)),
        "unused_data" => {
            let bytes = if state.eof {
                state.pending[state.offset..].to_vec()
            } else {
                Vec::new()
            };
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        _ => Err(Error::new("AttributeError", name)),
    }
}
