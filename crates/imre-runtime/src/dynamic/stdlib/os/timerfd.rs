//! Linux timer descriptors. The kernel tracks each timer's clock and expirations.
use std::mem::MaybeUninit;

use super::{Context, Error, Id, Result, Value, builtins, c_int, stat};
use crate::dynamic::stdlib::{math, select};

const BILLION: i64 = 1_000_000_000;

fn arguments<const N: usize>(
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    names: [&str; N],
) -> Result<[Option<Id>; N]> {
    if args.len() + kwargs.len() <= N && args.len() != 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes exactly 1 positional argument ({} given)",
                args.len()
            ),
        ));
    }
    builtins::bind_builtin_arguments(name, args, kwargs, names, 0)
}

fn fd(ctx: &mut Context, value: Id) -> Result<i32> {
    stat::warn_bool_fd(ctx, value)?;
    select::fd(ctx, value)
}

fn checked(result: i32) -> Result<i32> {
    if result == -1 {
        return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
    }
    Ok(result)
}

fn seconds_to_ns(seconds: f64) -> Result<i64> {
    let nanos = (seconds * BILLION as f64).floor();
    if !(i64::MIN as f64..-(i64::MIN as f64)).contains(&nanos) {
        return Err(Error::new(
            "OverflowError",
            "timestamp out of range for platform time_t",
        ));
    }
    Ok(nanos as i64)
}

fn nanoseconds(ctx: &mut Context, value: Option<Id>) -> Result<i64> {
    let Some(value) = value else { return Ok(0) };
    ctx.index_integer(value)?
        .to_i64()
        .ok_or_else(|| Error::new("OverflowError", "int too big to convert"))
}

fn timespec(ns: i64, name: &str) -> Result<libc::timespec> {
    let tv_sec = ns
        .div_euclid(BILLION)
        .try_into()
        .map_err(|_| Error::new("ValueError", format!("invalid {name} value")))?;
    Ok(libc::timespec {
        tv_sec,
        tv_nsec: ns.rem_euclid(BILLION) as _,
    })
}

fn timer_value(ctx: &mut Context, stamp: libc::timespec, nanos: bool) -> Result<Id> {
    if nanos {
        let ns = crate::objects::time::timestamp_ns(stamp.tv_sec, stamp.tv_nsec)?;
        Ok(ctx.int(ns))
    } else {
        Ok(ctx.alloc(Value::Float(
            stamp.tv_sec as f64 + stamp.tv_nsec as f64 * 1e-9,
        )))
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "timerfd_create" {
        let [clock, flags] = arguments(name, args, kwargs, ["", "flags"])?;
        let clock = c_int(ctx, clock.unwrap())?;
        let flags = flags.map(|x| c_int(ctx, x)).transpose()?.unwrap_or(0);
        // SAFETY: both arguments are integers. A successful fd is owned by the caller.
        let fd = checked(unsafe { libc::timerfd_create(clock, flags | libc::TFD_CLOEXEC) })?;
        return Ok(ctx.int(i64::from(fd)));
    }

    let nanos = name.ends_with("_ns");
    let mut previous = MaybeUninit::<libc::itimerspec>::uninit();
    let status = if matches!(name, "timerfd_gettime" | "timerfd_gettime_ns") {
        let arg = builtins::one_positional_arg(&format!("posix.{name}"), args, kwargs)?;
        let fd = fd(ctx, arg)?;
        // SAFETY: previous points to writable storage for the kernel timer state.
        unsafe { libc::timerfd_gettime(fd, previous.as_mut_ptr()) }
    } else {
        let [arg, flags, initial, interval] =
            arguments(name, args, kwargs, ["", "flags", "initial", "interval"])?;
        let fd = fd(ctx, arg.unwrap())?;
        let flags = flags.map(|x| c_int(ctx, x)).transpose()?.unwrap_or(0);
        let (initial, interval) = if nanos {
            (nanoseconds(ctx, initial)?, nanoseconds(ctx, interval)?)
        } else {
            // Argument Clinic converts both Python objects to doubles before rounding either.
            let initial = initial
                .map(|x| math::as_double(ctx, x))
                .transpose()?
                .unwrap_or(0.0);
            let interval = interval
                .map(|x| math::as_double(ctx, x))
                .transpose()?
                .unwrap_or(0.0);
            (seconds_to_ns(initial)?, seconds_to_ns(interval)?)
        };
        let new = libc::itimerspec {
            it_value: timespec(initial, "initial")?,
            it_interval: timespec(interval, "interval")?,
        };
        // SAFETY: new and previous point to distinct structs for the duration of the call.
        unsafe { libc::timerfd_settime(fd, flags, &new, previous.as_mut_ptr()) }
    };
    checked(status)?;
    // SAFETY: a successful gettime or settime initialized the result.
    let previous = unsafe { previous.assume_init() };
    let expiration = timer_value(ctx, previous.it_value, nanos)?;
    let interval = timer_value(ctx, previous.it_interval, nanos)?;
    Ok(ctx.alloc(Value::Tuple(vec![expiration, interval])))
}
