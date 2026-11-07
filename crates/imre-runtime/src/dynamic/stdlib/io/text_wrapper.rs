//! Text encoding and buffering over the original Python binary stream.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::super::codecs;
use super::{Context, Error, Id, MemoryStream, NONE, Result, TRUE, Value};
use crate::dynamic::builtins::boolean;
use crate::objects::{Codec, DecodeIssue, FixedUnicode};

mod cookie;
mod reader;
mod replay;
use cookie::Cookie;
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
    "__repr__",
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
    "truncate",
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
    invoke_with(ctx, obj, name, |ctx, method| {
        ctx.call(method, args, Vec::new())
    })
}

pub(super) fn invoke_with(
    ctx: &mut Context,
    obj: Id,
    name: &str,
    call: impl FnOnce(&mut Context, Id) -> Result<Id>,
) -> Result<Id> {
    let mark = ctx.heap.temporary_mark();
    ctx.heap.keep(obj);
    let result = ctx
        .lookup_attr(obj, name)
        .and_then(|method| call(ctx, method));
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

fn flush_open(ctx: &mut Context, stream: Id) -> Result<()> {
    let closed = ctx
        .lookup_attr(stream, "closed")
        .and_then(|value| ctx.truth(value))
        .unwrap_or(false);
    if !closed {
        invoke(ctx, stream, "flush", vec![])?;
    }
    Ok(())
}

/// Flush the active sys bindings. Displaced original streams may still own buffered bytes.
pub(in crate::dynamic) fn flush_standard_outputs(ctx: &mut Context) -> bool {
    if ctx.threads.system_module.is_none() {
        return true;
    }
    let mark = ctx.heap.temporary_mark();
    let mut active = [None; 2];
    let mut ok = true;
    for (index, name) in ["stdout", "stderr"].into_iter().enumerate() {
        if let Ok(Some(stream)) = ctx.sys_binding(name)
            && stream != NONE
        {
            ctx.heap.keep(stream);
            active[index] = Some(stream);
            if let Err(error) = flush_open(ctx, stream) {
                ok = false;
                if index == 0 {
                    crate::dynamic::unraisable::report(
                        ctx,
                        error,
                        false,
                        None,
                        "Exception ignored while flushing sys.stdout",
                    );
                }
            }
        }
    }
    for index in 0..ctx.heap.standard_outputs.len() {
        if let Some(stream) = ctx.heap.standard_outputs[index]
            && !active.contains(&Some(stream))
            && Wrapper::from_receiver(ctx, stream)
                .is_ok_and(|wrapper| wrapper.buffer_id().is_some())
        {
            ctx.heap.keep(stream);
            let _ = flush_open(ctx, stream);
        }
    }
    ctx.heap.release_temporaries_since(mark);
    ok
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

/// Render public name and mode under the same recursion guard as other I/O objects.
pub(super) fn repr(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let class = ctx.exact_type(receiver)?;
    Wrapper::from_receiver(ctx, receiver)?;
    let name = if ctx.heap.get(class).io_kind() == Some("TextIOWrapper") {
        "_io.TextIOWrapper"
    } else {
        ctx.class_name(class)
    };
    let name = &name[..name.floor_char_boundary(100)];
    if ctx.repr_ancestors.contains(&receiver) {
        return Err(Error::new(
            "RuntimeError",
            format!("reentrant call inside {name}.__repr__"),
        ));
    }
    let mut output = format!("<{name}")
        .chars()
        .map(u32::from)
        .collect::<Vec<_>>();
    ctx.repr_ancestors
        .try_reserve(1)
        .map_err(|_| crate::Error::allocation())?;
    let roots = ctx.heap.temporary_mark();
    ctx.heap.keep(receiver);
    ctx.repr_ancestors.push(receiver);
    let mut result = Ok(());
    for (field, label) in [
        ("name", " name="),
        ("mode", " mode="),
        ("encoding", " encoding="),
    ] {
        let field_mark = ctx.heap.temporary_mark();
        let mut rendered_mark = None;
        result = (|| {
            let value = if field == "encoding" {
                // Either public getter can reinitialize the wrapper before its private encoding is used.
                Wrapper::from_receiver(ctx, receiver)?.encoding.get()
            } else {
                match ctx.lookup_attr(receiver, field) {
                    Ok(value) => value,
                    Err(err)
                        if field != "encoding"
                            && (ctx.is_attribute_error(&err)
                                || field == "name" && ctx.is_builtin_error(&err, "ValueError")) =>
                    {
                        return Ok(());
                    }
                    Err(err) => return Err(err),
                }
            };
            ctx.heap.keep(value);
            rendered_mark = Some(ctx.heap.temporary_mark());
            let shown = crate::dynamic::builtins::call(ctx, "repr", &[value], &[])?;
            ctx.heap.keep(shown);
            let text = ctx.string_codepoints(shown)?;
            output
                .try_reserve(label.len() + text.len() + 1)
                .map_err(|_| crate::Error::allocation())?;
            output.extend(label.bytes().map(u32::from));
            output.extend_from_slice(&text);
            Ok(())
        })();
        // %R drops the rendered string before the field value, including on a bad return.
        // Their finalizers can reenter this guard or reinitialize the next field.
        for mark in rendered_mark.into_iter().chain([field_mark]) {
            let collect = ctx.heap.discard_truth_operands(mark, ctx.modules);
            if let Err(error) = &result {
                for root in error.roots() {
                    ctx.heap.keep(root);
                }
            }
            if collect {
                ctx.collect_unreachable();
            }
        }
        if result.is_err() {
            break;
        }
    }
    ctx.repr_ancestors.pop();
    ctx.heap.release_temporaries_since(roots);
    if let Err(error) = result {
        for root in error.roots() {
            ctx.heap.keep(root);
        }
        return Err(error);
    }
    output.push(u32::from('>'));
    Ok(ctx.text_codepoints(output))
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

    pub(in crate::dynamic) fn buffer_id(&self) -> Option<Id> {
        self.buffer.get()
    }

    pub(in crate::dynamic) fn children(
        &self,
        visit: &mut impl crate::dynamic::value::VisitReferences,
    ) {
        visit.extend(self.buffer_id());
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
                let order = if signature { 0 } else { format.byte_order() };
                codecs::encode_fixed(ctx, translated, format, &errors, order)
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
                    drop(input);
                    return Ok(ctx.alloc(part));
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
                let mut size = w.chunk_size.get();
                if let Some(limit) = limit
                    && !line
                {
                    size = w.input.borrow().read_size(size, limit);
                }
                (
                    if w.read1 { "read1" } else { "read" },
                    vec![ctx.int(size as i64)],
                )
            };
            // A Python read method may seek or reinitialize the wrapper. The snapshot
            // describes the decoder before the call, as in TextIOWrapper._read_chunk.
            let mut snapshot = if all {
                None
            } else {
                w.input.borrow().capture(
                    w.decoder.get().codec,
                    w.decoder_newline.get(),
                    w.telling.get(),
                )?
            };
            let data = invoke(ctx, buffer, method, args)?;
            if all && data == NONE {
                return Err(Error::new("BlockingIOError", "Read returned None."));
            }
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
            let newline = w.decoder_newline.get();
            let telling = w.telling.get();
            if let Some(saved) = &mut snapshot {
                let raw = Rc::make_mut(&mut saved.input);
                raw.try_reserve(bytes.len())
                    .map_err(|_| crate::Error::allocation())?;
                raw.extend_from_slice(bytes);
            }
            let byte_count = bytes.len();
            let chunk = w.input.borrow_mut().chunk(bytes, decoder.codec, end)?;
            let count = decode_chunk(ctx, &w.input, decoder, newline, chunk, end)?;
            let mut input = w.input.borrow_mut();
            if !all {
                input.record(snapshot, byte_count, count, telling);
                let nonempty = count != 0;
                input.decoded_chunk_nonempty = (!line || !end || nonempty).then_some(nonempty);
                if nonempty && end {
                    // An empty binary read may release a pending CR or replacement.
                    // As long as the decoder emitted text, check the line/size limit normally.
                    continue;
                }
                if line && end {
                    input.clear_snapshot();
                }
            }
            if end {
                let part = input
                    .take(limit, line, w.newline.get(), true)?
                    .expect("read at EOF returns remaining text");
                if all {
                    input.after_read_all();
                }
                drop(input);
                return Ok(ctx.alloc(part));
            }
        }
    }
}

/// Decode into either the live stream or a temporary decoder replaying a saved position.
fn decode_chunk(
    ctx: &mut Context,
    input: &RefCell<Reader>,
    decoder: Coder,
    newline: Option<&str>,
    (codec, bytes): (StreamCodec, Vec<u8>),
    end: bool,
) -> Result<usize> {
    let raw = ctx.alloc(Value::Bytes(bytes));
    let errors = ctx.scalar_value(decoder.errors);
    // Registered error handlers can reenter the stream; no reader borrow spans decoding.
    let decoded = match codec {
        StreamCodec::Registered(c) => codecs::decode_bytes(ctx, c, raw, Some(errors))?,
        StreamCodec::Wide { format, bom } => {
            let errors = ctx.str_value(errors)?.to_owned();
            let requested = if bom { 0 } else { format.byte_order() };
            let (decoded, consumed, order) =
                codecs::decode_wide_chunk(ctx, raw, &errors, format, requested, end)?;
            let data = ctx.bytes_like(raw)?;
            if bom && order == 0 && consumed >= format.width() {
                let issue = DecodeIssue {
                    encoding: format.codec_name(true),
                    start: 0,
                    end: format.width(),
                    reason: "Stream does not start with BOM",
                };
                let error = issue.error(data);
                return Err(ctx.decode_error(error));
            }
            let mut reader = input.borrow_mut();
            reader.save_tail(&data[consumed.min(data.len())..])?;
            reader.order = order;
            if order != 0 {
                reader.beginning = false;
            }
            decoded
        }
    };
    let storage = ctx
        .string_storage(decoded)
        .ok_or_else(|| Error::new("TypeError", "expected str"))?;
    input
        .borrow_mut()
        .append(ctx.heap.get(storage), newline, end)
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

/// Flush the text stream, then pass the original position to its current binary buffer.
pub(super) fn truncate(
    ctx: &mut Context,
    receiver: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "TextIOWrapper.truncate() takes no keyword arguments",
        ));
    }
    super::argument_count("TextIOWrapper", "truncate", args.len())?;
    let buffer = |ctx: &Context| Wrapper::from_receiver(ctx, receiver)?.binary();
    buffer(ctx)?;
    invoke(ctx, receiver, "flush", vec![])?;
    let stream = buffer(ctx)?;
    invoke(
        ctx,
        stream,
        "truncate",
        vec![args.first().copied().unwrap_or(NONE)],
    )
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
            if let Value::Instance {
                class,
                member_values: Some(members),
                ..
            } = ctx.heap.get(receiver)
                && let Some(base) = ctx.find_in_mro(*class, |base| {
                    (ctx.heap.get(base).io_kind() == Some("TextIOWrapper")).then_some(base)
                })
                && let Some(member) = ctx.own_class_attr(base, "_finalizing")
                && members.get(&member) == Some(&TRUE)
            {
                // A warning hook can raise or replace the buffer before flush and close.
                let mark = ctx.heap.temporary_mark();
                let _ = Wrapper::invoke_buffer(ctx, receiver, "_dealloc_warn", vec![receiver]);
                if ctx.heap.discard_truth_operands(mark, ctx.modules) {
                    ctx.collect_unreachable();
                }
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
                w.drain(ctx, receiver)?;
                invoke(ctx, receiver, "flush", vec![])?;
                let pos = Wrapper::invoke_buffer(ctx, receiver, "tell", vec![])?;
                return Wrapper::position(ctx, receiver, pos);
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
                let w = Wrapper::from_receiver(ctx, receiver)?;
                let mut input = w.input.borrow_mut();
                input.discard();
                input.reset_decoder();
                drop(input);
                let zero = ctx.int(0);
                let end = ctx.int(2);
                let position = Wrapper::invoke_buffer(ctx, receiver, "seek", vec![zero, end])?;
                if Wrapper::from_receiver(ctx, receiver)?.writable.get() {
                    let beginning = ctx.checked_equal(position, zero)?;
                    let w = Wrapper::from_receiver(ctx, receiver)?;
                    w.write_start.set(beginning);
                    w.utf8_signature_pending.set(beginning);
                }
                return Ok(position);
            }

            let state = Cookie::parse(&offset)?;
            let off = ctx.int(state.start);
            Wrapper::invoke_buffer(ctx, receiver, "seek", vec![off])?;
            let w = Wrapper::from_receiver(ctx, receiver)?;
            w.input
                .borrow_mut()
                .after_seek(state, w.decoder.get().codec, w.decoder_newline.get());

            if state.skip != 0 {
                // Even an EOF-only cookie must issue read(0). The underlying file
                // may have changed since tell(), so decode the actual bytes returned.
                let size = ctx.int(i64::from(state.feed));
                let data = Wrapper::invoke_buffer(ctx, receiver, "read", vec![size])?;
                let Value::Bytes(bytes) = ctx.heap.get(ctx.scalar_value(data)) else {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "underlying read() should have returned a bytes object, not '{}'",
                            ctx.type_name(data)
                        ),
                    ));
                };
                let w = Wrapper::from_receiver(ctx, receiver)?;
                let decoder = w.decoder.get();
                let mut input = w.input.borrow_mut();
                input.seek_input(state.flags, bytes)?;
                let chunk = input.chunk(bytes, decoder.codec, state.eof)?;
                drop(input);
                let chars = decode_chunk(
                    ctx,
                    &w.input,
                    decoder,
                    w.decoder_newline.get(),
                    chunk,
                    state.eof,
                )?;
                let mut input = w.input.borrow_mut();
                input.decoded_chunk_nonempty = Some(chars != 0);
                input.skip_chars(state.skip as usize)?;
            }
            let w = Wrapper::from_receiver(ctx, receiver)?;
            let beginning = state.start == 0 && state.flags == 0;
            w.write_start.set(beginning);
            w.utf8_signature_pending.set(beginning);
            Ok(cookie)
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("unknown text stream operation {name}"),
        )),
    }
}
