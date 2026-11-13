//! Buffered input over a Python raw reader. No raw callback runs with a buffer borrow held.

mod update;
use update::Update;

use crate::objects::{append_read_ahead, append_read_bytes};
use crate::shared::Shared;
use std::cell::Cell;

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
/// An absent replacement keeps the current raw object visible during the callback.
pub(super) fn start_init(ctx: &mut Context, receiver: Id, replacement: Option<Id>) -> Result<()> {
    let previous = raw_member(ctx, receiver);
    let raw = replacement.or(previous);
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
    start_init(ctx, receiver, None)?;
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

/// Discard the outgoing callback buffer without seeking, flushing, or closing its raw.
pub(super) fn reinitialize_update(
    ctx: &mut Context,
    receiver: Id,
    raw: Id,
    size: i64,
    capabilities: &[&str],
) -> Result<bool> {
    let (previous, locked) = match ctx.heap.get(receiver) {
        Value::Instance {
            memory_stream: Some(MemoryStream::Reader(old)),
            ..
        } if old.update.is_some() => (old.raw.get(), old.locked.get()),
        Value::Instance {
            memory_stream: Some(MemoryStream::UninitializedReader(Some(raw))),
            ..
        } => (Some(*raw), false),
        _ => return Ok(false),
    };
    if previous.is_some_and(|id| super::file_io::state(ctx, id).is_some())
        || super::file_io::state(ctx, raw).is_some()
    {
        return Ok(false);
    }
    // A raw I/O callback may currently hold a view into the live buffer. Keep
    // that operation and its ownership intact instead of replacing its state.
    if locked {
        return Err(Error::new(
            "RuntimeError",
            "reentrant call inside buffered reader",
        ));
    }
    start_init(ctx, receiver, previous)?;
    super::descriptor_buffer::check_capabilities(ctx, raw, capabilities)?;
    if let Ok(nested) = Reader::get(ctx, receiver) {
        if nested
            .raw
            .get()
            .is_some_and(|id| super::file_io::state(ctx, id).is_some())
        {
            return Err(Error::new(
                "NotImplementedError",
                "BufferedRandom reinitialization is unsupported",
            ));
        }
        // A capability callback made the receiver usable. It stays usable while the
        // outer call publishes its raw, validates size, and probes the position.
        if let Some(previous) = nested.raw.replace(Some(raw))
            && previous != raw
        {
            ctx.heap.weak_edge_removed(previous);
        }
    } else if super::descriptor_buffer::backing(ctx, receiver).is_some() {
        return Err(Error::new(
            "NotImplementedError",
            "BufferedRandom reinitialization is unsupported",
        ));
    } else {
        start_init(ctx, receiver, Some(raw))?;
    }
    if size <= 0 {
        return Err(Error::new(
            "ValueError",
            "buffer size must be strictly positive",
        ));
    }
    install(ctx, receiver, raw, size, true)?;
    Ok(true)
}

pub(super) fn install(
    ctx: &mut Context,
    receiver: Id,
    raw: Id,
    size: i64,
    update: bool,
) -> Result<Id> {
    let size = usize::try_from(size).map_err(|_| Error::new("ValueError", "negative count"))?;
    let bytes = crate::zeroed_io_buffer(size).map_err(|_| Error::new("MemoryError", ""))?;
    install_buffer(ctx, receiver, raw, bytes, update)
}

pub(super) fn install_buffer(
    ctx: &mut Context,
    receiver: Id,
    raw: Id,
    mut bytes: Vec<u8>,
    update: bool,
) -> Result<Id> {
    let state = Reader::new(ctx, raw, &mut bytes, update.then(Update::default))?;
    if update && Reader::get(ctx, receiver).is_ok() {
        attach(ctx, receiver, state.clone());
    }
    let absolute = invoke(ctx, raw, "tell", vec![])
        .and_then(|n| index(ctx, n))
        .ok()
        .filter(|n| *n >= 0);
    if update {
        // A nested initialization can finish inside raw.tell(). Keep its raw,
        // but discard reads and writes buffered before the outer probe returned.
        if let Ok(live) = Reader::get(ctx, receiver)
            && let Some(update) = &live.update
        {
            update.dirty.set(None);
            live.reset();
            if absolute.is_some() {
                live.absolute.set(absolute);
            }
            return Ok(NONE);
        }
        if super::descriptor_buffer::finish_random_probe(ctx, receiver) {
            return Ok(NONE);
        }
    }
    state.absolute.set(absolute);
    if update {
        state.raw.set(raw_member(ctx, receiver));
    }
    attach(ctx, receiver, state);
    Ok(NONE)
}

/// Retain the descriptor's complete read/write window without probing or reopening the raw.
pub(super) fn adopt_for_close<F>(
    ctx: &mut Context,
    receiver: Id,
    raw: Id,
    file: &mut crate::objects::Random<F>,
) -> Result<()> {
    let update = Update {
        terminal_raw_pos: Some(Cell::new(file.last_raw_pos() as i64)),
        raw_offset: Cell::new(file.raw_pos as i64),
        dirty: Cell::new(file.dirty.as_ref().map(|span| (span.start, span.end))),
        ..Update::default()
    };
    let state = Reader::new(ctx, raw, &mut file.buffer, Some(update))?;
    state.end.set(file.read_end);
    state.pos.set(file.pos);
    attach(ctx, receiver, state);
    Ok(())
}

fn attach(ctx: &mut Context, receiver: Id, state: Shared<Reader>) {
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    *memory_stream = Some(MemoryStream::Reader(state));
}

impl Reader {
    fn new(
        ctx: &mut Context,
        raw: Id,
        bytes: &mut Vec<u8>,
        update: Option<Update>,
    ) -> Result<Shared<Self>> {
        // Reserve the new owner before moving bytes that the descriptor has already accepted.
        let mut state = Shared::try_new(Self {
            raw: Cell::new(Some(raw)),
            size: bytes.len(),
            buffer: NONE,
            end: Cell::new(0),
            pos: Cell::new(0),
            absolute: Cell::new(None),
            locked: Cell::new(false),
            update,
        })
        .map_err(|_| Error::new("MemoryError", ""))?;
        Shared::get_mut(&mut state)
            .expect("fresh reader is uniquely owned")
            .buffer = ctx.alloc(Value::ByteArray(std::mem::take(bytes)));
        Ok(state)
    }

    pub(super) fn pending_update(&self) -> Option<Id> {
        self.update.as_ref()?.dirty.get()?;
        self.raw.get()
    }

    fn get(ctx: &Context, receiver: Id) -> Result<Shared<Self>> {
        match ctx.heap.get(receiver) {
            Value::Instance {
                memory_stream: Some(MemoryStream::Reader(r)),
                ..
            } => Ok(r.clone()),
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
        let unread = &data[offset..offset + self.unread()];
        let n = append_read_ahead(out, unread, limit, line)?;
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

    fn raw_closed(&self, ctx: &mut Context) -> Result<bool> {
        let raw = self.raw()?;
        if let Some(handle) = super::descriptor_buffer::native_handle(ctx, raw, &["closed"]) {
            return Ok(handle.live_fd().is_none());
        }
        let closed = ctx.lookup_attr(raw, "closed")?;
        ctx.truth(closed)
    }

    fn check_raw_open(&self, ctx: &mut Context, message: &str) -> Result<()> {
        if self.raw_closed(ctx)? {
            Err(Error::new("ValueError", message))
        } else {
            Ok(())
        }
    }

    fn check_read_open(&self, ctx: &mut Context, message: &str) -> Result<()> {
        if self.update.is_some() {
            return self.check_open(ctx, message);
        }
        if self.raw_closed(ctx)? && self.unread() == 0 {
            return Err(Error::new("ValueError", message));
        }
        Ok(())
    }

    fn with_lock<T>(
        &self,
        ctx: &mut Context,
        receiver: Id,
        work: impl FnOnce(&mut Context) -> Result<T>,
    ) -> Result<T> {
        if self.locked.replace(true) {
            let repr = ctx.builtin("repr");
            let value = ctx.call(repr, vec![receiver], Vec::new())?;
            return Err(Error::new(
                "RuntimeError",
                format!("reentrant call inside {}", ctx.str_value(value)?),
            ));
        }
        let result = work(ctx);
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
        // A builtin FileIO cannot retain the temporary view. Use its same read
        // operation directly while the live instance method is still native.
        if let Some(handle) = super::descriptor_buffer::native_handle(ctx, raw, &["readinto"]) {
            match super::file_io::read_span(ctx, &handle, backing, start..start + length) {
                // Buffered I/O retries an EINTR raised by Python. Look up the raw
                // method again because the handler could have installed an override.
                Err(error) if ctx.has_errno(&error, libc::EINTR) => {}
                result => {
                    let count = result?;
                    self.advance(count.unwrap_or(0));
                    return Ok(count);
                }
            }
        }
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

    fn read_raw(
        &self,
        ctx: &mut Context,
        amount: usize,
        reserved: Option<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>> {
        self.reset();
        let mark = ctx.heap.temporary_mark();
        if amount > crate::MAX_BYTES_LEN {
            return Err(Error::new("OverflowError", "byte string is too large"));
        }
        let data = match reserved {
            Some(data) => data,
            None => crate::zeroed_io_buffer(amount)
                .map_err(|_| Error::new("MemoryError", "bytes allocation is too large"))?,
        };
        let buffer = ctx.alloc(Value::ByteArray(data));
        let Some(n) = self.raw_into(ctx, buffer, 0, amount)? else {
            ctx.heap.release_temporaries_since(mark);
            return Ok(None);
        };
        let mut bytes = Vec::new();
        append_read_bytes(&mut bytes, &ctx.bytes_like(buffer)?[..n])?;
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

    fn read_into(&self, ctx: &mut Context, receiver: Id, name: &str, dst: Id) -> Result<Id> {
        let (target, range) = super::buffered_io::readinto_buffer(ctx, name, dst)?;
        let once = name == "readinto1";
        ctx.with_buffer_export(dst, |ctx| {
            ctx.with_buffer_export(target, |ctx| {
                self.read_into_span(ctx, receiver, target, range, once)
            })
        })
    }

    fn read_into_span(
        &self,
        ctx: &mut Context,
        receiver: Id,
        target: Id,
        range: std::ops::Range<usize>,
        once: bool,
    ) -> Result<Id> {
        let size = range.len();
        self.check_read_open(ctx, "readinto of closed file")?;
        self.with_lock(ctx, receiver, |ctx| {
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
        let mut first_raw = None;
        if name == "read" {
            if n > crate::MAX_BYTES_LEN {
                return Err(Error::new("OverflowError", "byte string is too large"));
            }
            // Reserve before consuming read-ahead or flushing a buffered write.
            let reserve =
                |size| crate::zeroed_io_buffer(size).map_err(|_| crate::Error::memory_error());
            out = reserve(n)?;
            let remaining = n.saturating_sub(self.unread());
            let direct = remaining - remaining % self.size;
            if direct == 0 {
                out.clear();
            } else if direct == n {
                first_raw = Some(std::mem::take(&mut out));
            } else {
                // Release the full reservation before acquiring the shorter raw
                // window. The caller's prefix still lives in the reader.
                out = Vec::new();
                first_raw = Some(reserve(direct)?);
            }
        }
        match name {
            "peek" => {
                self.sync_update(ctx)?;
                if self.unread() == 0 {
                    self.fill(ctx, false)?;
                }
                append_read_bytes(
                    &mut out,
                    &ctx.bytes_like(self.buffer)?[self.pos.get()..self.end.get()],
                )?;
            }
            "read1" if self.unread() == 0 && n != 0 => {
                self.sync_update(ctx)?;
                out = self.read_raw(ctx, n, None)?.unwrap_or_default();
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
                        match self.read_raw(ctx, amount, first_raw.take())? {
                            Some(bytes) if !bytes.is_empty() => {
                                append_read_bytes(&mut out, &bytes)?;
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
        if name == "read" && out.len() < n {
            crate::compact_bytes(&mut out);
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
            append_read_bytes(&mut data, bytes)?;
            ctx.heap.release_temporaries_since(mark);
            if readall.is_some() {
                break;
            }
        }
        Ok(ctx.alloc(Value::Bytes(data)))
    }

    fn seek(&self, ctx: &mut Context, receiver: Id, offset: i64, whence: i32) -> Result<Id> {
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
        self.with_lock(ctx, receiver, |ctx| {
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
            if let Some(cursor) = self
                .update
                .as_ref()
                .and_then(|u| u.terminal_raw_pos.as_ref())
            {
                cursor.set(0);
            }
            self.absolute.set(Some(pos));
            Ok(ctx.int(pos))
        })
    }
}

pub(super) fn initialized_raw(ctx: &Context, receiver: Id) -> Option<Id> {
    Reader::get(ctx, receiver).ok()?.raw.get()
}

pub(super) fn close_raw(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let reader = Reader::get(ctx, receiver)?;
    let raw = reader.raw()?;
    let result = reader.with_lock(ctx, receiver, |ctx| {
        let result = invoke(ctx, raw, "close", vec![]);
        if let Some(update) = &reader.update {
            // Retire the backing on which raw.close was called, even when it refuses.
            // The raw remains attached and determines the public closed property.
            update.retired.set(true);
            if let Some(pos) = &update.terminal_raw_pos
                && (reader.end.get() != 0 || update.dirty.get().is_some())
            {
                pos.set(update.raw_offset.get());
            }
            update.dirty.set(None);
            reader.reset();
            if let Some(pos) = &update.terminal_raw_pos {
                update.raw_offset.set(pos.get());
            }
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
        "readinto" | "readinto1" => reader.read_into(ctx, receiver, name, args[0]),
        "closed" | "name" | "mode" => ctx.lookup_attr(raw, name),
        "write" | "flush" | "truncate" if reader.update.is_some() => {
            reader.update_call(ctx, receiver, name, args)
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
            let closed = reader.with_lock(ctx, receiver, |ctx| {
                let closed = ctx.lookup_attr(raw, "closed")?;
                let closed = ctx.truth(closed)?;
                if !closed && ctx.raw_instance_field(receiver, "_finalizing") == Some(TRUE) {
                    super::buffered_io::dealloc_warn(ctx, receiver, receiver);
                }
                Ok(closed)
            })?;
            if closed {
                return Ok(NONE);
            }
            super::buffered_io::close(ctx, receiver)
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
            Reader::get(ctx, receiver)?.seek(ctx, receiver, offset, whence)
        }
        "read" | "read1" | "readline" | "peek" => {
            if name == "read" && size < -1 {
                return Err(Error::new(
                    "ValueError",
                    "read length must be non-negative or -1",
                ));
            }
            reader.check_read_open(
                ctx,
                match name {
                    "readline" => "readline of closed file",
                    "peek" => "peek of closed file",
                    _ => "read of closed file",
                },
            )?;
            // Zero-length reads can finish before entering a read-only buffer lock.
            if size == 0
                && match &reader.update {
                    None => name != "peek",
                    Some(update) => name == "read" && update.terminal_raw_pos.is_some(),
                }
            {
                return Ok(ctx.alloc(Value::Bytes(Vec::new())));
            }
            reader.with_lock(ctx, receiver, |ctx| {
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
