//! Linux scatter reads and gather writes over exported buffers.

use super::{Context, Error, Id, Result, Value};
use std::{
    collections::{HashMap, TryReserveError, hash_map::Entry},
    ops::Range,
};

fn no_memory(_: TryReserveError) -> Error {
    Error::new("MemoryError", "")
}

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
        ) || ctx.slot(id, "__getitem__").is_some())
}

fn buffer_span(ctx: &Context, id: Id, writable: bool) -> Result<(Id, Range<usize>)> {
    if let Some(span) = ctx.contiguous_byte_span(id, writable)? {
        return Ok(span);
    }
    match ctx.heap.get(ctx.scalar_value(id)) {
        Value::Bytes(_) => Err(Error::new("BufferError", "Object is not writable.")),
        Value::MemoryView { .. } => {
            let reason = if writable && ctx.view_is_readonly(id)? {
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

// The large-file interface keeps the Python offset intact on 32-bit GNU targets.
type Preadv2 = unsafe extern "C" fn(i32, *const libc::iovec, i32, i64, i32) -> isize;

#[cfg(not(target_env = "gnu"))]
use libc::{off_t as FileOffset, preadv as read_at};
#[cfg(target_env = "gnu")]
use libc::{off64_t as FileOffset, preadv64 as read_at};

fn preadv2() -> Option<Preadv2> {
    static CALL: std::sync::OnceLock<Option<Preadv2>> = std::sync::OnceLock::new();
    *CALL.get_or_init(|| {
        // glibc added preadv64v2 in 2.26. Other Linux libcs may provide preadv2
        // with a 64-bit off_t, or only the original flags-free preadv.
        #[cfg(target_env = "gnu")]
        let name = c"preadv64v2";
        #[cfg(not(target_env = "gnu"))]
        let name = {
            if size_of::<libc::off_t>() != size_of::<i64>() {
                return None;
            }
            c"preadv2"
        };
        // SAFETY: both names above have the declared C ABI with a 64-bit offset.
        let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
        if symbol.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute::<*mut libc::c_void, Preadv2>(symbol) })
        }
    })
}

enum Transfer {
    Read,
    Write,
    Positioned(FileOffset),
    PositionedWithFlags(Preadv2, i64, i32),
}

fn transfer_exported(
    ctx: &mut Context,
    fd: i32,
    spans: &[(Id, Range<usize>)],
    transfer: Transfer,
) -> Result<Id> {
    let mut vectors = Vec::new();
    vectors.try_reserve_exact(spans.len()).map_err(no_memory)?;
    let mut bases = HashMap::new();
    bases.try_reserve(spans.len()).map_err(no_memory)?;
    loop {
        // Get one pointer per allocation; multiple iovecs may overlap in that storage.
        // A signal handler can mutate buffers between attempts, so reacquire the bases.
        bases.clear();
        vectors.clear();
        for (storage, range) in spans {
            let (base, length) = match bases.entry(*storage) {
                Entry::Occupied(entry) => *entry.get(),
                Entry::Vacant(entry) => {
                    let backing = if matches!(transfer, Transfer::Write) {
                        let bytes = ctx.owner_bytes(*storage)?;
                        (bytes.as_ptr().cast_mut(), bytes.len())
                    } else {
                        let bytes = ctx.writable_storage_bytes(*storage)?;
                        (bytes.as_mut_ptr(), bytes.len())
                    };
                    *entry.insert(backing)
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
        let iov = vectors.as_ptr();
        let count = vectors.len() as libc::c_int;
        // SAFETY: each iovec addresses live exported storage until the syscall returns.
        // No Rust slice into those buffers or Python callback is live during the call.
        let n = unsafe {
            match transfer {
                Transfer::Read => libc::readv(fd, iov, count),
                // writev only reads iov_base, including for immutable bytes.
                Transfer::Write => libc::writev(fd, iov, count),
                Transfer::Positioned(offset) => read_at(fd, iov, count, offset),
                Transfer::PositionedWithFlags(call, offset, flags) => {
                    call(fd, iov, count, offset, flags)
                }
            }
        };
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

pub(super) fn readv(
    ctx: &mut Context,
    fd: i32,
    buffers: Id,
    position: Option<(i64, i32)>,
) -> Result<Id> {
    let name = if position.is_some() {
        "preadv2"
    } else {
        "readv"
    };
    with_vectors(ctx, fd, buffers, name, || {
        Ok(match position {
            None => Transfer::Read,
            Some((offset, flags)) => match preadv2() {
                Some(call) => Transfer::PositionedWithFlags(call, offset, flags),
                None if flags == 0 => Transfer::Positioned(
                    FileOffset::try_from(offset)
                        .map_err(|_| Error::new("OverflowError", "offset does not fit in off_t"))?,
                ),
                None => {
                    return Err(Error::new(
                        "NotImplementedError",
                        "preadv2: flags unavailable on this platform",
                    ));
                }
            },
        })
    })
}

pub(super) fn writev(ctx: &mut Context, fd: i32, buffers: Id) -> Result<Id> {
    crate::initialize_io();
    with_vectors(ctx, fd, buffers, "writev", || Ok(Transfer::Write))
}

fn with_vectors(
    ctx: &mut Context,
    fd: i32,
    buffers: Id,
    name: &str,
    select: impl FnOnce() -> Result<Transfer>,
) -> Result<Id> {
    if !is_sequence(ctx, buffers) {
        return Err(Error::new(
            "TypeError",
            format!("{name}() arg 2 must be a sequence"),
        ));
    }
    let count = ctx.sequence_len(buffers)?;
    let transfer = select()?;
    let writable = !matches!(transfer, Transfer::Write);
    let mut spans = Vec::new();
    spans.try_reserve_exact(count).map_err(no_memory)?;
    let mark = ctx.buffer_exports.len();
    let result = (|| {
        for index in 0..count {
            let key = ctx.int(index as i64);
            let item = ctx.get_item(buffers, key)?;
            let (storage, range) = buffer_span(ctx, item, writable)?;
            // Pin this view and backing before the next __getitem__ runs Python.
            ctx.buffer_exports.try_reserve(2).map_err(no_memory)?;
            ctx.buffer_exports.extend([item, storage]);
            spans.push((storage, range));
        }
        transfer_exported(ctx, fd, &spans, transfer)
    })();
    ctx.buffer_exports.truncate(mark);
    result
}
