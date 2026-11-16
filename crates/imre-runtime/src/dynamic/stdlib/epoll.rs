//! Boxed Python interface to the Linux kernel epoll descriptor.
use super::{Context, Error, Id, NONE, Result, Value, builtins, os, os_error, select};
use crate::objects::epoll::Epoll;
use num_traits::ToPrimitive;
use std::rc::Rc;

pub(in crate::dynamic) const METHODS: &[&str] = &[
    "close",
    "fileno",
    "register",
    "modify",
    "unregister",
    "poll",
    "__enter__",
    "__exit__",
];

fn owner(ctx: &mut Context, fd: i32) -> Result<Id> {
    let handle = Epoll::from_fd(fd).map_err(os_error)?;
    ctx.heap
        .alloc_descriptor_owner(Value::Epoll(Rc::new(handle)))
}

pub(super) fn create(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [sizehint, flags] =
        builtins::bind_builtin_arguments("epoll", args, kwargs, ["sizehint", "flags"], 0)?;
    let sizehint = sizehint
        .map(|v| os::c_int(ctx, v))
        .transpose()?
        .unwrap_or(-1);
    let flags = flags.map(|v| os::c_int(ctx, v)).transpose()?.unwrap_or(0);
    if sizehint != -1 && sizehint <= 0 {
        return Err(Error::new("ValueError", "negative sizehint"));
    }
    if flags != 0 && flags != libc::EPOLL_CLOEXEC {
        return Err(Error::new("OSError", "invalid flags"));
    }
    owner(ctx, -1)
}

fn fileno(handle: &Epoll) -> Result<i32> {
    let fd = handle.fd();
    if fd < 0 {
        Err(Error::new(
            "ValueError",
            "I/O operation on closed epoll object",
        ))
    } else {
        Ok(fd)
    }
}

fn timeout_ns(ctx: &mut Context, timeout: Option<Id>) -> Result<Option<i64>> {
    let Some(value) = timeout.filter(|&v| v != NONE) else {
        return Ok(None);
    };
    if let Value::Float(seconds) = ctx.heap.get(ctx.scalar_value(value)) {
        return Ok(Some(crate::objects::time::sleep_float_ns(*seconds)?));
    }
    let seconds = ctx.index_integer(value).map_err(|e| {
        if ctx.is_builtin_error(&e, "TypeError") {
            Error::new("TypeError", "timeout must be an integer or None")
        } else {
            e
        }
    })?;
    seconds
        .to_i64()
        .and_then(|s| s.checked_mul(1_000_000_000))
        .map(Some)
        .ok_or_else(|| {
            Error::new(
                "OverflowError",
                "timestamp too large to convert to C PyTime_t",
            )
        })
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "fromfd" {
        let [Some(fd)] = builtins::bind_method_arguments("fromfd", args, kwargs, [""], 1)? else {
            return Err(Error::new(
                "TypeError",
                "fromfd() missing required argument 'fd'",
            ));
        };
        let fd = os::c_int(ctx, fd)?;
        return owner(ctx, fd);
    }
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new("TypeError", "expected epoll instance"));
    };
    let Value::Epoll(handle) = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected epoll instance"));
    };
    let handle = handle.clone();
    match name {
        "close" | "fileno" | "__enter__" => {
            builtins::bind_builtin_arguments(name, args, kwargs, [], 0)?;
            if name == "close" {
                handle.close();
                return Ok(NONE);
            }
            let fd = fileno(&handle)?;
            Ok(if name == "fileno" {
                ctx.int(i64::from(fd))
            } else {
                receiver
            })
        }
        "__exit__" => {
            builtins::bind_method_arguments(name, args, kwargs, ["", "", ""], 3)?;
            handle.close();
            Ok(NONE)
        }
        "register" | "modify" | "unregister" => {
            let (fd, mask, op) = if name == "unregister" {
                let [fd] = builtins::bind_builtin_arguments(name, args, kwargs, ["fd"], 1)?;
                (fd, None, libc::EPOLL_CTL_DEL)
            } else {
                let modify = name == "modify";
                let [fd, mask] = builtins::bind_builtin_arguments(
                    name,
                    args,
                    kwargs,
                    ["fd", "eventmask"],
                    if modify { 2 } else { 1 },
                )?;
                (
                    fd,
                    mask,
                    if modify {
                        libc::EPOLL_CTL_MOD
                    } else {
                        libc::EPOLL_CTL_ADD
                    },
                )
            };
            let fd = fd.expect("required fd");
            os::stat::warn_bool_fd(ctx, fd)?;
            let fd = select::fd(ctx, fd)?;
            let events = if let Some(mask) = mask {
                let mask = ctx.index_integer(mask)?;
                (&*mask.big() & num_bigint::BigInt::from(u32::MAX))
                    .to_u32()
                    .unwrap()
            } else {
                (libc::EPOLLIN | libc::EPOLLPRI | libc::EPOLLOUT) as u32
            };
            fileno(&handle)?;
            handle.ctl(op, fd, events).map_err(os_error)?;
            Ok(NONE)
        }
        "poll" => {
            let [timeout, maxevents] =
                builtins::bind_builtin_arguments(name, args, kwargs, ["timeout", "maxevents"], 0)?;
            let maxevents = maxevents
                .map(|v| os::c_int(ctx, v))
                .transpose()?
                .unwrap_or(-1);
            fileno(&handle)?;
            let timeout = timeout_ns(ctx, timeout)?;
            let events = handle.poll(timeout, maxevents, || ctx.dispatch_signals())?;
            let mut out = Vec::new();
            out.try_reserve_exact(events.len())
                .map_err(|_| Error::new("MemoryError", ""))?;
            for (fd, mask) in events {
                let fd = ctx.int(i64::from(fd));
                let mask = ctx.int(i64::from(mask));
                out.push(ctx.alloc(Value::Tuple(super::super::copy_ids(&[fd, mask])?)));
            }
            Ok(ctx.alloc(Value::List(out)))
        }
        _ => Err(Error::new("AttributeError", name.to_owned())),
    }
}
