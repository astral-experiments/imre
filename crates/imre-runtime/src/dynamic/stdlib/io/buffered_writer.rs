//! Buffer writes to a Python raw binary stream and retain unwritten bytes across retries.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::{Context, Error, FALSE, Id, MemoryStream, NONE, Result, Value, copy_bytes, invoke};

pub(super) const METHODS: &[&str] = &[
    "__init__", "raw", "closed", "mode", "name", "flush", "close", "detach", "readable",
    "writable", "seekable", "isatty", "fileno", "seek", "tell", "write", "truncate",
];

pub(in crate::dynamic) struct Writer {
    pub(in crate::dynamic) raw: Cell<Option<Id>>,
    capacity: usize,
    pending: RefCell<Vec<u8>>,
    busy: Cell<bool>,
}

fn initialize(ctx: &mut Context, receiver: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let mut raw = args.first().copied();
    let mut size = args.get(1).copied();
    if args.len() + kwargs.len() > 2 {
        return Err(Error::new(
            "TypeError",
            "BufferedWriter() takes at most 2 arguments",
        ));
    }
    for (name, id) in kwargs {
        let slot = match name.as_str() {
            "raw" => &mut raw,
            "buffer_size" => &mut size,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("unexpected keyword argument '{name}'"),
                ));
            }
        };
        if slot.replace(*id).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("multiple values for argument '{name}'"),
            ));
        }
    }
    let raw = raw.ok_or_else(|| {
        Error::new(
            "TypeError",
            "BufferedWriter() missing required argument 'raw' (pos 1)",
        )
    })?;
    let size = match size {
        None => super::DEFAULT_BUFFER_SIZE,
        Some(size) => ctx.index_integer(size)?.to_i64().ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?,
    };
    let writable = invoke(ctx, raw, "writable", vec![])?;
    if !ctx.truth(writable)? {
        return Err(Error::new(
            "UnsupportedOperation",
            "File or stream is not writable.",
        ));
    }
    let capacity = usize::try_from(size)
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| Error::new("ValueError", "buffer size must be strictly positive"))?;
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(capacity)
        .map_err(|_| crate::Error::allocation())?;
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        return Err(Error::new(
            "TypeError",
            "expected a BufferedWriter instance",
        ));
    };
    let old = memory_stream.replace(MemoryStream::Writer(Rc::new(Writer {
        raw: Cell::new(Some(raw)),
        capacity,
        pending: RefCell::new(pending),
        busy: Cell::new(false),
    })));
    if let Some(MemoryStream::Writer(old)) = old
        && let Some(raw) = old.raw.get()
    {
        ctx.heap.weak_edge_removed(raw);
    }
    Ok(NONE)
}

fn write_raw(ctx: &mut Context, raw: Id, bytes: Vec<u8>) -> Result<Option<usize>> {
    let size = bytes.len();
    let data = ctx.alloc(Value::Bytes(bytes));
    let written = invoke(ctx, raw, "write", vec![data])?;
    if written == NONE {
        return Ok(None);
    }
    let n = ctx.index_integer(written)?.to_i64().unwrap_or(-1);
    if n <= 0 || n as usize > size {
        return Err(Error::new(
            "OSError",
            format!(
                "raw write() returned invalid length {n} (should have been between 0 and {size})"
            ),
        ));
    }
    Ok(Some(n as usize))
}

fn blocked(ctx: &mut Context, count: usize) -> Error {
    let errno = ctx.int(i64::from(libc::EAGAIN));
    let message = ctx.text("write could not complete without blocking");
    let written = ctx.int(count as i64);
    ctx.os_exception("BlockingIOError", None, &[errno, message, written], None)
        .unwrap_or_else(|e| e)
}

impl Writer {
    fn drain(&self, ctx: &mut Context, raw: Id) -> Result<bool> {
        while !self.pending.borrow().is_empty() {
            let bytes = copy_bytes(&self.pending.borrow())?;
            let Some(n) = write_raw(ctx, raw, bytes)? else {
                return Ok(false);
            };
            self.pending.borrow_mut().drain(..n);
        }
        Ok(true)
    }

    fn write(&self, ctx: &mut Context, raw: Id, data: Id) -> Result<Id> {
        let bytes = ctx.bytes_like(data)?;
        let size = bytes.len();
        if size < self.capacity && size <= self.capacity - self.pending.borrow().len() {
            self.pending.borrow_mut().extend_from_slice(bytes);
            return Ok(ctx.int(size as i64));
        }
        // Snapshot mutable input before invoking user-defined raw.write.
        let bytes = copy_bytes(bytes)?;
        let mut sent = 0;
        if self.drain(ctx, raw)? {
            while size - sent >= self.capacity {
                let Some(n) = write_raw(ctx, raw, copy_bytes(&bytes[sent..])?)? else {
                    break;
                };
                sent += n;
            }
        }
        let mut pending = self.pending.borrow_mut();
        let accepted = (size - sent).min(self.capacity - pending.len());
        pending.extend_from_slice(&bytes[sent..sent + accepted]);
        if sent + accepted < size {
            Err(blocked(ctx, sent + accepted))
        } else {
            Ok(ctx.int(size as i64))
        }
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
            format!("BufferedWriter.{name}() takes no keyword arguments"),
        ));
    }
    super::argument_count("BufferedWriter", name, args.len())?;
    if name == "readable" {
        return Ok(FALSE);
    }
    let Value::Instance {
        memory_stream: Some(MemoryStream::Writer(writer)),
        ..
    } = ctx.heap.get(receiver)
    else {
        return Err(Error::new(
            "ValueError",
            "I/O operation on uninitialized object",
        ));
    };
    let writer = Rc::clone(writer);
    if name == "raw" {
        return Ok(writer.raw.get().unwrap_or(NONE));
    }
    let raw = writer
        .raw
        .get()
        .ok_or_else(|| Error::new("ValueError", "raw stream has been detached"))?;
    // Python overrides can detach or reinitialize the receiver inside any callback.
    ctx.heap.keep(raw);
    if matches!(name, "closed" | "name" | "mode") {
        return ctx.lookup_attr(raw, name);
    }
    if name == "__exit__" {
        return invoke(ctx, receiver, "close", vec![]);
    }
    let closed = ctx.lookup_attr(raw, "closed")?;
    if ctx.truth(closed)? {
        return if name == "close" {
            Ok(NONE)
        } else {
            Err(Error::new(
                "ValueError",
                match name {
                    "write" => "write to closed file",
                    "detach" | "flush" => "flush of closed file",
                    "truncate" => "truncate of closed file",
                    "seek" => "seek of closed file",
                    _ => "I/O operation on closed file.",
                },
            ))
        };
    }
    match name {
        "__enter__" => return Ok(receiver),
        "writable" | "seekable" | "isatty" | "fileno" => return invoke(ctx, raw, name, vec![]),
        "tell" => {
            let pos = invoke(ctx, raw, "tell", vec![])?;
            let pos = ctx.as_integer(pos)?;
            let pos = pos
                .checked_add(writer.pending.borrow().len() as i64)
                .ok_or_else(|| Error::new("OverflowError", "file position out of range"))?;
            return Ok(ctx.int(pos));
        }
        "close" => {
            let flushed = invoke(ctx, receiver, "flush", vec![]);
            let closed = invoke(ctx, raw, "close", vec![])?;
            return flushed.map(|_| closed);
        }
        "detach" => {
            invoke(ctx, receiver, "flush", vec![])?;
            writer.raw.set(None);
            return Ok(raw);
        }
        _ => {}
    }
    if writer.busy.replace(true) {
        return Err(Error::new(
            "RuntimeError",
            "reentrant call inside buffered writer",
        ));
    }
    let result = (|| {
        if name == "write" {
            return writer.write(ctx, raw, args[0]);
        }
        if !writer.drain(ctx, raw)? {
            return Err(blocked(ctx, 0));
        }
        match name {
            "flush" => Ok(NONE),
            "seek" | "truncate" => invoke(ctx, raw, name, args.to_vec()),
            _ => unreachable!("registered BufferedWriter method"),
        }
    })();
    writer.busy.set(false);
    result
}
