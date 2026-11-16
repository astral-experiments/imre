//! Boxed POSIX readiness shares the descriptor sets and poll registrations with typed calls.
use super::super::{calls::RegisteredCall, copy_ids};
use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::{
    errno_error,
    readiness::{self, SelectFds},
};
use std::{io, time::Duration};

/// Convert an integer or the integer returned by a file object's fileno method.
pub(super) fn fd(ctx: &mut Context, object: Id) -> Result<i32> {
    let number = if let Ok(n) = ctx.borrow_int(object) {
        n
    } else {
        let method = ctx.lookup_attr(object, "fileno").map_err(|e| {
            if ctx.is_attribute_error(&e) {
                Error::new(
                    "TypeError",
                    "argument must be an int, or have a fileno() method.",
                )
            } else {
                e
            }
        })?;
        let number = ctx.call(method, Vec::new(), Vec::new())?;
        ctx.borrow_int(number)
            .map_err(|_| Error::new("TypeError", "fileno() returned a non-integer"))?
    };
    Ok(readiness::checked_fd(number.to_c_int()?)?)
}

/// Convert the optional timeout before a syscall or any descriptor lookups.
pub(super) fn timeout(
    ctx: &mut Context,
    value: Option<&Id>,
    poll: bool,
) -> Result<Option<Duration>> {
    let Some(&value) = value.filter(|&&id| id != NONE) else {
        return Ok(None);
    };
    if let Value::Float(value) = ctx.heap.get(ctx.scalar_value(value)) {
        return Ok(readiness::float_timeout(*value, poll)?);
    }
    let n = ctx.index_integer(value).map_err(|e| {
        if e.kind != "TypeError" {
            return e;
        }
        Error::new(
            "TypeError",
            if poll {
                "timeout must be an integer or None"
            } else {
                "timeout must be a float or None"
            },
        )
    })?;
    Ok(readiness::int_timeout(n.to_i64(), poll)?)
}

/// Keep the positional arguments under the ordinary call's operand roots until return.
pub(in crate::dynamic) fn prepare(
    args: Vec<Id>,
    kwargs: &[(String, Id)],
) -> Result<RegisteredCall> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "select.select() takes no keyword arguments",
        ));
    }
    let n = args.len();
    let expected = match n {
        3 | 4 => return Ok(RegisteredCall::Select(args)),
        0..3 => "at least 3 arguments",
        _ => "at most 4 arguments",
    };
    Err(Error::new(
        "TypeError",
        format!("select expected {expected}, got {n}"),
    ))
}

/// Convert each input and keep its Python identity alive on the calling execution.
fn descriptor_sets(
    ctx: &mut Context,
    args: &[Id],
    inputs: &mut [Vec<(Id, i32)>; 3],
) -> Result<(SelectFds, Option<Duration>)> {
    let timeout = timeout(ctx, args.get(3), false)?;
    let mut sets = SelectFds::new();
    for (group, items) in inputs.iter_mut().enumerate() {
        let source = args[group];
        let iterator = ctx.iterator(source).map_err(|e| {
            if e.kind == "TypeError" {
                Error::new("TypeError", "arguments 1-3 must be sequences")
            } else {
                e
            }
        })?;
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(iterator);
        while let Some(obj) = ctx.next_item(iterator)? {
            ctx.heap.reserve_temporaries(1)?;
            ctx.heap.keep(obj);
            let n = fd(ctx, obj)?;
            sets.insert(group, items.len(), n)?;
            items
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            items.push((obj, n));
        }
    }
    Ok((sets, timeout))
}

/// Retain original ready objects in input order, even when the same descriptor appears twice.
///
/// # Safety
/// The registered caller owns the context and roots the arguments throughout the wait.
pub(in crate::dynamic) unsafe fn run(context: *mut Context, args: &[Id]) -> Result<Id> {
    // SAFETY: borrow the active execution only while converting and rooting inputs.
    let mut inputs = [Vec::new(), Vec::new(), Vec::new()];
    // SAFETY: the caller owns the context. This borrow ends before the readiness wait.
    let (mut sets, timeout) = descriptor_sets(unsafe { &mut *context }, args, &mut inputs)?;
    let any_ready = sets.wait(timeout, || {
        // SAFETY: only the rooted operands and native fd sets survive through signal delivery.
        unsafe { Context::dispatch_pending_signals(context) }
    })?;
    // SAFETY: the native wait and any registered signal callbacks have finished.
    let ctx = unsafe { &mut *context };
    let mut lists = Vec::new();
    lists
        .try_reserve_exact(3)
        .map_err(|_| Error::new("MemoryError", ""))?;
    for (group, items) in inputs.into_iter().enumerate() {
        let mut ready = Vec::new();
        if any_ready {
            for (obj, n) in items {
                if sets.ready(group, n) {
                    ready
                        .try_reserve(1)
                        .map_err(|_| Error::new("MemoryError", ""))?;
                    ready.push(obj);
                }
            }
        }
        lists.push(ctx.alloc(Value::List(ready)));
    }
    Ok(ctx.alloc(Value::Tuple(lists)))
}

/// Dispatch the module's positional functions and bound poll methods.
pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let method = name.strip_prefix("poll.");
    let (method, owner, handle, args) = if let Some(method) = method {
        let invalid = || Error::new("TypeError", format!("invalid receiver for poll.{method}"));
        let (&receiver, args) = args.split_first().ok_or_else(invalid)?;
        let Value::Poll(handle) = ctx.heap.get(receiver) else {
            return Err(invalid());
        };
        (method, "poll", Some(handle.clone()), args)
    } else {
        (name, "select", None, args)
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{owner}.{method}() takes no keyword arguments"),
        ));
    }
    let n = args.len();
    let expected = match (name, n) {
        ("poll", 0) => return Ok(ctx.alloc(Value::Poll(Default::default()))),
        ("poll", _) => {
            return Err(Error::new(
                "TypeError",
                format!("select.poll() takes no arguments ({n} given)"),
            ));
        }
        ("poll.register", 0) => Some("at least 1 argument"),
        ("poll.register", 3..) => Some("at most 2 arguments"),
        ("poll.modify", n) if n != 2 => Some("2 arguments"),
        ("poll.unregister", n) if n != 1 => {
            return Err(Error::new(
                "TypeError",
                format!("poll.unregister() takes exactly one argument ({n} given)"),
            ));
        }
        ("poll.poll", 2..) => Some("at most 1 argument"),
        _ => None,
    };
    if let Some(expected) = expected {
        return Err(Error::new(
            "TypeError",
            format!("{method} expected {expected}, got {n}"),
        ));
    }
    let Some(handle) = handle else {
        return Err(Error::new(
            "AttributeError",
            format!("module 'select' has no attribute '{name}'"),
        ));
    };
    if method == "poll" {
        let timeout = timeout(ctx, args.first(), true)?;
        let events = readiness::Poll::ready(&handle, timeout, || ctx.dispatch_signals())?;
        let mut out = Vec::new();
        out.try_reserve_exact(events.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        for event in events {
            let fd = ctx.int(i64::from(event.fd));
            let flags = ctx.int(i64::from(event.revents as u16));
            out.push(ctx.alloc(Value::Tuple(copy_ids(&[fd, flags])?)));
        }
        return Ok(ctx.alloc(Value::List(out)));
    }
    let fd = fd(ctx, args[0])?;
    if method == "unregister" {
        if !handle.borrow_mut().remove(fd) {
            let key = ctx.int(i64::from(fd));
            return Err(ctx.lookup_error("KeyError", key));
        }
    } else {
        let mask = if let Some(&mask) = args.get(1) {
            readiness::poll_mask(&ctx.index_integer(mask)?)?
        } else {
            libc::POLLIN | libc::POLLOUT | libc::POLLPRI
        };
        let mut poll = handle.borrow_mut();
        if method == "modify" && !poll.contains(fd) {
            return Err(errno_error(io::Error::from_raw_os_error(libc::ENOENT)).into());
        }
        poll.set(fd, mask)
            .map_err(|_| Error::new("MemoryError", ""))?;
    }
    Ok(NONE)
}
