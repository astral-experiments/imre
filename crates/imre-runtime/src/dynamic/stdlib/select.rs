//! Boxed POSIX readiness shares the descriptor sets and poll registrations with typed calls.
use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::{
    errno_error,
    readiness::{self, SelectFds},
};
use std::{io, time::Duration};

/// Convert an integer or the integer returned by a file object's fileno method.
fn fd(ctx: &mut Context, object: Id) -> Result<i32> {
    let number = if let Ok(n) = ctx.as_int(object) {
        n
    } else {
        let method = ctx.lookup_attr(object, "fileno").map_err(|e| {
            if e.kind == "AttributeError" {
                Error::new(
                    "TypeError",
                    "argument must be an int, or have a fileno() method.",
                )
            } else {
                e
            }
        })?;
        let number = ctx.call(method, Vec::new(), Vec::new())?;
        ctx.as_int(number)
            .map_err(|_| Error::new("TypeError", "fileno() returned a non-integer"))?
    };
    let n = number
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))?;
    Ok(readiness::checked_fd(n)?)
}

/// Convert the optional timeout before a syscall or any descriptor lookups.
fn timeout(ctx: &mut Context, value: Option<&Id>, poll: bool) -> Result<Option<Duration>> {
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

/// Retain original ready objects in input order, even when the same descriptor appears twice.
fn select(ctx: &mut Context, args: &[Id]) -> Result<Id> {
    let timeout = timeout(ctx, args.get(3), false)?;
    let mut sets = SelectFds::new();
    let mut inputs = [Vec::new(), Vec::new(), Vec::new()];
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
    let any_ready = sets.wait(timeout)?;
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
        ("select", 3 | 4) => return select(ctx, args),
        ("select", 0..3) => Some("at least 3 arguments"),
        ("select", _) => Some("at most 4 arguments"),
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
        let mut poll = handle.borrow_mut();
        let (n, events) = poll.ready(timeout)?;
        let mut out = Vec::new();
        out.try_reserve_exact(n)
            .map_err(|_| Error::new("MemoryError", ""))?;
        for (fd, flags) in events {
            let fd = ctx.int(i64::from(fd));
            let flags = ctx.int(i64::from(flags));
            out.push(ctx.alloc(Value::Tuple(vec![fd, flags])));
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
