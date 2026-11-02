//! Buffered input over a Python raw reader. No raw callback runs with a buffer borrow held.

mod update;
use update::Update;

use std::cell::Cell;
use std::rc::Rc;

use super::super::files;
use super::descriptor_buffer::ssize as index;
use super::text_wrapper::invoke;
use super::{Context, Error, Id, MemoryStream, NONE, Result, TRUE, Value};

pub(in crate::dynamic) struct Reader {
    pub(in crate::dynamic) raw: Cell<Option<Id>>,
    size: usize,
    pub(in crate::dynamic) buffer: Id,
    end: Cell<usize>,
    pos: Cell<usize>,
    absolute: Cell<Option<i64>>,
    locked: Cell<bool>,
    update: Option<Update>,
}

pub(super) fn has_state(ctx: &Context, receiver: Id) -> bool {
    matches!(
        ctx.heap.get(receiver),
        Value::Instance {
            memory_stream: Some(MemoryStream::Reader(_) | MemoryStream::UninitializedReader(_)),
            ..
        }
    )
}

fn raw_member(ctx: &Context, receiver: Id) -> Option<Id> {
    match ctx.heap.get(receiver) {
        Value::Instance {
            memory_stream: Some(MemoryStream::Reader(reader)),
            ..
        } => reader.raw.get(),
        Value::Instance {
            memory_stream: Some(MemoryStream::UninitializedReader(raw)),
            ..
        } => *raw,
        _ => super::descriptor_buffer::explicit_reader_raw(ctx, receiver),
    }
}

/// Reinitialization invalidates buffered operations; the public raw slot changes only
/// after readable() succeeds, even if allocation or size validation subsequently fails.
fn start_init(ctx: &mut Context, receiver: Id, raw: Option<Id>) -> Result<()> {
    let previous = raw_member(ctx, receiver);
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        return Err(Error::new(
            "TypeError",
            "expected a BufferedReader instance",
        ));
    };
    *memory_stream = Some(MemoryStream::UninitializedReader(raw));
    if let Some(id) = previous
        && previous != raw
    {
        ctx.heap.weak_edge_removed(id);
    }
    Ok(())
}

pub(super) fn initialize(ctx: &mut Context, receiver: Id, raw: Id, size: i64) -> Result<Id> {
    start_init(ctx, receiver, raw_member(ctx, receiver))?;
    if invoke(ctx, raw, "readable", vec![])? != TRUE {
        return Err(Error::new(
            "UnsupportedOperation",
            "File or stream is not readable.",
        ));
    }
    start_init(ctx, receiver, Some(raw))?;
    if size <= 0 {
        return Err(Error::new(
            "ValueError",
            "buffer size must be strictly positive",
        ));
    }
    install(ctx, receiver, raw, size, false)
}

pub(super) fn install(
    ctx: &mut Context,
    receiver: Id,
    raw: Id,
    size: i64,
    update: bool,
) -> Result<Id> {
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
        update: update.then(Update::default),
    };
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    *memory_stream = Some(MemoryStream::Reader(Rc::new(state)));
    Ok(NONE)
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
        self.end.get().saturating_sub(self.pos.get())
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
        if let Some(update) = &self.update {
            update.raw_offset.set(0);
            update.outside.set(None);
        }
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
        if self
            .update
            .as_ref()
            .is_some_and(|update| update.retired.get())
        {
            return Err(Error::new("ValueError", message));
        }
        self.check_raw_open(ctx, message)
    }

    fn check_raw_open(&self, ctx: &mut Context, message: &str) -> Result<()> {
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
        let mark = ctx.heap.temporary_mark();
        let raw = self.raw()?;
        let dest = ctx.ownerless_byte_view(backing, start, length, false)?;
        let retry = ctx.heap.temporary_mark();
        let value = loop {
            match invoke(ctx, raw, "readinto", vec![dest]) {
                Err(error) if ctx.has_errno(&error, libc::EINTR) => {
                    ctx.heap.release_temporaries_since(retry);
                }
                result => break result?,
            }
        };
        if value == NONE {
            ctx.heap.release_temporaries_since(mark);
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
        ctx.heap.release_temporaries_since(mark);
        self.advance(count);
        Ok(Some(count))
    }

    fn read_raw(&self, ctx: &mut Context, amount: usize) -> Result<Option<Vec<u8>>> {
        self.reset();
        let mark = ctx.heap.temporary_mark();
        let buffer = ctx.alloc(Value::ByteArray(super::zeroed_bytes(amount as i64)?));
        let Some(n) = self.raw_into(ctx, buffer, 0, amount)? else {
            ctx.heap.release_temporaries_since(mark);
            return Ok(None);
        };
        let mut bytes = Vec::new();
        append(&mut bytes, &ctx.bytes_like(buffer)?[..n])?;
        ctx.heap.release_temporaries_since(mark);
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
        if let Some(update) = &self.update {
            update.raw_offset.set(self.end.get() as i64);
        }
        Ok(count.is_some())
    }

    fn read_into(&self, ctx: &mut Context, dst: Id, once: bool) -> Result<Id> {
        let Some((target, range)) = ctx.writable_byte_span(dst)? else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{}() argument must be read-write bytes-like object, not {}",
                    if once { "readinto1" } else { "readinto" },
                    ctx.type_name(dst)
                ),
            ));
        };
        ctx.with_buffer_export(dst, |ctx| {
            ctx.with_buffer_export(target, |ctx| self.read_into_span(ctx, target, range, once))
        })
    }

    fn read_into_span(
        &self,
        ctx: &mut Context,
        target: Id,
        range: std::ops::Range<usize>,
        once: bool,
    ) -> Result<Id> {
        let size = range.len();
        self.check_open(ctx, "readinto of closed file")?;
        self.with_lock(|| {
            if size == 0 && self.unread() == 0 {
                self.sync_update(ctx)?;
            }
            let mut written = 0;
            let mut continuing = false;
            let mut rewind = true;
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
                        let Some((Value::ByteArray(bytes), dest)) =
                            ctx.heap.source_and_target(self.buffer, target)
                        else {
                            unreachable!("distinct buffered readinto source and destination")
                        };
                        let dest = Context::mutable_storage_bytes(dest)?;
                        dest[start..start + n].copy_from_slice(&bytes[offset..offset + n]);
                    }
                    self.pos.set(offset + n);
                    written += n;
                }
                let remaining = size - written;
                if remaining == 0 {
                    break;
                }
                if rewind {
                    self.sync_update(ctx)?;
                    rewind = false;
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
    }

    fn bounded(&self, ctx: &mut Context, name: &str, n: usize) -> Result<Id> {
        let mut out = Vec::new();
        match name {
            "peek" => {
                self.sync_update(ctx)?;
                if self.unread() == 0 {
                    self.fill(ctx, false)?;
                }
                append(
                    &mut out,
                    &ctx.bytes_like(self.buffer)?[self.pos.get()..self.end.get()],
                )?;
            }
            "read1" if self.unread() == 0 && n != 0 => {
                self.sync_update(ctx)?;
                out = self.read_raw(ctx, n)?.unwrap_or_default();
            }
            _ => {
                let line = name == "readline";
                let mut continuing = false;
                let mut rewind = true;
                loop {
                    let remaining = n - out.len();
                    self.consume(ctx, &mut out, remaining, line)?;
                    if out.len() == n || name == "read1" || line && out.last() == Some(&b'\n') {
                        break;
                    }
                    if rewind {
                        self.sync_update(ctx)?;
                        rewind = false;
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
        self.sync_update(ctx)?;
        self.reset();
        let raw = self.raw()?;
        let readall = match ctx.lookup_attr(raw, "readall") {
            Ok(f) => Some(f),
            Err(e) if ctx.is_attribute_error(&e) => None,
            Err(e) => return Err(e),
        };
        let mark = ctx.heap.temporary_mark();
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
            ctx.heap.release_temporaries_since(mark);
            if readall.is_some() {
                break;
            }
        }
        Ok(ctx.alloc(Value::Bytes(data)))
    }

    fn seek(&self, ctx: &mut Context, offset: i64, whence: i32) -> Result<Id> {
        let pos = self.logical_pos();
        if let Some(absolute) = self.absolute.get() {
            let start = absolute - self.raw_offset();
            let target = match whence {
                0 => Some(offset),
                1 => start
                    .checked_add(pos)
                    .and_then(|logical| logical.checked_add(offset)),
                _ => None,
            };
            if let Some(target) = target
                && let Some(within) = target.checked_sub(start)
                && (0..=self.end.get() as i64).contains(&within)
                && self.unread() > 0
            {
                self.pos.set(within as usize);
                return Ok(ctx.int(target.max(0)));
            }
        }
        self.with_lock(|| {
            self.write_back(ctx)?;
            let offset = if whence == 1 {
                offset
                    .checked_add(self.logical_pos() - self.raw_offset())
                    .ok_or_else(|| Error::new("OverflowError", "seek offset out of range"))?
            } else {
                offset
            };
            let (off, whence) = (ctx.int(offset), ctx.int(i64::from(whence)));
            let value = invoke(ctx, self.raw()?, "seek", vec![off, whence])?;
            let pos = index(ctx, value)?;
            if pos < 0 {
                return Err(Error::new(
                    "OSError",
                    format!("Raw stream returned invalid position {pos}"),
                ));
            }
            self.reset();
            self.absolute.set(Some(pos));
            Ok(ctx.int(pos))
        })
    }
}

pub(super) fn pending_update(ctx: &Context, receiver: Id) -> Option<Id> {
    let reader = Reader::get(ctx, receiver).ok()?;
    reader.update.as_ref()?.dirty.get()?;
    reader.raw.get()
}

pub(super) fn initialized_raw(ctx: &Context, receiver: Id) -> Option<Id> {
    Reader::get(ctx, receiver).ok()?.raw.get()
}

pub(super) fn close_raw(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let reader = Reader::get(ctx, receiver)?;
    let raw = reader.raw()?;
    let result = reader.with_lock(|| {
        let result = invoke(ctx, raw, "close", vec![]);
        if let Some(update) = &reader.update {
            // Retire the backing on which raw.close was called, even when it refuses.
            // The raw remains attached and determines the public closed property.
            update.retired.set(true);
            update.dirty.set(None);
            reader.reset();
        }
        result
    });
    if reader.update.is_none() {
        reader.reset();
    }
    result
}

pub(super) fn call(ctx: &mut Context, receiver: Id, name: &str, args: &[Id]) -> Result<Id> {
    let size = match name {
        "read" | "readline" => super::super::files::read_length(ctx, args.first().copied())?,
        "read1" => args
            .first()
            .map(|&id| index(ctx, id))
            .transpose()?
            .unwrap_or(-1),
        _ => 0,
    };
    let whence = if name == "seek" {
        files::whence_argument(ctx, args.get(1).copied())?
    } else {
        0
    };
    if name == "raw" {
        return Ok(raw_member(ctx, receiver).unwrap_or(NONE));
    }
    let reader = Reader::get(ctx, receiver)?;
    let raw = reader.raw()?;
    ctx.heap.keep(raw);
    ctx.heap.keep(reader.buffer);
    match name {
        "readinto" | "readinto1" => reader.read_into(ctx, args[0], name == "readinto1"),
        "closed" | "name" | "mode" => ctx.lookup_attr(raw, name),
        "write" | "flush" | "truncate" if reader.update.is_some() => {
            reader.update_call(ctx, name, args)
        }
        "readable" | "writable" | "seekable" | "fileno" | "isatty" | "flush" => {
            invoke(ctx, raw, name, vec![])
        }
        "truncate" => {
            reader.check_open(ctx, "truncate of closed file")?;
            Err(Error::new("UnsupportedOperation", "truncate"))
        }
        "__enter__" => {
            reader.check_raw_open(ctx, "I/O operation on closed file.")?;
            Ok(receiver)
        }
        "__exit__" => invoke(ctx, receiver, "close", vec![]),
        "close" => {
            let closed = reader.with_lock(|| {
                let closed = ctx.lookup_attr(raw, "closed")?;
                let closed = ctx.truth(closed)?;
                if !closed && ctx.raw_instance_field(receiver, "_finalizing") == Some(TRUE) {
                    super::native_reader::dealloc_warn(ctx, receiver, receiver);
                }
                Ok(closed)
            })?;
            if closed {
                return Ok(NONE);
            }
            super::native_reader::flush_and_close(ctx, receiver)
        }
        "detach" => {
            invoke(ctx, receiver, "flush", vec![])?;
            // A flush override may have reinitialized this wrapper over a native FileIO.
            // Leave its descriptor attached until that backing supports detach.
            if super::descriptor_buffer::backing(ctx, receiver).is_some() {
                return Err(Error::new("UnsupportedOperation", "detach"));
            }
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
            let logical = end.saturating_add(reader.logical_pos() - reader.raw_offset());
            Ok(ctx.int(logical.max(0)))
        }
        "seek" => {
            files::check_buffered_whence(whence)?;
            reader.check_open(ctx, "seek of closed file")?;
            let raw = Reader::get(ctx, receiver)?.raw()?;
            if invoke(ctx, raw, "seekable", vec![])? != TRUE {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "File or stream is not seekable.",
                ));
            }
            // The offset protocol may read, seek, or reinitialize this reader.
            let offset = files::buffered_offset(ctx, args[0])?;
            Reader::get(ctx, receiver)?.seek(ctx, offset, whence)
        }
        "read" | "read1" | "readline" | "peek" => {
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
