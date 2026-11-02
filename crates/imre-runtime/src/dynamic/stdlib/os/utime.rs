use std::{io, ptr};

use super::{
    Context, Error, Id, NONE, PathArg, Result, Value, builtins, directory_fd, path_or_fd, stat,
};
use crate::dynamic::stdlib::audit;

const BILLION: i64 = 1_000_000_000;

fn time_overflow() -> Error {
    Error::new(
        "OverflowError",
        "timestamp out of range for platform time_t",
    )
}

fn seconds(ctx: &mut Context, value: Id) -> Result<libc::time_t> {
    ctx.index_c_long(value).map_err(|err| {
        let overflow = ctx.builtin("OverflowError");
        if ctx.raised_error_matches(&err, overflow) {
            time_overflow()
        } else {
            err
        }
    })
}

// CPython splits the fractional seconds before multiplying, preserving precision near the epoch.
fn from_seconds(ctx: &mut Context, value: Id) -> Result<libc::timespec> {
    if let Value::Float(d) = *ctx.heap.get(ctx.scalar_value(value)) {
        if d.is_nan() {
            return Err(Error::new("ValueError", "Invalid value NaN (not a number)"));
        }
        let mut sec = d.trunc();
        let mut ns = (d.fract() * BILLION as f64).floor() as i64;
        if ns < 0 {
            sec -= 1.0;
            ns += BILLION;
        } else if ns >= BILLION {
            sec += 1.0;
            ns -= BILLION;
        }
        let lower = libc::time_t::MIN as f64;
        if !(lower..-lower).contains(&sec) {
            return Err(time_overflow());
        }
        return Ok(libc::timespec {
            tv_sec: sec as libc::time_t,
            tv_nsec: ns,
        });
    }
    let sec = seconds(ctx, value).map_err(|err| {
        let ty = ctx.builtin("TypeError");
        if ctx.raised_error_matches(&err, ty) {
            Error::new(
                "TypeError",
                format!(
                    "argument must be int or float, not {}",
                    ctx.type_name(value)
                ),
            )
        } else {
            err
        }
    })?;
    Ok(libc::timespec {
        tv_sec: sec,
        tv_nsec: 0,
    })
}

fn from_nanoseconds(ctx: &mut Context, value: Id) -> Result<libc::timespec> {
    let billion = ctx.int(BILLION);
    let parts = ctx.divmod(value, billion)?;
    let items = ctx.tuple_items(parts)?;
    let Some(&[sec, ns]) = items.as_deref() else {
        return Err(Error::new(
            "TypeError",
            format!(
                "{}.__divmod__() must return a 2-tuple, not {}",
                ctx.type_name(value),
                ctx.type_name(parts)
            ),
        ));
    };
    Ok(libc::timespec {
        tv_sec: seconds(ctx, sec)?,
        tv_nsec: ctx.index_c_long(ns)?,
    })
}

fn timestamps(ctx: &mut Context, times: Id, ns: Option<Id>) -> Result<Option<[libc::timespec; 2]>> {
    let (pair, convert, message): (_, fn(&mut Context, Id) -> Result<libc::timespec>, _) =
        match (times, ns) {
            (NONE, None) => return Ok(None),
            (NONE, Some(pair)) => (
                pair,
                from_nanoseconds,
                "utime: 'ns' must be a tuple of two ints",
            ),
            (pair, None) => (
                pair,
                from_seconds,
                "utime: 'times' must be either a tuple of two ints or None",
            ),
            (_, Some(_)) => {
                return Err(Error::new(
                    "ValueError",
                    "utime: you may specify either 'times' or 'ns' but not both",
                ));
            }
        };
    if let Value::Tuple(items) = ctx.heap.get(pair)
        && let &[a, m] = items.as_slice()
    {
        return Ok(Some([convert(ctx, a)?, convert(ctx, m)?]));
    }
    Err(Error::new("TypeError", message))
}

/// Set both timestamps with a single POSIX call. A null pair requests the kernel's current time.
pub(super) fn call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let names = ["path", "times", "ns", "dir_fd", "follow_symlinks"];
    let [Some(path), times, ns, dir_fd, follow] = (if args.len() > 2 {
        builtins::bind_method_arguments("utime", args, kwargs, names, 2)?
    } else {
        builtins::bind_builtin_arguments("utime", args, kwargs, names, 1)?
    }) else {
        unreachable!("utime requires path")
    };
    let (path, filename) = path_or_fd(ctx, "utime", path, true, stat::warn_bool_fd)?;
    let dir_fd = directory_fd(ctx, dir_fd)?;
    let follow = ctx.optional_truth(follow, true)?;
    let times = times.unwrap_or(NONE);
    let stamps = timestamps(ctx, times, ns)?;
    if let PathArg::Fd(fd) = &path {
        if dir_fd != libc::AT_FDCWD {
            return Err(Error::new(
                "ValueError",
                "utime: can't specify dir_fd without matching path",
            ));
        }
        if *fd >= 0 && !follow {
            return Err(Error::new(
                "ValueError",
                "utime: cannot use fd and follow_symlinks together",
            ));
        }
    }
    let audit_dir = ctx.int(i64::from(if dir_fd == libc::AT_FDCWD {
        -1
    } else {
        dir_fd
    }));
    audit::event(
        ctx,
        "os.utime",
        &[filename, times, ns.unwrap_or(NONE), audit_dir],
    )?;

    let times_ptr = stamps.as_ref().map_or(ptr::null(), |ts| ts.as_ptr());
    let rc = match &path {
        PathArg::Fd(fd) if *fd == -1 || !follow => {
            // Linux returns immediately for two UTIME_OMIT entries without looking up the path.
            #[cfg(target_os = "linux")]
            if stamps.is_some_and(|ts| ts.iter().all(|t| t.tv_nsec == libc::UTIME_OMIT)) {
                return Ok(NONE);
            }
            // CPython selects its pathname form, but a descriptor supplies no pathname.
            let err = io::Error::from_raw_os_error(libc::EFAULT);
            return Err(ctx.with_errno_filename(crate::objects::errno_error(err).into(), filename)?);
        }
        // SAFETY: fd stays borrowed and times_ptr is null or points to two live timespecs.
        PathArg::Fd(fd) => unsafe { libc::futimens(*fd, times_ptr) },
        PathArg::Name(path) => {
            let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
            // SAFETY: path is NUL-terminated and times_ptr is null or points to two live timespecs.
            unsafe { libc::utimensat(dir_fd, path.as_ptr(), times_ptr, flags) }
        }
    };
    if rc != 0 {
        let error = io::Error::last_os_error();
        #[cfg(target_os = "macos")]
        if error.raw_os_error() == Some(libc::ENOSYS) {
            return Err(Error::new(
                "NotImplementedError",
                "dir_fd unavailable on this platform",
            ));
        }
        return Err(ctx.with_errno_filename(crate::objects::errno_error(error).into(), filename)?);
    }
    Ok(NONE)
}
