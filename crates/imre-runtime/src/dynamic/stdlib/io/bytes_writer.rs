//! BufferedWriter over an exact native BytesIO. No Python callback sees a borrowed byte slice.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::super::files;
use super::bytes_io::BytesIo;
use super::text_wrapper::invoke;
use super::{Context, Error, Id, MemoryStream, NONE, Result, TRUE, Value};

pub(in crate::dynamic) struct Writer {
    pub(in crate::dynamic) raw: Cell<Option<Id>>,
    capacity: usize,
    pending: RefCell<Vec<u8>>,
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

pub(super) fn is_bytes_io(ctx: &Context, raw: Id) -> bool {
    ctx.heap
        .get(raw)
        .instance_class()
        .and_then(|class| ctx.heap.get(class).io_kind())
        == Some("BytesIO")
}

fn native_writer(ctx: &Context, raw: Id) -> Result<Rc<BytesIo>> {
    if ctx.raw_instance_field(raw, "write").is_none()
        && let Value::Instance {
            memory_stream: Some(MemoryStream::Bytes(stream)),
            ..
        } = ctx.heap.get(raw)
    {
        return Ok(stream.clone());
    }
    Err(Error::new(
        "NotImplementedError",
        "BufferedWriter with Python raw write methods is unsupported",
    ))
}

pub(super) fn initialize(ctx: &mut Context, receiver: Id, raw: Id, capacity: usize) -> Result<Id> {
    native_writer(ctx, raw)?;
    // CPython probes the initial position, including when the stream is not seekable.
    let _ = invoke(ctx, raw, "tell", vec![]);
    native_writer(ctx, raw)?;
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        return Err(Error::new(
            "NotImplementedError",
            "BufferedWriter reinitialization is unsupported",
        ));
    };
    if memory_stream.is_some() {
        return Err(Error::new(
            "NotImplementedError",
            "BufferedWriter reinitialization is unsupported",
        ));
    }
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(capacity)
        .map_err(|_| crate::Error::allocation())?;
    *memory_stream = Some(MemoryStream::BufferedBytes(Rc::new(Writer {
        raw: Cell::new(Some(raw)),
        capacity,
        pending: RefCell::new(pending),
    })));
    Ok(NONE)
}

/// Python seek/truncate overrides could reenter while CPython holds its buffer lock.
fn require_native_method(ctx: &Context, raw: Id, name: &str) -> Result<()> {
    if ctx.raw_instance_field(raw, name).is_some() {
        return Err(Error::new(
            "NotImplementedError",
            format!("BufferedWriter with a Python raw {name} method is unsupported"),
        ));
    }
    Ok(())
}

fn position(ctx: &mut Context, result: Id) -> Result<i64> {
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
    fn raw(&self) -> Result<Id> {
        self.raw
            .get()
            .ok_or_else(|| Error::new("ValueError", "raw stream has been detached"))
    }

    fn drain(&self, ctx: &Context, raw: Id) -> Result<()> {
        let mut pending = self.pending.borrow_mut();
        if !pending.is_empty() {
            native_writer(ctx, raw)?.write(&pending)?;
            pending.clear();
        }
        Ok(())
    }

    fn write(&self, ctx: &mut Context, raw: Id, data: Id) -> Result<Id> {
        let stream = native_writer(ctx, raw)?;
        let bytes = files::output_bytes(ctx, data)?;
        let n = bytes.len();
        if n >= self.capacity || n > self.capacity - self.pending.borrow().len() {
            self.drain(ctx, raw)?;
        }
        if n >= self.capacity {
            stream.write(bytes)?;
        } else {
            self.pending.borrow_mut().extend_from_slice(bytes);
        }
        Ok(ctx.int(n as i64))
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
        let closed = ctx.lookup_attr(raw, "closed")?;
        if ctx.truth(closed)? {
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
            "write" => self.write(ctx, raw, args[0]),
            "flush" => {
                self.drain(ctx, raw)?;
                Ok(NONE)
            }
            "close" => {
                let flushed = invoke(ctx, receiver, "flush", vec![]);
                // invoke retains the exception roots across raw.close().
                match (flushed, invoke(ctx, self.raw()?, "close", vec![])) {
                    (Err(flush), Err(mut close)) => {
                        close.context = ctx.raised_instance(flush);
                        Err(close)
                    }
                    (Err(flush), _) => Err(flush),
                    (_, result) => result,
                }
            }
            "detach" => {
                invoke(ctx, receiver, "flush", vec![])?;
                let raw = self.raw()?;
                self.raw.set(None);
                ctx.heap.weak_edge_removed(raw);
                Ok(raw)
            }
            "tell" => {
                let value = invoke(ctx, raw, "tell", vec![])?;
                let pos = position(ctx, value)?
                    .checked_add(self.pending.borrow().len() as i64)
                    .ok_or_else(|| Error::new("OverflowError", "file position out of range"))?;
                Ok(ctx.int(pos))
            }
            "seek" => {
                if !matches!(
                    whence,
                    libc::SEEK_SET
                        | libc::SEEK_CUR
                        | libc::SEEK_END
                        | libc::SEEK_DATA
                        | libc::SEEK_HOLE
                ) {
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
                let offset = ctx.index_integer(args[0])?.to_i64().ok_or_else(|| {
                    Error::new(
                        "ValueError",
                        "cannot fit 'int' into an offset-sized integer",
                    )
                })?;
                let raw = self.raw()?;
                require_native_method(ctx, raw, name)?;
                self.drain(ctx, raw)?;
                let args = vec![ctx.int(offset), ctx.int(i64::from(whence))];
                let value = invoke(ctx, raw, "seek", args)?;
                let pos = position(ctx, value)?;
                Ok(ctx.int(pos))
            }
            "truncate" => {
                require_native_method(ctx, raw, name)?;
                self.drain(ctx, raw)?;
                invoke(ctx, raw, name, args.to_vec())
            }
            "writable" | "seekable" | "isatty" => invoke(ctx, raw, name, vec![]),
            _ => unreachable!("registered BufferedWriter method"),
        }
    }
}
