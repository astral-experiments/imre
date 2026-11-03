//! Linux scatter reads and gather writes over exported buffers.

use super::{Context, Error, Id, Result, Value, is_sequence};
use std::{
    collections::{HashMap, TryReserveError, hash_map::Entry},
    ops::Range,
};

fn no_memory(_: TryReserveError) -> Error {
    Error::new("MemoryError", "")
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

// The large-file interfaces keep the Python offset intact on 32-bit GNU targets.
#[cfg(not(target_env = "gnu"))]
use libc::{off_t as FileOffset, preadv as read_at, pwritev as write_at};
#[cfg(target_env = "gnu")]
use libc::{off64_t as FileOffset, preadv64 as read_at, pwritev64 as write_at};

type Positioned = unsafe extern "C" fn(i32, *const libc::iovec, i32, FileOffset) -> isize;
type PositionedV2 = unsafe extern "C" fn(i32, *const libc::iovec, i32, i64, i32) -> isize;

fn positioned_v2(write: bool) -> Option<PositionedV2> {
    use std::sync::OnceLock;
    static READ: OnceLock<Option<PositionedV2>> = OnceLock::new();
    static WRITE: OnceLock<Option<PositionedV2>> = OnceLock::new();
    let (cache, large_file, regular) = if write {
        (&WRITE, c"pwritev64v2", c"pwritev2")
    } else {
        (&READ, c"preadv64v2", c"preadv2")
    };
    *cache.get_or_init(|| {
        // glibc added the v2 large-file interfaces in 2.26. Other Linux libcs
        // may provide the regular v2 symbols with a 64-bit off_t.
        let name = if cfg!(target_env = "gnu") {
            large_file
        } else if size_of::<libc::off_t>() == size_of::<i64>() {
            regular
        } else {
            return None;
        };
        // SAFETY: RTLD_DEFAULT is a valid lookup handle and name is a static C string.
        let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
        if symbol.is_null() {
            None
        } else {
            // SAFETY: the non-null preadv2/pwritev2 symbol has the PositionedV2 C ABI.
            // The large-file symbols take a 64-bit offset, as do the regular ones selected above.
            Some(unsafe { std::mem::transmute::<*mut libc::c_void, PositionedV2>(symbol) })
        }
    })
}

enum Transfer {
    Read,
    Write,
    Positioned(Positioned, FileOffset),
    PositionedWithFlags(PositionedV2, i64, i32),
}

fn transfer_exported(
    ctx: &mut Context,
    fd: i32,
    spans: &[(Id, Range<usize>)],
    transfer: Transfer,
    write: bool,
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
                    let backing = if write {
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
                // Gather writes only read iov_base, including for immutable bytes.
                Transfer::Write => libc::writev(fd, iov, count),
                Transfer::Positioned(call, offset) => call(fd, iov, count, offset),
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

pub(super) fn transfer(
    ctx: &mut Context,
    fd: i32,
    buffers: Id,
    name: &str,
    position: Option<(i64, i32)>,
) -> Result<Id> {
    let write = matches!(name, "writev" | "pwritev");
    if write {
        crate::initialize_io();
    }
    if !is_sequence(ctx, buffers) {
        // CPython uses the v2 spelling only for preadv's sequence check.
        let name = if name == "preadv" { "preadv2" } else { name };
        return Err(Error::new(
            "TypeError",
            format!("{name}() arg 2 must be a sequence"),
        ));
    }
    let count = ctx.sequence_len(buffers)?;
    let transfer = match position {
        None if write => Transfer::Write,
        None => Transfer::Read,
        Some((offset, flags)) => match positioned_v2(write) {
            Some(call) => Transfer::PositionedWithFlags(call, offset, flags),
            None if flags == 0 => Transfer::Positioned(
                if write { write_at } else { read_at },
                FileOffset::try_from(offset)
                    .map_err(|_| Error::new("OverflowError", "offset does not fit in off_t"))?,
            ),
            None => {
                return Err(Error::new(
                    "NotImplementedError",
                    format!("{name}2: flags unavailable on this platform"),
                ));
            }
        },
    };
    let mut spans = Vec::new();
    spans.try_reserve_exact(count).map_err(no_memory)?;
    let mark = ctx.buffer_exports.len();
    let result = (|| {
        for index in 0..count {
            let key = ctx.int(index as i64);
            let item = ctx.get_item(buffers, key)?;
            let (storage, range) = buffer_span(ctx, item, !write)?;
            // Pin this view and backing before the next __getitem__ runs Python.
            ctx.buffer_exports.try_reserve(2).map_err(no_memory)?;
            ctx.buffer_exports.extend([item, storage]);
            spans.push((storage, range));
        }
        transfer_exported(ctx, fd, &spans, transfer, write)
    })();
    ctx.buffer_exports.truncate(mark);
    result
}
