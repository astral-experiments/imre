//! The Python argument conversion and exported buffers for the native message syscalls.
use super::{
    Context, Error, Id, NONE, Result, Value, address, native, pack_socket_name, receive_size,
};
use crate::dynamic::{copy_ids, raw_read_window::IoVectors};
use crate::objects::Object;
use std::ops::Range;

// Retain both the exported view and its backing through conversions of subsequent
// buffers, without retaining any Rust borrow over a call into Python.
struct Lease {
    object_root: usize,
    view: Option<Id>,
    release_native: bool,
}

struct Buffers {
    mark: usize,
    leases: Vec<Lease>,
    spans: Vec<(Id, Range<usize>)>,
}

impl Buffers {
    fn new(ctx: &Context) -> Self {
        Self {
            mark: ctx.buffer_exports.len(),
            leases: Vec::new(),
            spans: Vec::new(),
        }
    }

    fn push(&mut self, ctx: &mut Context, object: Id, writable: bool) -> Result<()> {
        self.leases
            .try_reserve(1)
            .map_err(|_| crate::Error::memory_error())?;
        self.spans
            .try_reserve(1)
            .map_err(|_| crate::Error::memory_error())?;
        ctx.buffer_exports
            .try_reserve(2)
            .map_err(|_| crate::Error::memory_error())?;
        ctx.heap.reserve_temporaries(1)?;
        let inherited = matches!(
            ctx.heap.get(ctx.scalar_value(object)),
            Value::Bytes(_) | Value::ByteArray(_)
        ) && ctx.buffer_slot(object, "__buffer__") == Some(None);
        let acquire = |ctx: &mut Context| {
            let method = if inherited {
                None
            } else {
                ctx.python_buffer_method(object, true)?
            };
            let view = match method {
                Some(m) => ctx.acquire_python_buffer(object, m, i64::from(writable))?,
                None => ctx.scalar_value(object),
            };
            if !writable {
                ctx.native_buffer_bytes(view)?;
            }
            let span = ctx
                .contiguous_byte_span(view, writable)?
                .ok_or_else(|| Error::new("BufferError", "buffer is not contiguous or writable"))?;
            Ok((view, method.is_some(), span))
        };
        let object_root = ctx.heap.temporary_mark();
        ctx.heap.keep(object);
        let acquired = acquire(ctx);
        // The export slots below own the acquired view. A temporary from calling
        // __buffer__ must not keep it alive after this vector entry is released.
        ctx.heap.release_temporaries_since(object_root);
        let (view, python, (storage, range)) = acquired.map_err(|err| {
            if writable {
                Error::new("TypeError", "recvmsg_into() argument 1 must be an iterable of single-segment read-write buffers")
            } else {
                err
            }
        })?;
        ctx.heap.keep(object);
        ctx.buffer_exports.extend([view, storage]);
        self.spans.push((storage, range));
        self.leases.push(Lease {
            object_root,
            view: python.then_some(view),
            release_native: inherited,
        });
        Ok(())
    }

    fn finish<T>(self, ctx: &mut Context, result: Result<T>) -> Result<T> {
        if let Err(error) = &result {
            for root in error.roots() {
                ctx.heap.keep(root);
            }
        }
        // CPython releases each vector in acquisition order. Other exports of the
        // same allocation must remain visible to a reentrant release callback.
        for (i, lease) in self.leases.into_iter().enumerate() {
            let object = *ctx.heap.temporary_root(lease.object_root);
            let mark = ctx.heap.temporary_mark();
            if lease.release_native
                && ctx
                    .buffer_slot(object, "__release_buffer__")
                    .flatten()
                    .is_some()
            {
                ctx.notify_native_buffer_release(object);
            }
            let slot = self.mark + 2 * i;
            ctx.buffer_exports[slot..slot + 2].fill(NONE);
            if let Some(view) = lease.view {
                ctx.heap.keep(view);
                ctx.release_python_buffer(object, view);
            }
            ctx.heap.release_temporaries_since(mark);
            *ctx.heap.temporary_root(lease.object_root) = NONE;
        }
        ctx.buffer_exports.truncate(self.mark);
        result
    }
}

fn data_buffers(ctx: &mut Context, exports: &mut Buffers, input: Id, method: &str) -> Result<()> {
    // PySequence_Fast retains an exact list or tuple. In particular, a buffer hook
    // on one list item can replace a later item before its export is requested.
    let sequence = if matches!(ctx.heap.get(input), Value::List(_) | Value::Tuple(_)) {
        input
    } else {
        let iter = ctx.iterator(input).map_err(|e| {
            if ctx.is_builtin_error(&e, "TypeError") {
                Error::new(
                    "TypeError",
                    format!("{method}() argument 1 must be an iterable"),
                )
            } else {
                e
            }
        })?;
        let tail = ctx.iterator(iter)?;
        let default = ctx.int(8);
        let hint = ctx.length_hint(iter, Some(default))?;
        let mut items = Vec::new();
        items
            .try_reserve_exact(ctx.as_integer(hint)? as usize)
            .map_err(|_| crate::Error::memory_error())?;
        ctx.collect_iterator_items(tail, false, items)?
    };
    let count = ctx.sequence_len(sequence)?;
    if count > i32::MAX as usize {
        return Err(Error::new(
            "OSError",
            format!("{method}() argument 1 is too long"),
        ));
    }
    for i in 0..count {
        let key = ctx.int(i as i64);
        let item = ctx.get_item(sequence, key)?;
        exports.push(ctx, item, method == "recvmsg_into")?;
    }
    Ok(())
}

fn control_item(ctx: &mut Context, item: Id) -> Result<[Id; 3]> {
    const NAME: &str = "[sendmsg() ancillary data items]() argument";
    let fields = if let Some(tuple) = ctx.tuple_items(item)? {
        tuple
    } else {
        if !ctx.is_sequence(item)
            || matches!(
                ctx.heap.get(ctx.scalar_value(item)),
                Value::Str(_) | Value::SurrogateStr(_) | Value::Bytes(_) | Value::ByteArray(_)
            )
        {
            let kind = if item == NONE {
                "None"
            } else {
                ctx.type_name(item)
            };
            return Err(Error::new(
                "TypeError",
                format!("{NAME} must be 3-item tuple, not {kind}"),
            ));
        }
        let len = ctx.sequence_len(item)?;
        if len != 3 {
            return Err(Error::new(
                "TypeError",
                format!("{NAME} must be sequence of length 3, not {len}"),
            ));
        }
        let seq = ctx.collect_iterable(item, true)?;
        ctx.tuple_items(seq)?.expect("collected tuple")
    };
    fields.try_into().map_err(|v: Vec<Id>| {
        Error::new(
            "TypeError",
            format!("{NAME} must be tuple of length 3, not {}", v.len()),
        )
    })
}

/// Recreate any pointers into the Python heap after a returning signal handler.
fn retry<T>(
    ctx: &mut Context,
    socket: &Object,
    mut attempt: impl FnMut(&mut Context, Option<native::Deadline>) -> Result<T>,
) -> Result<T> {
    let until = native::deadline(socket);
    loop {
        match attempt(ctx, until) {
            Err(err) if ctx.has_errno(&err, libc::EINTR) => {
                ctx.dispatch_signals()?;
                if until.is_some_and(|end| end.remaining().is_zero()) {
                    return Err(Error::new("TimeoutError", "timed out"));
                }
            }
            result => return result,
        }
    }
}

fn send(ctx: &mut Context, socket: &Object, receiver: Id, args: &[Id], flags: i32) -> Result<Id> {
    let peer = args.get(3).copied().unwrap_or(NONE);
    let destination = if peer == NONE {
        None
    } else {
        Some(address(ctx, native::family(socket), peer, "sendmsg")?)
    };
    super::super::audit::event(ctx, "socket.sendmsg", &[receiver, peer])?;
    let mut streams = Buffers::new(ctx);
    let result = (|| {
        data_buffers(ctx, &mut streams, args[0], "sendmsg")?;
        let mut controls = Buffers::new(ctx);
        let result = (|| {
            let mut headers = Vec::new();
            if let Some(&anc) = args.get(1) {
                let seq = ctx.collect_iterable(anc, true).map_err(|_| {
                    Error::new("TypeError", "sendmsg() argument 2 must be an iterable")
                })?;
                for item in ctx.tuple_items(seq)?.expect("collected tuple") {
                    let [level, kind, payload] = control_item(ctx, item)?;
                    let level = ctx.index_c_int(level)?;
                    let kind = ctx.index_c_int(kind)?;
                    controls.push(ctx, payload, false)?;
                    headers
                        .try_reserve(1)
                        .map_err(|_| crate::Error::memory_error())?;
                    headers.push((level, kind));
                }
            }
            let mut ancillary = Vec::new();
            ancillary
                .try_reserve_exact(headers.len())
                .map_err(|_| crate::Error::memory_error())?;
            for ((level, kind), (storage, span)) in headers.into_iter().zip(&controls.spans) {
                ancillary.push((level, kind, &ctx.owner_bytes(*storage)?[span.clone()]));
            }
            let mut control = native::message::Control::from_items(&ancillary)?;
            drop(ancillary);
            let n = retry(ctx, socket, |ctx, until| {
                let mut scratch = IoVectors::new(streams.spans.len())?;
                let iov = scratch.resolve(ctx, &streams.spans, false)?;
                // SAFETY: the exports pin all ranges. The pointers expire before dispatch.
                Ok(unsafe {
                    native::message::send(
                        socket,
                        iov,
                        &mut control,
                        flags,
                        destination.as_ref(),
                        until,
                    )
                }?)
            })?;
            Ok(ctx.int(n as i64))
        })();
        controls.finish(ctx, result)
    })();
    streams.finish(ctx, result)
}

fn receive(
    ctx: &mut Context,
    socket: &Object,
    spans: &[(Id, Range<usize>)],
    mut data: Option<Vec<u8>>,
    ancbufsize: isize,
    flags: i32,
) -> Result<Id> {
    let size = usize::try_from(ancbufsize)
        .map_err(|_| Error::new("ValueError", "invalid ancillary data buffer length"))?;
    let mut control = native::message::ReceivedControl::new(size)?;
    let (n, flags, peer) = retry(ctx, socket, |ctx, until| {
        let mut single;
        let mut scratch;
        let iov = if let Some(bytes) = &mut data {
            single = [libc::iovec {
                iov_base: bytes.as_mut_ptr().cast(),
                iov_len: bytes.len(),
            }];
            &mut single[..]
        } else {
            scratch = IoVectors::new(spans.len())?;
            scratch.resolve(ctx, spans, true)?
        };
        // SAFETY: the private buffer or pinned writable exports outlive this attempt.
        Ok(unsafe { native::message::recv(socket, iov, &mut control, flags, until) }?)
    })?;
    if control.malformed() {
        ctx.emit_builtin_warning(
            crate::objects::exceptions::Kind::RuntimeWarning,
            "received malformed or improperly-truncated ancillary data",
            1,
        )?;
    }
    let mut entries = Vec::new();
    for (level, kind, payload) in control.iter() {
        let level = ctx.int(i64::from(level));
        let kind = ctx.int(i64::from(kind));
        let bytes = ctx.alloc(Value::Bytes(copy_ids(payload)?));
        entries
            .try_reserve(1)
            .map_err(|_| crate::Error::memory_error())?;
        entries.push(ctx.alloc(Value::Tuple(vec![level, kind, bytes])));
    }
    let payload = if let Some(mut bytes) = data {
        let requested = bytes.len();
        bytes.truncate(n);
        if bytes.len() < requested / 2 {
            crate::compact_bytes(&mut bytes);
        }
        ctx.alloc(Value::Bytes(bytes))
    } else {
        ctx.int(n as i64)
    };
    let ancillary = ctx.alloc(Value::List(entries));
    let flags = ctx.int(i64::from(flags));
    let address = pack_socket_name(ctx, peer)?;
    let result = ctx.alloc(Value::Tuple(vec![payload, ancillary, flags, address]));
    control.release_fds();
    Ok(result)
}

pub(super) fn call(
    ctx: &mut Context,
    socket: &Object,
    receiver: Id,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("socket.{method}() takes no keyword arguments"),
        ));
    }
    let max = if method == "sendmsg" { 4 } else { 3 };
    if args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{method}() takes at least 1 argument (0 given)"),
        ));
    }
    if args.len() > max {
        return Err(Error::new(
            "TypeError",
            format!(
                "{method}() takes at most {max} arguments ({} given)",
                args.len()
            ),
        ));
    }
    let size = if method == "recvmsg" {
        receive_size(ctx, args[0])?
    } else {
        0
    };
    let ancillary = if method != "sendmsg" && args.len() > 1 {
        receive_size(ctx, args[1])?
    } else {
        0
    };
    let flags = args
        .get(2)
        .map(|&id| ctx.index_c_int(id))
        .transpose()?
        .unwrap_or(0);
    if method == "sendmsg" {
        return send(ctx, socket, receiver, args, flags);
    }
    if method == "recvmsg" {
        let size = usize::try_from(size)
            .map_err(|_| Error::new("ValueError", "negative buffer size in recvmsg()"))?;
        let bytes = native::zeroed_receive_buffer(size)?;
        return receive(ctx, socket, &[], Some(bytes), ancillary, flags);
    }
    let mut buffers = Buffers::new(ctx);
    let result = (|| {
        data_buffers(ctx, &mut buffers, args[0], method)?;
        receive(ctx, socket, &buffers.spans, None, ancillary, flags)
    })();
    buffers.finish(ctx, result)
}
