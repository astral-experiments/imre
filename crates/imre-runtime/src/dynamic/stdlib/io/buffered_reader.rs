//! Buffered input over a Python raw reader. No raw callback runs with a buffer borrow held.

use std::cell::Cell;
use std::rc::Rc;

use super::text_wrapper::invoke;
use super::{Context, Error, Id, MemoryStream, NONE, Result, TRUE, Value};

pub(super) const METHODS: &[&str] = &[
    "__init__",
    "raw",
    "closed",
    "name",
    "mode",
    "readable",
    "seekable",
    "isatty",
    "fileno",
    "flush",
    "close",
    "detach",
    "tell",
    "seek",
    "read",
    "read1",
    "readinto",
    "readinto1",
    "peek",
    "readline",
    "__enter__",
    "__exit__",
];

pub(in crate::dynamic) struct Reader {
    pub(in crate::dynamic) raw: Cell<Option<Id>>,
    size: usize,
    pub(in crate::dynamic) buffer: Id,
    end: Cell<usize>,
    pos: Cell<usize>,
    absolute: Cell<Option<i64>>,
    locked: Cell<bool>,
}

fn initialize(ctx: &mut Context, receiver: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if args.len() + kwargs.len() > 2 {
        return Err(Error::new(
            "TypeError",
            format!(
                "BufferedReader() takes at most 2 arguments ({} given)",
                args.len() + kwargs.len()
            ),
        ));
    }
    for (key, _) in kwargs {
        if !matches!(key.as_str(), "raw" | "buffer_size") {
            return Err(Error::new(
                "TypeError",
                format!("'{key}' is an invalid keyword argument for BufferedReader()"),
            ));
        }
    }
    let raw = super::super::option(args, 0, kwargs, "raw")?.ok_or_else(|| {
        Error::new(
            "TypeError",
            "BufferedReader() missing required argument 'raw' (pos 1)",
        )
    })?;
    let size = match super::super::option(args, 1, kwargs, "buffer_size")? {
        None => super::DEFAULT_BUFFER_SIZE,
        Some(value) => index(ctx, value)?,
    };
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        return Err(Error::new(
            "TypeError",
            "expected a BufferedReader instance",
        ));
    };
    if let Some(MemoryStream::Reader(old)) = memory_stream.take()
        && let Some(id) = old.raw.get()
    {
        ctx.heap.weak_edge_removed(id);
    }
    if invoke(ctx, raw, "readable", vec![])? != TRUE {
        return Err(Error::new(
            "UnsupportedOperation",
            "File or stream is not readable.",
        ));
    }
    if size <= 0 {
        return Err(Error::new(
            "ValueError",
            "buffer size must be strictly positive",
        ));
    }
    let buffer = ctx.alloc(Value::ByteArray(super::zeroed_bytes(size)?));
    let absolute = invoke(ctx, raw, "tell", vec![])
        .and_then(|n| index(ctx, n))
        .ok()
        .filter(|n| *n >= 0);
    let state = Reader {
        raw: Cell::new(Some(raw)),
        size: size as usize,
        buffer,
        end: Cell::new(0),
        pos: Cell::new(0),
        absolute: Cell::new(absolute),
        locked: Cell::new(false),
    };
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    *memory_stream = Some(MemoryStream::Reader(Rc::new(state)));
    Ok(NONE)
}

fn index(ctx: &mut Context, id: Id) -> Result<i64> {
    ctx.index_integer(id)?.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

fn append(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    out.try_reserve(bytes.len())
        .map_err(|_| crate::Error::allocation())?;
    out.extend_from_slice(bytes);
    Ok(())
}

impl Reader {
    fn get(ctx: &Context, receiver: Id) -> Result<Rc<Self>> {
        match ctx.heap.get(receiver) {
            Value::Instance {
                memory_stream: Some(MemoryStream::Reader(r)),
                ..
            } => Ok(Rc::clone(r)),
            _ => Err(Error::new(
                "ValueError",
                "I/O operation on uninitialized object",
            )),
        }
    }

    fn raw(&self) -> Result<Id> {
        self.raw
            .get()
            .ok_or_else(|| Error::new("ValueError", "raw stream has been detached"))
    }

    fn unread(&self) -> usize {
        self.end.get() - self.pos.get()
    }

    fn consume(&self, ctx: &Context, out: &mut Vec<u8>, limit: usize, line: bool) -> Result<()> {
        let data = ctx.bytes_like(self.buffer)?;
        let offset = self.pos.get();
        let n = limit.min(self.unread());
        let bytes = &data[offset..offset + n];
        let n = if line {
            bytes.iter().position(|c| *c == b'\n').map_or(n, |i| i + 1)
        } else {
            n
        };
        append(out, &bytes[..n])?;
        self.pos.set(offset + n);
        Ok(())
    }

    fn reset(&self) {
        self.end.set(0);
        self.pos.set(0);
    }

    fn advance(&self, count: usize) {
        self.absolute.set(
            self.absolute
                .get()
                .and_then(|offset| offset.checked_add(count as i64)),
        );
    }

    fn check_open(&self, ctx: &mut Context, message: &str) -> Result<()> {
        let closed = ctx.lookup_attr(self.raw()?, "closed")?;
        if ctx.truth(closed)? {
            Err(Error::new("ValueError", message))
        } else {
            Ok(())
        }
    }

    fn with_lock<T>(&self, work: impl FnOnce() -> Result<T>) -> Result<T> {
        if self.locked.replace(true) {
            return Err(Error::new(
                "RuntimeError",
                "reentrant call inside buffered reader",
            ));
        }
        let result = work();
        self.locked.set(false);
        result
    }

    // The raw method may retain a view onto its precise destination. The reader keeps a
    // fixed backing allocation for the lifetime of every view into its read-ahead buffer.
    fn raw_into(
        &self,
        ctx: &mut Context,
        backing: Id,
        start: usize,
        length: usize,
    ) -> Result<Option<usize>> {
        let raw = self.raw()?;
        let readinto = ctx.lookup_attr(raw, "readinto")?;
        let dest = ctx.ownerless_byte_view(backing, start, length)?;
        let value = ctx.call(readinto, vec![dest], vec![])?;
        if value == NONE {
            return Ok(None);
        }
        let count = match index(ctx, value) {
            Ok(n) => n,
            Err(cause) => {
                let cause = ctx.raised_instance(cause);
                let mut err = Error::new("OSError", "raw readinto() failed");
                err.context = cause;
                err.tracing().cause = Some(cause);
                return Err(err);
            }
        };
        if count < 0 || count as usize > length {
            return Err(Error::new(
                "OSError",
                format!(
                    "raw readinto() returned invalid length {count} (should have been between 0 and {length})"
                ),
            ));
        }
        let count = count as usize;
        self.advance(count);
        Ok(Some(count))
    }

    fn read_raw(&self, ctx: &mut Context, amount: usize) -> Result<Option<Vec<u8>>> {
        self.reset();
        let buffer = ctx.alloc(Value::ByteArray(super::zeroed_bytes(amount as i64)?));
        let Some(n) = self.raw_into(ctx, buffer, 0, amount)? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        append(&mut bytes, &ctx.bytes_like(buffer)?[..n])?;
        Ok(Some(bytes))
    }

    fn fill(&self, ctx: &mut Context, continuing: bool) -> Result<bool> {
        let start = if continuing && self.end.get() < self.size {
            self.end.get()
        } else {
            0
        };
        self.end.set(start);
        self.pos.set(start);
        let count = self.raw_into(ctx, self.buffer, start, self.size - start)?;
        self.end.set(start + count.unwrap_or(0));
        Ok(count.is_some())
    }

    fn read_into(&self, ctx: &mut Context, dst: Id, once: bool) -> Result<Id> {
        let Some((target, range)) = ctx.writable_byte_span(dst) else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{}() argument must be read-write bytes-like object, not {}",
                    if once { "readinto1" } else { "readinto" },
                    ctx.type_name(dst)
                ),
            ));
        };
        let size = range.len();
        self.check_open(ctx, "readinto of closed file")?;
        ctx.with_buffer_export(target, |ctx| {
            self.with_lock(|| {
                let mut written = 0;
                let mut continuing = false;
                loop {
                    let n = (size - written).min(self.unread());
                    if n != 0 {
                        let offset = self.pos.get();
                        let start = range.start + written;
                        if self.buffer == target {
                            let Value::ByteArray(bytes) = ctx.heap.get_mut(target) else {
                                unreachable!()
                            };
                            bytes.copy_within(offset..offset + n, start);
                        } else {
                            let Some((Value::ByteArray(bytes), Value::ByteArray(dest))) =
                                ctx.heap.source_and_target(self.buffer, target)
                            else {
                                unreachable!()
                            };
                            dest[start..start + n].copy_from_slice(&bytes[offset..offset + n]);
                        }
                        self.pos.set(offset + n);
                        written += n;
                    }
                    let remaining = size - written;
                    if remaining == 0 {
                        break;
                    }
                    if remaining > self.size {
                        self.reset();
                        match self.raw_into(ctx, target, range.start + written, remaining)? {
                            Some(n) => {
                                written += n;
                                if n == 0 || once {
                                    break;
                                }
                            }
                            None if written == 0 => return Ok(NONE),
                            None => break,
                        }
                    } else {
                        if once && written != 0 {
                            break;
                        }
                        if !self.fill(ctx, continuing)? && written == 0 {
                            return Ok(NONE);
                        }
                        continuing = true;
                        if self.unread() == 0 {
                            break;
                        }
                    }
                }
                Ok(ctx.int(written as i64))
            })
        })
    }

    fn bounded(&self, ctx: &mut Context, name: &str, n: usize) -> Result<Id> {
        let mut out = Vec::new();
        match name {
            "peek" => {
                if self.unread() == 0 {
                    self.fill(ctx, false)?;
                }
                append(
                    &mut out,
                    &ctx.bytes_like(self.buffer)?[self.pos.get()..self.end.get()],
                )?;
            }
            "read1" if self.unread() == 0 && n != 0 => {
                out = self.read_raw(ctx, n)?.unwrap_or_default();
            }
            _ => {
                let line = name == "readline";
                let mut continuing = false;
                loop {
                    let remaining = n - out.len();
                    self.consume(ctx, &mut out, remaining, line)?;
                    if out.len() == n || name == "read1" || line && out.last() == Some(&b'\n') {
                        break;
                    }
                    let remaining = n - out.len();
                    if !line && remaining >= self.size {
                        let amount = remaining - remaining % self.size;
                        match self.read_raw(ctx, amount)? {
                            Some(bytes) if !bytes.is_empty() => {
                                append(&mut out, &bytes)?;
                                continue;
                            }
                            None if out.is_empty() => return Ok(NONE),
                            _ => break,
                        }
                    }
                    if !self.fill(ctx, continuing && !line)? && !line && out.is_empty() {
                        return Ok(NONE);
                    }
                    continuing = true;
                    if self.unread() == 0 {
                        break;
                    }
                }
            }
        }
        Ok(ctx.alloc(Value::Bytes(out)))
    }

    fn read_all(&self, ctx: &mut Context) -> Result<Id> {
        let mut data = Vec::new();
        self.consume(ctx, &mut data, usize::MAX, false)?;
        self.reset();
        let raw = self.raw()?;
        let readall = match ctx.lookup_attr(raw, "readall") {
            Ok(f) => Some(f),
            Err(e) if e.kind == "AttributeError" => None,
            Err(e) => return Err(e),
        };
        loop {
            let id = if let Some(f) = readall {
                ctx.call(f, vec![], vec![])?
            } else {
                invoke(ctx, raw, "read", vec![])?
            };
            if id == NONE {
                if data.is_empty() {
                    return Ok(NONE);
                }
                break;
            }
            let Value::Bytes(bytes) = ctx.heap.get(ctx.scalar_value(id)) else {
                let method = if readall.is_some() { "readall" } else { "read" };
                return Err(Error::new(
                    "TypeError",
                    format!("{method}() should return bytes"),
                ));
            };
            self.advance(bytes.len());
            if data.is_empty() && (readall.is_some() || bytes.is_empty()) {
                return Ok(id);
            }
            if bytes.is_empty() {
                break;
            }
            append(&mut data, bytes)?;
            if readall.is_some() {
                break;
            }
        }
        Ok(ctx.alloc(Value::Bytes(data)))
    }

    fn seek(&self, ctx: &mut Context, offset: i64, whence: i64) -> Result<Id> {
        self.check_open(ctx, "seek of closed file")?;
        let raw = self.raw()?;
        if !matches!(whence, 0..=4) {
            return Err(Error::new(
                "ValueError",
                format!("whence value {whence} unsupported"),
            ));
        }
        if invoke(ctx, raw, "seekable", vec![])? != TRUE {
            return Err(Error::new(
                "UnsupportedOperation",
                "File or stream is not seekable.",
            ));
        }
        let unread = self.unread() as i64;
        if let Some(end) = self.absolute.get() {
            let target = match whence {
                0 => Some(offset),
                1 => (end - unread).checked_add(offset),
                _ => None,
            };
            let start = end - self.end.get() as i64;
            if let Some(target) = target
                && (start..=end).contains(&target)
                && self.end.get() != 0
            {
                self.pos.set((target - start) as usize);
                return Ok(ctx.int(target));
            }
        }
        let offset = if whence == 1 {
            offset
                .checked_sub(unread)
                .ok_or_else(|| Error::new("OverflowError", "seek offset out of range"))?
        } else {
            offset
        };
        let (off, whence) = (ctx.int(offset), ctx.int(whence));
        let value = invoke(ctx, raw, "seek", vec![off, whence])?;
        let pos = index(ctx, value)?;
        if pos < 0 {
            return Err(Error::new(
                "OSError",
                format!("Raw stream returned invalid position {pos}"),
            ));
        }
        self.end.set(0);
        self.pos.set(0);
        self.absolute.set(Some(pos));
        Ok(ctx.int(pos))
    }
}

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
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("BufferedReader.{name}() takes no keyword arguments"),
        ));
    }
    if name == "peek" && args.len() > 1 {
        return Err(Error::new("TypeError", "peek expected at most 1 argument"));
    }
    if name == "readinto1" && args.len() != 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "BufferedReader.readinto1() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    if !matches!(name, "peek" | "readinto1") {
        super::argument_count("BufferedReader", name, args.len())?;
    }
    let size = match name {
        "read" | "readline" => super::super::files::read_length(ctx, args.first().copied())?,
        "read1" | "peek" => args
            .first()
            .map(|&id| index(ctx, id))
            .transpose()?
            .unwrap_or(-1),
        _ => 0,
    };
    let offset = if name == "seek" {
        Some((
            index(ctx, args[0])?,
            args.get(1)
                .map(|&id| index(ctx, id))
                .transpose()?
                .unwrap_or(0),
        ))
    } else {
        None
    };
    if name == "raw" {
        return Ok(Reader::get(ctx, receiver)
            .ok()
            .and_then(|r| r.raw.get())
            .unwrap_or(NONE));
    }
    let reader = Reader::get(ctx, receiver)?;
    let raw = reader.raw()?;
    ctx.heap.keep(raw);
    ctx.heap.keep(reader.buffer);
    match name {
        "readinto" | "readinto1" => reader.read_into(ctx, args[0], name == "readinto1"),
        "closed" | "name" | "mode" => ctx.lookup_attr(raw, name),
        "readable" | "seekable" | "fileno" | "isatty" | "flush" => invoke(ctx, raw, name, vec![]),
        "__enter__" => {
            reader.check_open(ctx, "I/O operation on closed file.")?;
            Ok(receiver)
        }
        "__exit__" => invoke(ctx, receiver, "close", vec![]),
        "close" => {
            reader.with_lock(|| Ok(()))?;
            let closed = ctx.lookup_attr(raw, "closed")?;
            if ctx.truth(closed)? {
                return Ok(NONE);
            }
            let flush = invoke(ctx, receiver, "flush", vec![]);
            if let Err(e) = &flush {
                for id in e.roots() {
                    ctx.heap.keep(id);
                }
            }
            let result = reader
                .with_lock(|| invoke(ctx, Reader::get(ctx, receiver)?.raw()?, "close", vec![]));
            reader.pos.set(0);
            reader.end.set(0);
            match (flush, result) {
                (Err(f), Err(mut e)) => {
                    e.context = ctx.raised_instance(f);
                    Err(e)
                }
                (Err(f), _) => Err(f),
                (_, r) => r,
            }
        }
        "detach" => {
            invoke(ctx, receiver, "flush", vec![])?;
            let live = Reader::get(ctx, receiver)?;
            let raw = live.raw()?;
            live.raw.set(None);
            ctx.heap.keep(raw);
            ctx.heap.weak_edge_removed(raw);
            Ok(raw)
        }
        "tell" => {
            let n = invoke(ctx, raw, "tell", vec![])?;
            let end = index(ctx, n)?;
            if end < 0 {
                return Err(Error::new(
                    "OSError",
                    format!("Raw stream returned invalid position {end}"),
                ));
            }
            reader.absolute.set(Some(end));
            Ok(ctx.int((end - reader.unread() as i64).max(0)))
        }
        "seek" => {
            let (offset, whence) = offset.unwrap();
            reader.with_lock(|| reader.seek(ctx, offset, whence))
        }
        "read" | "read1" | "peek" | "readline" => {
            if name == "read" && size < -1 {
                return Err(Error::new(
                    "ValueError",
                    "read length must be non-negative or -1",
                ));
            }
            reader.check_open(
                ctx,
                match name {
                    "readline" => "readline of closed file",
                    "peek" => "peek of closed file",
                    _ => "read of closed file",
                },
            )?;
            reader.with_lock(|| {
                if name == "read" && size == -1 {
                    reader.read_all(ctx)
                } else {
                    let limit = if size < 0 {
                        if name == "readline" {
                            usize::MAX
                        } else {
                            reader.size
                        }
                    } else {
                        size as usize
                    };
                    reader.bounded(ctx, name, limit)
                }
            })
        }
        _ => unreachable!("registered BufferedReader method"),
    }
}
