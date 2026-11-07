//! BufferedWriter over a Python raw stream. No Python callback sees a borrowed byte slice.

use std::cell::{Cell, RefCell};
use std::io::{self, Write};
use std::rc::Rc;

use crate::dynamic::value::Heap;
use crate::{BinaryOutput, BlockedWrite};

use super::super::files;
use super::bytes_io::{BytesIo, copy_bytes};
use super::text_wrapper::invoke;
use super::{Context, Error, Id, MemoryStream, NONE, Result, TRUE, Value};

pub(in crate::dynamic) struct Writer {
    pub(in crate::dynamic) raw: Cell<Option<Id>>,
    pub(super) active: super::native_writer::ActiveCall,
    pub(super) ready: Cell<bool>,
    capacity: Cell<usize>,
    pending: RefCell<Vec<u8>>,
    written: Cell<usize>,
    unflushed: Cell<usize>,
    busy: Cell<bool>,
}

pub(super) fn state(ctx: &Context, receiver: Id) -> Option<Rc<Writer>> {
    match ctx.heap.get(receiver) {
        Value::Instance {
            memory_stream: Some(MemoryStream::BufferedBytes(writer)),
            ..
        } => Some(writer.clone()),
        _ => None,
    }
}

/// Inspect pending output without resolving Python attributes during shutdown.
pub(in crate::dynamic) fn pending_callback_write(heap: &Heap, value: &Value) -> bool {
    let Value::Instance {
        memory_stream: Some(stream),
        ..
    } = value
    else {
        return false;
    };
    let raw = match stream {
        MemoryStream::Reader(reader) => reader.pending_update(),
        MemoryStream::BufferedBytes(writer) if writer.unflushed.get() != 0 => writer.raw.get(),
        _ => None,
    };
    raw.is_some_and(|raw| native_writer(heap, raw).is_none())
}

/// Bypass Python only on an exact BytesIO whose write method is still native.
fn native_writer(heap: &Heap, raw: Id) -> Option<Rc<BytesIo>> {
    if let Value::Instance {
        class,
        memory_stream: Some(MemoryStream::Bytes(stream)),
        ..
    } = heap.get(raw)
        && heap.get(*class).io_kind() == Some("BytesIO")
        && heap.raw_instance_field(raw, "write").is_none()
    {
        return Some(stream.clone());
    }
    None
}

pub(super) fn initialize(
    ctx: &mut Context,
    receiver: Id,
    raw: Id,
    capacity: usize,
    revived: bool,
) -> Result<Id> {
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(capacity)
        .map_err(|_| crate::Error::allocation())?;
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    *memory_stream = Some(MemoryStream::BufferedBytes(Rc::new(Writer {
        active: super::native_writer::ActiveCall::default(),
        ready: Cell::new(revived),
        raw: Cell::new(Some(raw)),
        capacity: Cell::new(capacity),
        pending: RefCell::new(pending),
        written: Cell::new(0),
        unflushed: Cell::new(0),
        busy: Cell::new(false),
    })));
    // Both tell() and conversion of its result may recursively initialize this same writer.
    let _ = invoke(ctx, raw, "tell", vec![]).and_then(|value| position(ctx, value));
    super::descriptor_buffer::finish_writer_probe(ctx, receiver)?;
    Ok(NONE)
}

/// Adapt a raw stream to BinaryOutput, carrying Python exceptions through std::io::Write.
struct RawWriter<'a> {
    ctx: &'a mut Context,
    raw: Id,
    unflushed: &'a Cell<usize>,
    writing: bool,
    error: Option<Error>,
}

pub(super) fn raw_write(ctx: &mut Context, raw: Id, data: &[u8]) -> Result<Option<usize>> {
    with_write_temporaries(ctx, |ctx| RawWriter::call_raw(ctx, raw, data))
}

/// Drain a live output window. Commit each returned count before delivering pending signals.
pub(super) fn flush_window(
    ctx: &mut Context,
    raw: Id,
    buffer: Id,
    range: std::ops::Range<usize>,
    mut advanced: impl FnMut(usize, usize),
) -> Result<()> {
    let mut start = range.start;
    while start < range.end {
        let len = range.end - start;
        let count = with_write_temporaries(ctx, |ctx| {
            let view = ctx.ownerless_byte_view(buffer, start, len, true)?;
            match RawWriter::call_view(ctx, raw, view, len)? {
                RawCount::Written(count) => Ok(count),
                RawCount::WouldBlock(errno) => {
                    Err(super::descriptor_buffer::blocking_error(ctx, errno, 0))
                }
            }
        })?;
        start += count;
        advanced(start, count);
        ctx.dispatch_signals()?;
    }
    Ok(())
}

fn with_write_temporaries<T>(
    ctx: &mut Context,
    write: impl FnOnce(&mut Context) -> Result<T>,
) -> Result<T> {
    let mark = ctx.heap.temporary_mark();
    let result = write(ctx);
    ctx.heap.release_temporaries_since(mark);
    if let Err(error) = &result {
        for id in error.roots() {
            ctx.heap.keep(id);
        }
    }
    result
}

enum RawCount {
    Written(usize),
    WouldBlock(i32),
}

impl RawWriter<'_> {
    fn call_raw(ctx: &mut Context, raw: Id, data: &[u8]) -> Result<Option<usize>> {
        if let Some(stream) = native_writer(&ctx.heap, raw) {
            stream.write(ctx, data)?;
            return Ok(Some(data.len()));
        }
        let bytes = ctx.alloc(Value::Bytes(copy_bytes(data)?));
        let view = ctx.alloc(Value::MemoryView {
            owner: Some(bytes),
            lease: None,
            anonymous: true,
            restricted: false,
            layout: None,
            readonly: false,
            hash: None,
        });
        Ok(match Self::call_view(ctx, raw, view, data.len())? {
            RawCount::Written(count) => Some(count),
            RawCount::WouldBlock(_) => None,
        })
    }

    fn call_view(ctx: &mut Context, raw: Id, view: Id, len: usize) -> Result<RawCount> {
        // Attribute lookup itself can invoke Python and collect temporaries.
        ctx.heap.keep(view);
        let retry_mark = ctx.heap.temporary_mark();
        let (result, raw_errno) = loop {
            // A raw returning None can leave a syscall errno, including EAGAIN.
            errno::set_errno(errno::Errno(0));
            let mut raw_errno = 0;
            let result = super::text_wrapper::invoke_with(ctx, raw, "write", |ctx, write| {
                let result = ctx.call(write, vec![view], Vec::new());
                raw_errno = errno::errno().0;
                result
            });
            match result {
                Err(error) if ctx.has_errno(&error, libc::EINTR) => {
                    ctx.heap.release_temporaries_since(retry_mark);
                    continue;
                }
                result => break (result?, raw_errno),
            }
        };
        if result == NONE {
            return Ok(RawCount::WouldBlock(raw_errno));
        }
        let count = ctx
            .index_integer(result)
            .ok()
            .and_then(|n| n.to_i64())
            .unwrap_or(-1);
        if count < 0 || count as u64 > len as u64 {
            return Err(Error::new(
                "OSError",
                format!(
                    "raw write() returned invalid length {count} (should have been between 0 and {len})"
                ),
            ));
        }
        Ok(RawCount::Written(count as usize))
    }
}

impl Write for RawWriter<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        match raw_write(self.ctx, self.raw, data) {
            // CPython retries a zero result without consuming any input.
            Ok(Some(0)) => Err(io::ErrorKind::Interrupted.into()),
            Ok(Some(count)) => {
                self.unflushed
                    .set(self.unflushed.get().saturating_sub(count));
                Ok(count)
            }
            Ok(None) => Err(io::ErrorKind::WouldBlock.into()),
            Err(error) => {
                if self.writing && self.unflushed.get() != 0 {
                    // A blocked preflush can still accept new input into the buffer.
                    // A direct raw write or explicit flush propagates the original error.
                    let blocking = self.ctx.builtin("BlockingIOError");
                    if self.ctx.raised_error_matches(&error, blocking) {
                        return Err(io::ErrorKind::WouldBlock.into());
                    }
                }
                self.error = Some(error);
                Err(io::ErrorKind::Other.into())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        // BufferedWriter.flush drains pending writes without calling raw.flush().
        Ok(())
    }
}

struct Lock<'a>(&'a Cell<bool>);

impl Drop for Lock<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(super) fn position(ctx: &mut Context, result: Id) -> Result<i64> {
    let pos = ctx.index_integer(result)?.to_i64().ok_or_else(|| {
        Error::new(
            "ValueError",
            "cannot fit 'int' into an offset-sized integer",
        )
    })?;
    if pos < 0 {
        return Err(Error::new(
            "OSError",
            format!("Raw stream returned invalid position {pos}"),
        ));
    }
    Ok(pos)
}

impl Writer {
    pub(super) fn finish_probe(&self) -> Result<()> {
        self.raw()?;
        self.pending.borrow_mut().clear();
        self.unflushed.set(0);
        self.written.set(0);
        self.ready.set(true);
        Ok(())
    }

    fn raw(&self) -> Result<Id> {
        self.raw
            .get()
            .ok_or_else(|| Error::new("ValueError", "raw stream has been detached"))
    }

    fn lock(&self) -> Result<Lock<'_>> {
        if self.busy.replace(true) {
            Err(Error::new(
                "RuntimeError",
                "reentrant call inside <_io.BufferedWriter>",
            ))
        } else {
            Ok(Lock(&self.busy))
        }
    }

    /// Take the buffer out before callbacks so reentry cannot alias Rust storage.
    fn transfer(&self, ctx: &mut Context, raw: Id, data: Option<Id>) -> Result<Id> {
        let bytes = data.map(|id| files::output_bytes(ctx, id)).transpose()?;
        if let Some(bytes) = bytes
            && bytes.len() < self.capacity.get()
            && bytes.len() <= self.capacity.get() - self.pending.borrow().len()
        {
            self.pending.borrow_mut().extend_from_slice(bytes);
            self.unflushed.set(self.unflushed.get() + bytes.len());
            return Ok(ctx.int(bytes.len() as i64));
        }
        let refresh = match (data, bytes) {
            (Some(id), Some(b))
                if b.len() < self.capacity.get()
                    && matches!(
                        ctx.buffer_storage(id)?,
                        Value::ByteArray(_)
                            | Value::Array(_)
                            | Value::BytesIoStorage(_)
                            | Value::RawReadWindow(_)
                    ) =>
            {
                Some(id)
            }
            _ => None,
        };
        let bytes = bytes.map(copy_bytes).transpose()?;
        let length = bytes.as_ref().map(Vec::len);
        let mut output = BinaryOutput {
            buffer: self.pending.take(),
            written: self.written.get(),
            inner: RawWriter {
                ctx,
                raw,
                unflushed: &self.unflushed,
                writing: data.is_some(),
                error: None,
            },
        };
        let result = match bytes {
            Some(data) => output.write_all(&data),
            None => output.flush(),
        };
        self.unflushed.set(output.buffer.len() - output.written);
        self.written.set(output.written);
        self.pending.replace(output.buffer);
        if let Some(error) = output.inner.error {
            return Err(error);
        }
        let blocked = result
            .as_ref()
            .err()
            .and_then(|error| error.get_ref())
            .and_then(|e| e.downcast_ref::<BlockedWrite>())
            .map(|e| e.accepted);
        // A preflush may have changed a mutable input in-place. Short writes
        // only place new bytes into our buffer; copy the accepted prefix now.
        if let Some(input) = refresh {
            let count = if result.is_ok() {
                length.unwrap()
            } else {
                blocked.unwrap_or(0)
            };
            let mut pending = self.pending.borrow_mut();
            let start = pending.len() - count;
            pending[start..].copy_from_slice(&files::output_bytes(ctx, input)?[..count]);
        }
        if let Some(accepted) = blocked {
            // A raw callback returning None need not have set errno.
            return Err(super::descriptor_buffer::blocking_error(ctx, 0, accepted));
        }
        result.map_err(super::super::os_error)?;
        Ok(length.map_or(NONE, |n| ctx.int(n as i64)))
    }

    pub(super) fn call(
        &self,
        ctx: &mut Context,
        receiver: Id,
        name: &str,
        args: &[Id],
    ) -> Result<Id> {
        if name == "raw" {
            return Ok(self.raw.get().unwrap_or(NONE));
        }
        if !self.ready.get() {
            return Err(Error::new(
                "ValueError",
                "I/O operation on uninitialized object",
            ));
        }
        let whence = if name == "seek" && args.len() == 2 {
            ctx.index_integer(args[1])?
                .to_i64()
                .and_then(|i| i32::try_from(i).ok())
                .ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C int")
                })?
        } else {
            0
        };
        let raw = self.raw()?;
        ctx.heap.keep(raw);
        if name == "seek"
            && !matches!(
                whence,
                libc::SEEK_SET
                    | libc::SEEK_CUR
                    | libc::SEEK_END
                    | libc::SEEK_DATA
                    | libc::SEEK_HOLE
            )
        {
            return Err(Error::new(
                "ValueError",
                format!("whence value {whence} unsupported"),
            ));
        }

        if matches!(name, "closed" | "name" | "mode") {
            return ctx.lookup_attr(raw, name);
        }
        if name == "fileno" {
            return match ctx.lookup_attr(raw, name) {
                Ok(method) => ctx.call(method, vec![], Vec::new()),
                Err(error) if error.kind == "AttributeError" => {
                    Err(Error::new("UnsupportedOperation", "fileno"))
                }
                Err(error) => Err(error),
            };
        }
        if matches!(name, "writable" | "seekable" | "isatty") {
            return invoke(ctx, raw, name, vec![]);
        }
        if name == "tell" {
            let value = invoke(ctx, raw, "tell", vec![])?;
            let pos = position(ctx, value)?
                .checked_add(self.unflushed.get() as i64)
                .ok_or_else(|| Error::new("OverflowError", "file position out of range"))?;
            return Ok(ctx.int(pos));
        }
        if name == "detach" {
            invoke(ctx, receiver, "flush", vec![])?;
            let _guard = self.lock()?;
            let raw = self.raw()?;
            self.raw.set(None);
            ctx.heap.weak_edge_removed(raw);
            return Ok(raw);
        }
        if name == "write"
            && self.capacity.get() != 0
            && files::output_bytes(ctx, args[0])?.len() >= self.capacity.get()
            && matches!(
                ctx.buffer_storage(args[0])?,
                Value::ByteArray(_)
                    | Value::Array(_)
                    | Value::BytesIoStorage(_)
                    | Value::RawReadWindow(_)
            )
            && native_writer(&ctx.heap, raw).is_none()
        {
            // A direct raw.write view must observe in-place mutations to the input.
            // Retaining a Rust borrow across a Python callback cannot implement it.
            return Err(Error::new(
                "NotImplementedError",
                "direct buffered writes from a mutable exporter to Python raw methods are unsupported",
            ));
        }
        let guard = if matches!(name, "write" | "close") {
            Some(self.lock()?)
        } else {
            None
        };
        // close() can retire the buffer even if raw.close() leaves the raw open.
        let retired =
            self.capacity.get() == 0 && matches!(name, "flush" | "write" | "seek" | "truncate");
        if retired || {
            let closed = ctx.lookup_attr(raw, "closed")?;
            ctx.truth(closed)?
        } {
            if name == "close" {
                return Ok(NONE);
            }
            return Err(Error::new(
                "ValueError",
                match name {
                    "write" => "write to closed file",
                    "flush" | "detach" => "flush of closed file",
                    "seek" => "seek of closed file",
                    "truncate" => "truncate of closed file",
                    _ => "I/O operation on closed file.",
                },
            ));
        }
        match name {
            "__enter__" => Ok(receiver),
            "write" => self.transfer(ctx, self.raw()?, Some(args[0])),
            "flush" => {
                let _guard = self.lock()?;
                self.transfer(ctx, self.raw()?, None)
            }
            "close" => {
                if ctx.raw_instance_field(receiver, "_finalizing") == Some(TRUE) {
                    super::buffered_io::dealloc_warn(ctx, receiver, receiver);
                }
                // Public flush takes the lock itself and can be overridden.
                drop(guard);
                super::buffered_io::flush_and_close(ctx, receiver, |ctx| {
                    let _guard = self.lock()?;
                    let raw = self.raw()?;
                    let closed = invoke(ctx, raw, "close", vec![]);
                    self.pending.take();
                    self.capacity.set(0);
                    self.unflushed.set(0);
                    self.written.set(0);
                    closed
                })
            }
            "seek" => {
                if invoke(ctx, raw, "seekable", vec![])? != TRUE {
                    return Err(Error::new(
                        "UnsupportedOperation",
                        "File or stream is not seekable.",
                    ));
                }
                let offset = ctx.index_integer(args[0])?.to_i64().ok_or_else(|| {
                    Error::new(
                        "ValueError",
                        "cannot fit 'int' into an offset-sized integer",
                    )
                })?;
                let _guard = self.lock()?;
                let raw = self.raw()?;
                self.transfer(ctx, raw, None)?;
                let args = vec![ctx.int(offset), ctx.int(i64::from(whence))];
                let value = invoke(ctx, raw, "seek", args)?;
                let pos = position(ctx, value)?;
                Ok(ctx.int(pos))
            }
            "truncate" => {
                let _guard = self.lock()?;
                let raw = self.raw()?;
                self.transfer(ctx, raw, None)?;
                let value = invoke(ctx, raw, name, vec![args.first().copied().unwrap_or(NONE)])?;
                let _ = invoke(ctx, raw, "tell", vec![]);
                Ok(value)
            }
            _ => unreachable!("registered BufferedWriter method"),
        }
    }
}
