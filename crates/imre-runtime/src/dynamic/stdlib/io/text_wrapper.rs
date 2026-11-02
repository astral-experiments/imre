//! Text encoding and buffering over the original Python binary stream.

use num_traits::ToPrimitive;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::super::codecs;
use super::{Context, Error, Id, MemoryStream, NONE, Result, Value};
use crate::dynamic::builtins::boolean;
use crate::objects::{Codec, DecodeIssue, FixedUnicode};

mod reader;
mod reconfigure;
use reader::Reader;

const CHUNK: usize = 8192;
const NATIVE_ORDER: i32 = if cfg!(target_endian = "little") {
    -1
} else {
    1
};
pub(super) const METHODS: &[&str] = &[
    "__init__",
    "__next__",
    "_CHUNK_SIZE",
    "buffer",
    "encoding",
    "errors",
    "line_buffering",
    "write_through",
    "newlines",
    "closed",
    "name",
    "readable",
    "writable",
    "seekable",
    "isatty",
    "fileno",
    "flush",
    "close",
    "detach",
    "reconfigure",
    "write",
    "read",
    "readline",
    "seek",
    "tell",
];

pub(super) fn property(name: &str) -> bool {
    matches!(
        name,
        "_CHUNK_SIZE"
            | "buffer"
            | "encoding"
            | "errors"
            | "line_buffering"
            | "write_through"
            | "newlines"
            | "closed"
            | "name"
    )
}

#[derive(Clone, Copy)]
pub(in crate::dynamic::stdlib) enum StreamCodec {
    Registered(Codec),
    Wide { format: FixedUnicode, bom: bool },
}

impl StreamCodec {
    fn lookup(encoding: &str) -> Result<Self> {
        let key = crate::objects::normalize_codec(encoding)?;
        Ok(match FixedUnicode::encoding(&key) {
            Some((format, bom)) => Self::Wide { format, bom },
            None => Self::Registered(Codec::by_key(&key, encoding)?),
        })
    }

    fn detects_bom(self) -> bool {
        matches!(self, Self::Wide { bom: true, .. })
    }
}

#[derive(Clone, Copy)]
struct Coder {
    codec: StreamCodec,
    errors: Id,
}

/// The heap traces the buffer until detach returns it. No borrow spans a Python stream call.
pub(in crate::dynamic) struct Wrapper {
    pub(super) buffer: Cell<Option<Id>>,
    encoding: Cell<Id>,
    errors: Cell<Id>,
    newline: Cell<Option<&'static str>>,
    decoder_newline: Cell<Option<&'static str>>,
    decoder: Cell<Coder>,
    encoder: Cell<Coder>,
    line_buffering: Cell<bool>,
    write_through: Cell<bool>,
    chunk_size: Cell<usize>,
    readable: Cell<bool>,
    writable: Cell<bool>,
    seekable: bool,
    telling: Cell<bool>,
    read1: bool,
    // CPython's fast encoders track the stream start on the wrapper. UTF-8-SIG
    // tracks its first write on each new incremental encoder, even across reentry.
    write_start: Cell<bool>,
    utf8_signature_pending: Cell<bool>,
    output: RefCell<Vec<u8>>,
    input: RefCell<Reader>,
}

pub(super) fn invoke(ctx: &mut Context, obj: Id, name: &str, args: Vec<Id>) -> Result<Id> {
    let mark = ctx.heap.temporary_mark();
    ctx.heap.keep(obj);
    let result = ctx
        .lookup_attr(obj, name)
        .and_then(|f| ctx.call(f, args, Vec::new()));
    ctx.heap.release_temporaries_since(mark);
    match &result {
        Ok(value) => ctx.heap.keep(*value),
        Err(error) => {
            for value in error.roots() {
                ctx.heap.keep(value);
            }
        }
    }
    ctx.heap.weak_edge_removed(obj);
    result
}

fn flag(ctx: &mut Context, obj: Id, method: &str) -> Result<bool> {
    let value = invoke(ctx, obj, method, Vec::new())?;
    ctx.truth(value)
}

fn validate_newline(value: &str) -> Result<&'static str> {
    match value {
        "" => Ok(""),
        "\n" => Ok("\n"),
        "\r" => Ok("\r"),
        "\r\n" => Ok("\r\n"),
        _ => Err(Error::new(
            "ValueError",
            format!("illegal newline value: {value}"),
        )),
    }
}

pub(in crate::dynamic::stdlib) fn text_options(
    ctx: &mut Context,
    encoding: Id,
    errors: Id,
    newline: Id,
) -> Result<(String, Option<&'static str>, StreamCodec)> {
    // Argument Clinic converts encoding and newline before __init__ checks errors.
    for (id, name) in [
        (encoding, "encoding"),
        (newline, "newline"),
        (errors, "errors"),
    ] {
        if id != NONE {
            super::file_io::check_string_arg(ctx, id, "TextIOWrapper", name, "str or None")?;
        }
    }
    let string = |id| (id != NONE).then(|| ctx.str_value(id)).transpose();
    let encoding = crate::sys::file_encoding(string(encoding)?).into_owned();
    let newline = string(newline)?.map(validate_newline).transpose()?;
    let codec = StreamCodec::lookup(&encoding)?;
    Ok((encoding, newline, codec))
}

fn initialize(ctx: &mut Context, receiver: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    const KEYS: [&str; 6] = [
        "buffer",
        "encoding",
        "errors",
        "newline",
        "line_buffering",
        "write_through",
    ];
    let mut options = [NONE; 6];
    if args.len() + kwargs.len() > KEYS.len() {
        return Err(Error::new(
            "TypeError",
            format!(
                "TextIOWrapper() takes at most 6 arguments ({} given)",
                args.len() + kwargs.len()
            ),
        ));
    }
    options[..args.len()].copy_from_slice(args);
    for (name, id) in kwargs {
        let i = KEYS.iter().position(|&k| k == name).ok_or_else(|| {
            Error::new(
                "TypeError",
                format!("TextIOWrapper() got an unexpected keyword argument '{name}'"),
            )
        })?;
        if i < args.len() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "argument for TextIOWrapper() given by name ('{name}') and position ({})",
                    i + 1
                ),
            ));
        }
        options[i] = *id;
    }
    if args.is_empty() && !kwargs.iter().any(|(k, _)| k == "buffer") {
        return Err(Error::new(
            "TypeError",
            "TextIOWrapper() missing required argument 'buffer' (pos 1)",
        ));
    }
    let [
        buffer,
        encoding,
        errors,
        newline,
        line_buffering,
        write_through,
    ] = options;
    let (encoding, newline, codec) = text_options(ctx, encoding, errors, newline)?;
    let line_buffering = ctx.truth(line_buffering)?;
    let write_through = ctx.truth(write_through)?;
    let readable = flag(ctx, buffer, "readable")?;
    let writable = flag(ctx, buffer, "writable")?;
    let seekable = flag(ctx, buffer, "seekable")?;
    let read1 = match ctx.lookup_attr(buffer, "read1") {
        Ok(_) => true,
        Err(e) if e.kind == "AttributeError" => false,
        Err(e) => return Err(e),
    };
    let write_start = if writable && seekable {
        let position = invoke(ctx, buffer, "tell", vec![])?;
        !ctx.truth(position)?
    } else {
        true
    };
    let errors = if errors == NONE {
        ctx.text("strict")
    } else {
        errors
    };
    let coder = Coder {
        codec,
        errors: ctx.scalar_value(errors),
    };
    let state = Wrapper {
        buffer: Cell::new(Some(buffer)),
        decoder: Cell::new(coder),
        encoder: Cell::new(coder),
        encoding: Cell::new(ctx.text(encoding)),
        errors: Cell::new(errors),
        newline: Cell::new(newline),
        decoder_newline: Cell::new(newline),
        line_buffering: Cell::new(line_buffering),
        write_through: Cell::new(write_through),
        chunk_size: Cell::new(CHUNK),
        readable: Cell::new(readable),
        writable: Cell::new(writable),
        seekable,
        telling: Cell::new(seekable),
        read1,
        write_start: Cell::new(write_start),
        utf8_signature_pending: Cell::new(write_start),
        output: RefCell::default(),
        input: RefCell::default(),
    };
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        return Err(Error::new("TypeError", "expected a TextIOWrapper instance"));
    };
    if let Some(MemoryStream::Wrapper(old)) =
        memory_stream.replace(MemoryStream::Wrapper(Rc::new(state)))
        && let Some(buffer) = old.buffer.get()
    {
        ctx.heap.weak_edge_removed(buffer);
    }
    Ok(NONE)
}

impl Wrapper {
    /// Get the current stream state after a Python callback may have reinitialized it.
    fn from_receiver(ctx: &Context, receiver: Id) -> Result<Rc<Self>> {
        let Value::Instance {
            memory_stream: Some(MemoryStream::Wrapper(w)),
            ..
        } = ctx.heap.get(receiver)
        else {
            return Err(Error::new(
                "ValueError",
                "I/O operation on uninitialized object",
            ));
        };
        Ok(w.clone())
    }

    /// Resolve the current buffer before calling a method that can reenter the wrapper.
    fn invoke_buffer(ctx: &mut Context, receiver: Id, name: &str, args: Vec<Id>) -> Result<Id> {
        let buffer = Self::from_receiver(ctx, receiver)?.binary()?;
        invoke(ctx, buffer, name, args)
    }

    pub(in crate::dynamic) fn children(&self, visit: &mut crate::dynamic::value::MarkQueue<'_>) {
        visit.extend(self.buffer.get());
        visit.extend([
            self.encoding.get(),
            self.errors.get(),
            self.encoder.get().errors,
            self.decoder.get().errors,
        ]);
    }

    fn binary(&self) -> Result<Id> {
        self.buffer
            .get()
            .ok_or_else(|| Error::new("ValueError", "underlying buffer has been detached"))
    }

    fn check_open(&self, ctx: &mut Context, buffer: Id) -> Result<()> {
        let closed = ctx.lookup_attr(buffer, "closed")?;
        if ctx.truth(closed)? {
            Err(Error::new("ValueError", "I/O operation on closed file."))
        } else {
            Ok(())
        }
    }

    fn drain(&self, ctx: &mut Context, receiver: Id) -> Result<()> {
        let pending = self.output.take();
        if !pending.is_empty() {
            let data = ctx.alloc(Value::Bytes(pending));
            Self::invoke_buffer(ctx, receiver, "write", vec![data])?;
        }
        Ok(())
    }

    fn position(&self, ctx: &mut Context, end: Id) -> Result<Id> {
        let input = self.input.borrow();
        let width = match self.decoder.get().codec {
            StreamCodec::Wide { format, .. } => format.width(),
            StreamCodec::Registered(_) => 1,
        };
        let offset = input.position(ctx, end, width)?;
        if !self.decoder.get().codec.detects_bom() || !input.wide_snapshot {
            return Ok(offset);
        }
        let flag = match input.order {
            0 if input.beginning => 2u128,
            order if order == -NATIVE_ORDER => 1,
            _ => return Ok(offset),
        };
        // TextIOWrapper cookies store the newline decoder's flags above the raw offset.
        let flag = flag << usize::from(matches!(self.newline.get(), None | Some("")));
        let cookie = (flag << 64) | u128::from(ctx.as_integer(offset)? as u64);
        Ok(ctx.alloc(Value::Int(crate::Int::new(cookie.into()))))
    }

    fn write(&self, ctx: &mut Context, receiver: Id, value: Id) -> Result<Id> {
        if !self.writable.get() {
            return Err(Error::new("UnsupportedOperation", "not writable"));
        }
        let replacement = match self.newline.get() {
            None if cfg!(windows) => Some("\r\n"),
            Some("\r") => Some("\r"),
            Some("\r\n") => Some("\r\n"),
            _ => None,
        };
        let storage = ctx.string_storage(value).ok_or_else(|| {
            Error::new(
                "TypeError",
                format!("write() argument must be str, not {}", ctx.type_name(value)),
            )
        })?;
        let (count, has_lf, has_cr) = match ctx.heap.get(storage) {
            Value::Str(text) => (
                text.chars().count(),
                text.contains('\n'),
                text.contains('\r'),
            ),
            Value::SurrogateStr(points) => {
                (points.len(), points.contains(&10), points.contains(&13))
            }
            _ => unreachable!(),
        };
        let write_through = self.write_through.get();
        let flush = self.line_buffering.get() && (has_lf || has_cr);
        let translated = if let Some(nl) = replacement
            && has_lf
        {
            let points = ctx.string_codepoints(storage)?;
            let extra = if nl.len() == 2 {
                points.iter().filter(|&&c| c == 10).count()
            } else {
                0
            };
            let mut result = Vec::new();
            result
                .try_reserve_exact(
                    count
                        .checked_add(extra)
                        .ok_or_else(crate::Error::allocation)?,
                )
                .map_err(|_| crate::Error::allocation())?;
            for &point in points.iter() {
                if point == 10 {
                    result.extend(nl.bytes().map(u32::from))
                } else {
                    result.push(point)
                }
            }
            ctx.text_codepoints(result)
        } else {
            value
        };
        let encoder = self.encoder.get();
        let encoded = match encoder.codec {
            StreamCodec::Wide { format, bom } => {
                let errors = ctx.str_value(encoder.errors)?.to_owned();
                let signature = bom && self.seekable && self.write_start.get();
                codecs::encode_fixed(ctx, translated, format, &errors, signature)
            }
            StreamCodec::Registered(codec) => {
                let encoding = if matches!(codec, Codec::Utf8 { signature: true })
                    && !self.utf8_signature_pending.replace(false)
                {
                    ctx.text("utf-8")
                } else {
                    ctx.text(codec.name())
                };
                codecs::encode_text(ctx, translated, Some(encoding), Some(encoder.errors))
            }
        };

        // A fast encoder clears the wrapper's start flag even when the error
        // handler raises or installs a different encoder.
        if !matches!(
            encoder.codec,
            StreamCodec::Registered(Codec::Utf8 { signature: true } | Codec::CodePage(_))
        ) {
            self.write_start.set(false);
        }
        let encoded = encoded?;
        if encoded.len() >= self.chunk_size.get() {
            while !self.output.borrow().is_empty() {
                self.drain(ctx, receiver)?;
            }
        }
        let length = {
            let mut pending = self.output.borrow_mut();
            pending
                .try_reserve(encoded.len())
                .map_err(|_| crate::Error::allocation())?;
            pending.extend_from_slice(&encoded);
            pending.len()
        };
        if flush || write_through || length >= self.chunk_size.get() {
            self.drain(ctx, receiver)?;
        }
        if flush {
            Self::invoke_buffer(ctx, receiver, "flush", vec![])?;
        }
        self.input.borrow_mut().after_write();
        Ok(ctx.int(count as i64))
    }

    fn read(ctx: &mut Context, receiver: Id, limit: Option<usize>, line: bool) -> Result<Id> {
        let w = Self::from_receiver(ctx, receiver)?;
        w.binary()?;
        if !w.readable.get() {
            return Err(Error::new("UnsupportedOperation", "not readable"));
        }
        w.drain(ctx, receiver)?;
        let all = !line && limit.is_none();
        loop {
            let w = Self::from_receiver(ctx, receiver)?;
            let buffer = w.binary()?;
            if !all {
                let mut input = w.input.borrow_mut();
                if (!line || input.decoded_chunk_nonempty == Some(true))
                    && let Some(part) = input.take(limit, line, w.newline.get(), false)?
                {
                    return Ok(ctx.text(part));
                }
                if line && input.decoded_chunk_nonempty == Some(true) {
                    // A consumed line chunk is discarded. An empty decoder result
                    // stays installed if the next refill raises.
                    input.decoded_chunk_nonempty = None;
                }
            }
            let (method, args) = if all {
                ("read", vec![])
            } else {
                (
                    if w.read1 { "read1" } else { "read" },
                    vec![ctx.int(w.chunk_size.get() as i64)],
                )
            };
            let data = invoke(ctx, buffer, method, args)?;
            let w = Self::from_receiver(ctx, receiver)?;
            w.binary()?;
            let bytes = ctx.bytes_like(data).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "underlying {method}() should have returned a bytes-like object, not '{}'",
                        ctx.type_name(data)
                    ),
                )
            })?;
            let end = all || bytes.is_empty();
            let decoder = w.decoder.get();
            let (codec, raw) = w.input.borrow_mut().chunk(bytes, decoder.codec, end)?;
            let raw = ctx.alloc(Value::Bytes(raw));
            // The ordinary native codec entry owns error handling. A registered handler
            // may run Python, so release the reader borrow before invoking it.
            let errors = ctx.scalar_value(decoder.errors);
            let (decoded, consumed, signature, resolved) = match codec {
                StreamCodec::Registered(c) => (
                    codecs::decode_bytes(ctx, c, raw, Some(errors))?,
                    ctx.bytes_like(raw)?.len(),
                    0,
                    codec,
                ),
                StreamCodec::Wide { format, bom } => {
                    let errors = ctx.str_value(errors)?.to_owned();
                    let requested = if bom { 0 } else { format.byte_order() };
                    let (decoded, consumed, order) =
                        codecs::decode_wide_chunk(ctx, raw, &errors, format, requested, end)?;
                    let data = ctx.bytes_like(raw)?;
                    let width = format.width();
                    if bom && order == 0 && consumed >= width {
                        let err = DecodeIssue {
                            encoding: format.codec_name(true),
                            start: 0,
                            end: width,
                            reason: "Stream does not start with BOM",
                        }
                        .error(data);
                        return Err(ctx.decode_error(err));
                    }
                    let consumed = consumed.min(data.len());
                    let mut reader = w.input.borrow_mut();
                    reader.save_tail(&data[consumed..])?;
                    let signature = if bom && order != 0 { width } else { 0 };
                    reader.order = order;
                    if order != 0 {
                        reader.beginning = false;
                    }
                    if !all && w.telling.get() {
                        reader.wide_snapshot = true;
                    }
                    let resolved = StreamCodec::Wide {
                        format: format.with_order(order),
                        bom: false,
                    };
                    (decoded, consumed, signature, resolved)
                }
            };
            let mut input = w.input.borrow_mut();
            input.append(
                ctx.str_value(decoded)?,
                &ctx.bytes_like(raw)?[signature.min(consumed)..consumed],
                resolved,
                w.decoder_newline.get(),
                end,
                !all && w.telling.get(),
            )?;
            if !all {
                let nonempty = input.final_chunk_has_text();
                input.decoded_chunk_nonempty = (!line || !end || nonempty).then_some(nonempty);
                if line && end {
                    if nonempty {
                        // The decoder can still emit a pending CR or an error replacement at EOF.
                        continue;
                    }
                    input.clear_snapshot();
                }
            }
            if end {
                let part = input
                    .take(limit, line, w.newline.get(), true)?
                    .unwrap_or_default();
                if all {
                    input.after_read_all();
                }
                return Ok(ctx.text(part));
            }
        }
    }
}

/// Advance the concrete text iterator. Only an empty line restores telling at EOF.
pub(super) fn next(
    ctx: &mut Context,
    receiver: Id,
    exact: bool,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "wrapper __next__() takes no keyword arguments",
        ));
    }
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("expected 0 arguments, got {}", args.len()),
        ));
    }
    let w = Wrapper::from_receiver(ctx, receiver)?;
    let buffer = w.binary()?;
    w.telling.set(false);
    let line = if exact {
        w.check_open(ctx, buffer)?;
        Wrapper::read(ctx, receiver, None, true)?
    } else {
        invoke(ctx, receiver, "readline", vec![])?
    };
    let text = ctx.string_storage(line).ok_or_else(|| {
        Error::new(
            "OSError",
            format!(
                "readline() should have returned a str object, not '{}'",
                ctx.type_name(line)
            ),
        )
    })?;
    if ctx.truth(text)? {
        return Ok(line);
    }
    let w = Wrapper::from_receiver(ctx, receiver)?;
    w.input.borrow_mut().clear_snapshot();
    w.telling.set(w.seekable);
    Err(Error::new("StopIteration", ""))
}

/// Dispatch to the text layer while keeping the supplied buffer and its overrides observable.
pub(super) fn call(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "__init__" {
        return initialize(ctx, receiver, args, kwargs);
    }
    if name == "reconfigure" {
        return reconfigure::call(ctx, receiver, args, kwargs);
    }
    let w = Wrapper::from_receiver(ctx, receiver)?;
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("TextIOWrapper.{name}() takes no keyword arguments"),
        ));
    }
    if name == "set_chunk_size" {
        w.binary()?;
        let Some(&value) = args.first() else {
            return Err(Error::new("AttributeError", "cannot delete attribute"));
        };
        let size = ctx.index_integer(value)?.to_i64().ok_or_else(|| {
            Error::new(
                "ValueError",
                format!(
                    "cannot fit '{}' into an index-sized integer",
                    ctx.type_name(value)
                ),
            )
        })?;
        if size <= 0 {
            return Err(Error::new(
                "ValueError",
                "a strictly positive integer is required",
            ));
        }
        w.chunk_size.set(size as usize);
        return Ok(NONE);
    }
    super::argument_count("TextIOWrapper", name, args.len())?;
    match name {
        "buffer" => return Ok(w.buffer.get().unwrap_or(NONE)),
        "encoding" => return Ok(w.encoding.get()),
        "errors" => return Ok(w.errors.get()),
        "line_buffering" => return Ok(boolean(w.line_buffering.get())),
        "write_through" => return Ok(boolean(w.write_through.get())),
        "__exit__" => return invoke(ctx, receiver, "close", vec![]),
        _ => {}
    }
    let buffer = w.binary()?;
    match name {
        "_CHUNK_SIZE" => return Ok(ctx.int(w.chunk_size.get() as i64)),
        "closed" | "name" => return ctx.lookup_attr(buffer, name),
        "fileno" => return invoke(ctx, buffer, name, vec![]),
        "newlines" => return Ok(w.input.borrow().newlines(ctx)),
        "close" => {
            let closed = ctx.lookup_attr(buffer, "closed")?;
            if ctx.truth(closed)? || w.buffer.get().is_none() {
                return Ok(NONE);
            }
            let flushed = invoke(ctx, receiver, "flush", vec![]);
            if let Err(error) = &flushed {
                for id in error.roots() {
                    ctx.heap.keep(id);
                }
            }
            let closed = Wrapper::invoke_buffer(ctx, receiver, "close", vec![]);
            return match (flushed, closed) {
                (Err(flush), Err(mut close)) => {
                    close.context = ctx.raised_instance(flush);
                    Err(close)
                }
                (Err(flush), _) => Err(flush),
                (_, closed) => closed,
            };
        }
        "detach" => {
            invoke(ctx, receiver, "flush", vec![])?;
            let w = Wrapper::from_receiver(ctx, receiver)?;
            let buffer = w.binary()?;
            w.buffer.set(None);
            return Ok(buffer);
        }
        _ => {}
    }
    w.check_open(ctx, buffer)?;
    let w = Wrapper::from_receiver(ctx, receiver)?;
    let buffer = w.binary()?;
    match name {
        "__enter__" => Ok(receiver),
        "flush" => {
            w.telling.set(w.seekable);
            w.drain(ctx, receiver)?;
            Wrapper::invoke_buffer(ctx, receiver, "flush", vec![])
        }
        "readable" | "writable" | "seekable" | "isatty" => invoke(ctx, buffer, name, vec![]),
        "write" => w.write(ctx, receiver, args[0]),
        "read" | "readline" => {
            let n = if name == "read" {
                super::super::files::read_length(ctx, args.first().copied())?
            } else if let Some(&size) = args.first() {
                ctx.index_integer(size)?.to_i64().ok_or_else(|| {
                    Error::new(
                        "OverflowError",
                        "Python int too large to convert to C ssize_t",
                    )
                })?
            } else {
                -1
            };
            Wrapper::read(ctx, receiver, usize::try_from(n).ok(), name == "readline")
        }
        "seek" | "tell" => {
            if !w.seekable {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "underlying stream is not seekable",
                ));
            }
            if name == "tell" {
                if !w.telling.get() {
                    return Err(Error::new(
                        "OSError",
                        "telling position disabled by next() call",
                    ));
                }
                invoke(ctx, receiver, "flush", vec![])?;
                let pos = Wrapper::invoke_buffer(ctx, receiver, "tell", vec![])?;
                return Wrapper::from_receiver(ctx, receiver)?.position(ctx, pos);
            }
            let mut cookie = args[0];
            let seek = ctx.index_integer(cookie)?;
            let whence = match args.get(1) {
                Some(&id) => ctx.as_integer(id)?,
                None => 0,
            };
            let (offset, whence) = match whence {
                0 if seek.big().sign() == num_bigint::Sign::Minus => {
                    return Err(Error::new(
                        "ValueError",
                        format!("negative seek position {}", seek.big()),
                    ));
                }
                0 => (seek, 0),
                2 if seek.to_i64() == Some(0) => (seek, 2),
                1 if seek.to_i64() == Some(0) => {
                    cookie = invoke(ctx, receiver, "tell", vec![])?;
                    (ctx.index_integer(cookie)?, 0)
                }
                1 | 2 => {
                    return Err(Error::new(
                        "UnsupportedOperation",
                        if whence == 1 {
                            "can't do nonzero cur-relative seeks"
                        } else {
                            "can't do nonzero end-relative seeks"
                        },
                    ));
                }
                _ => {
                    return Err(Error::new(
                        "ValueError",
                        format!("invalid whence ({whence}, should be 0, 1 or 2)"),
                    ));
                }
            };
            invoke(ctx, receiver, "flush", vec![])?;
            if whence == 2 {
                Wrapper::from_receiver(ctx, receiver)?
                    .input
                    .borrow_mut()
                    .discard();
            }
            let wide_cookie = if w.decoder.get().codec.detects_bom() {
                offset.big().to_u128().filter(|n| n >> 64 > 0)
            } else {
                None
            };
            if let Some(bits) = wide_cookie {
                let shift = usize::from(matches!(w.newline.get(), None | Some("")));
                let flags = bits >> 64;
                if flags != 1 << shift && flags != 2 << shift {
                    return Err(Error::new(
                        "NotImplementedError",
                        "native text position needs a decoder snapshot",
                    ));
                }
            }
            let byte_offset = match wide_cookie {
                Some(bits) => i64::try_from(bits as u64).ok(),
                None => offset.to_i64(),
            }
            .ok_or_else(|| Error::new("OverflowError", "seek position out of range"))?;
            let off = ctx.int(byte_offset);
            let dir = ctx.int(whence);
            let position = Wrapper::invoke_buffer(ctx, receiver, "seek", vec![off, dir])?;
            let beginning = !ctx.truth(position)?;
            let w = Wrapper::from_receiver(ctx, receiver)?;
            let mut input = Reader::after_seek(beginning, whence == 0);
            if let Some(bits) = wide_cookie {
                let flag = (bits >> 64) >> usize::from(matches!(w.newline.get(), None | Some("")));
                input.beginning = flag == 2;
                if flag == 1 {
                    input.order = -NATIVE_ORDER;
                }
                input.wide_snapshot = true;
            }
            w.input.replace(input);
            w.write_start.set(beginning);
            w.utf8_signature_pending.set(beginning);
            Ok(if wide_cookie.is_some() {
                cookie
            } else {
                position
            })
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("unknown text stream operation {name}"),
        )),
    }
}
