//! Text encoding and buffering over the original Python binary stream.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::{Context, Error, Id, MemoryStream, NONE, Result, Value};
use crate::dynamic::builtins::boolean;
use crate::objects::Codec;

mod reader;
use reader::Reader;

const CHUNK: usize = 8192;
pub(super) const METHODS: &[&str] = &[
    "__init__",
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

/// The heap traces the buffer until detach returns it. No borrow spans a Python stream call.
pub(in crate::dynamic) struct Wrapper {
    buffer: Cell<Option<Id>>,
    encoding: Id,
    errors: Id,
    newline: Option<String>,
    codec: Codec,
    line_buffering: bool,
    write_through: bool,
    chunk_size: Cell<usize>,
    readable: bool,
    writable: bool,
    seekable: bool,
    read1: bool,
    write_start: Cell<bool>,
    output: RefCell<Vec<u8>>,
    input: RefCell<Reader>,
}

fn invoke(ctx: &mut Context, obj: Id, name: &str, args: Vec<Id>) -> Result<Id> {
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
    let string = |id, key: &str| -> Result<Option<String>> {
        if id == NONE {
            return Ok(None);
        }
        let v = ctx.str_value(id).map_err(|_| {
            Error::new(
                "TypeError",
                format!(
                    "TextIOWrapper() argument '{key}' must be str or None, not {}",
                    ctx.type_name(id)
                ),
            )
        })?;
        if v.contains('\0') {
            return Err(Error::new("ValueError", "embedded null character"));
        }
        Ok(Some(v.to_owned()))
    };
    let encoding = string(encoding, "encoding")?;
    let encoding = crate::sys::file_encoding(encoding.as_deref()).to_owned();
    string(errors, "errors")?;
    let newline = string(newline, "newline")?;
    if let Some(nl) = &newline
        && !matches!(nl.as_str(), "" | "\r" | "\n" | "\r\n")
    {
        return Err(Error::new(
            "ValueError",
            format!("illegal newline value: {nl}"),
        ));
    }
    let codec = Codec::lookup(&encoding)?;
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
    let state = Wrapper {
        buffer: Cell::new(Some(buffer)),
        codec,
        encoding: ctx.text(encoding),
        errors: if errors == NONE {
            ctx.text("strict")
        } else {
            errors
        },
        newline,
        line_buffering,
        write_through,
        chunk_size: Cell::new(CHUNK),
        readable,
        writable,
        seekable,
        read1,
        write_start: Cell::new(write_start),
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
        visit.extend([self.encoding, self.errors]);
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
        if !self.writable {
            return Err(Error::new("UnsupportedOperation", "not writable"));
        }
        let text = ctx.str_value(value).map_err(|_| {
            Error::new(
                "TypeError",
                format!("write() argument must be str, not {}", ctx.type_name(value)),
            )
        })?;
        let count = text.chars().count();
        let flush = self.line_buffering && text.contains(['\r', '\n']);
        let replacement = match self.newline.as_deref() {
            None if cfg!(windows) => Some("\r\n"),
            Some("\r") => Some("\r"),
            Some("\r\n") => Some("\r\n"),
            _ => None,
        };
        let translated;
        let text = if let Some(nl) = replacement {
            translated = text.replace('\n', nl);
            &translated
        } else {
            text
        };
        let encoding =
            if matches!(self.codec, Codec::Utf8 { signature: true }) && !self.write_start.get() {
                "utf-8"
            } else {
                ctx.str_value(self.encoding)?
            };
        let encoded = Codec::encode_str(text, encoding, ctx.str_value(self.errors)?)?;
        self.write_start.set(false);
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
        if flush || self.write_through || length >= self.chunk_size.get() {
            self.drain(ctx, receiver)?;
        }
        if flush {
            Self::invoke_buffer(ctx, receiver, "flush", vec![])?;
        }
        self.input.borrow_mut().discard();
        Ok(ctx.int(count as i64))
    }

    fn read(ctx: &mut Context, receiver: Id, limit: Option<usize>, line: bool) -> Result<Id> {
        let w = Self::from_receiver(ctx, receiver)?;
        w.binary()?;
        if !w.readable {
            return Err(Error::new("UnsupportedOperation", "not readable"));
        }
        w.drain(ctx, receiver)?;
        let all = !line && limit.is_none();
        loop {
            let w = Self::from_receiver(ctx, receiver)?;
            let buffer = w.binary()?;
            if !all
                && let Some(part) =
                    w.input
                        .borrow_mut()
                        .take(limit, line, w.newline.as_deref(), false)
            {
                return Ok(ctx.text(part));
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
            let (codec, raw) = w.input.borrow_mut().chunk(bytes, w.codec, end)?;
            let raw = ctx.alloc(Value::Bytes(raw));
            // The ordinary native codec entry owns error handling. A registered handler
            // may run Python, so release the reader borrow before invoking it.
            let errors = ctx.scalar_value(w.errors);
            let decoded = super::super::codecs::decode(ctx, codec, &[raw, errors], &[])?;
            let Value::Tuple(parts) = ctx.heap.get(decoded) else {
                unreachable!("native codec decoder returns (text, consumed)");
            };
            w.input.borrow_mut().append(
                ctx.str_value(parts[0])?,
                ctx.bytes_like(raw)?,
                codec,
                w.newline.as_deref(),
                end,
            );
            if end {
                let mut input = w.input.borrow_mut();
                let part = input
                    .take(limit, line, w.newline.as_deref(), true)
                    .unwrap_or_default();
                if all {
                    input.discard();
                }
                return Ok(ctx.text(part));
            }
        }
    }
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
        "encoding" => return Ok(w.encoding),
        "errors" => return Ok(w.errors),
        "line_buffering" => return Ok(boolean(w.line_buffering)),
        "write_through" => return Ok(boolean(w.write_through)),
        "__exit__" => return invoke(ctx, receiver, "close", vec![]),
        _ => {}
    }
    let buffer = w.binary()?;
    match name {
        "_CHUNK_SIZE" => return Ok(ctx.int(w.chunk_size.get() as i64)),
        "closed" | "name" => return ctx.lookup_attr(buffer, name),
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
                    close.tracing().context = Some(ctx.raised_instance(flush));
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
            w.drain(ctx, receiver)?;
            Wrapper::invoke_buffer(ctx, receiver, "flush", vec![])
        }
        "readable" | "writable" | "seekable" | "isatty" | "fileno" => {
            invoke(ctx, buffer, name, vec![])
        }
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
            invoke(ctx, receiver, "flush", vec![])?;
            if name == "tell" {
                let pos = Wrapper::invoke_buffer(ctx, receiver, "tell", vec![])?;
                return w.input.borrow().position(ctx, pos);
            }
            let offset = ctx
                .index_integer(args[0])?
                .to_i64()
                .ok_or_else(|| Error::new("OverflowError", "seek position out of range"))?;
            let whence = match args.get(1) {
                Some(&id) => ctx.as_integer(id)?,
                None => 0,
            };
            let (offset, whence) = match whence {
                0 if offset < 0 => {
                    return Err(Error::new(
                        "ValueError",
                        format!("negative seek position {offset}"),
                    ));
                }
                0 => (offset, 0),
                2 if offset == 0 => (0, 2),
                1 if offset == 0 => {
                    let pos = Wrapper::invoke_buffer(ctx, receiver, "tell", vec![])?;
                    let pos = w.input.borrow().position(ctx, pos)?;
                    (ctx.as_integer(pos)?, 0)
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
            let off = ctx.int(offset);
            let dir = ctx.int(whence);
            let position = Wrapper::invoke_buffer(ctx, receiver, "seek", vec![off, dir])?;
            let beginning = !ctx.truth(position)?;
            w.input.replace(Reader::at(beginning));
            w.write_start.set(beginning);
            Ok(position)
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("unknown text stream operation {name}"),
        )),
    }
}
