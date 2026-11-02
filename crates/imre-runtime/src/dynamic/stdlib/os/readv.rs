//! Read into a sequence of exported buffers in one Linux scatter-read operation.

use super::{Context, Error, Id, Result, Value};
use std::{collections::HashMap, ops::Range};

fn is_sequence(ctx: &Context, id: Id) -> bool {
    // PySequence_Check excludes dict subclasses even if they override __getitem__.
    ctx.dict_storage(id).is_none()
        && (matches!(
            ctx.heap.get(ctx.scalar_value(id)),
            Value::List(_)
                | Value::Tuple(_)
                | Value::NamedTuple { .. }
                | Value::Range(_)
                | Value::Deque(_)
                | Value::Str(_)
                | Value::SurrogateStr(_)
                | Value::Bytes(_)
                | Value::ByteArray(_)
                | Value::Array(_)
                | Value::MemoryView { .. }
        ) || ctx
            .heap
            .get(id)
            .instance_class()
            .is_some_and(|class| ctx.class_attr(class, "__getitem__").is_some()))
}

fn writable_span(ctx: &Context, id: Id) -> Result<(Id, Range<usize>)> {
    if let Some(span) = ctx.writable_byte_span(id)? {
        return Ok(span);
    }
    match ctx.heap.get(ctx.scalar_value(id)) {
        Value::Bytes(_) => Err(Error::new("BufferError", "Object is not writable.")),
        Value::MemoryView { .. } => {
            let reason = if ctx.view_is_readonly(id)? {
                "memoryview: underlying buffer is not writable"
            } else {
                "memoryview: underlying buffer is not C-contiguous"
            };
            Err(Error::new("BufferError", reason))
        }
        _ => Err(Error::new(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                ctx.type_name(id)
            ),
        )),
    }
}

fn read_exported(ctx: &mut Context, fd: i32, spans: &[(Id, Range<usize>)]) -> Result<Id> {
    let mut vectors = Vec::with_capacity(spans.len());
    let mut bases = HashMap::new();
    loop {
        // Get one pointer per allocation; multiple iovecs may overlap in that storage.
        // A signal handler can mutate buffers between attempts, so reacquire the bases.
        bases.clear();
        vectors.clear();
        for (storage, range) in spans {
            let (base, length) = match bases.entry(*storage) {
                std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let bytes = ctx.writable_storage_bytes(*storage)?;
                    *entry.insert((bytes.as_mut_ptr(), bytes.len()))
                }
            };
            if range.end > length {
                return Err(Error::new(
                    "BufferError",
                    "underlying buffer has changed size",
                ));
            }
            vectors.push(libc::iovec {
                iov_base: base.wrapping_add(range.start).cast(),
                iov_len: range.len(),
            });
        }
        // SAFETY: each range addresses live exported storage until this call returns.
        // No Rust slice into those buffers or Python callback is live during the syscall.
        let n = unsafe { libc::readv(fd, vectors.as_ptr(), vectors.len() as libc::c_int) };
        if n >= 0 {
            return Ok(ctx.int(n as i64));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(super::super::os_error(error));
        }
        ctx.dispatch_signals()?;
    }
}

pub(super) fn readv(ctx: &mut Context, fd: i32, buffers: Id) -> Result<Id> {
    if !is_sequence(ctx, buffers) {
        return Err(Error::new("TypeError", "readv() arg 2 must be a sequence"));
    }
    let count = ctx.sequence_len(buffers)?;
    let mut spans = Vec::new();
    spans
        .try_reserve_exact(count)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let mark = ctx.buffer_exports.len();
    let result = (|| {
        for index in 0..count {
            let key = ctx.int(index as i64);
            let item = ctx.get_item(buffers, key)?;
            let (storage, range) = writable_span(ctx, item)?;
            // Pin this view and backing before the next __getitem__ runs Python.
            ctx.buffer_exports.extend([item, storage]);
            spans.push((storage, range));
        }
        read_exported(ctx, fd, &spans)
    })();
    ctx.buffer_exports.truncate(mark);
    result
}
