use std::io;

use super::{Context, Id, NONE, PathArg, Result, builtins, file_offset, native, path_or_fd, stat};
use crate::dynamic::stdlib::{audit, os_error};

/// Truncate a pathname or an open descriptor after converting both arguments.
pub(super) fn call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(path), Some(length)] =
        builtins::bind_builtin_arguments("truncate", args, kwargs, ["path", "length"], 2)?
    else {
        unreachable!("truncate requires path and length")
    };
    let (path, filename) = path_or_fd(ctx, "truncate", path, true, stat::warn_bool_fd)?;
    let length = file_offset(ctx, length)?;
    if let PathArg::Fd(fd) = path
        && fd != -1
    {
        return by_fd(ctx, fd, length);
    }

    let audit_length = ctx.int(length);
    audit::event(ctx, "os.truncate", &[filename, audit_length])?;
    let error = match path {
        PathArg::Name(name) => {
            // SAFETY: name holds a live NUL-terminated pathname for this call.
            if unsafe { libc::truncate(name.as_ptr(), length) } == 0 {
                return Ok(NONE);
            }
            io::Error::last_os_error()
        }
        // CPython's path_t has no pathname for fd -1. Linux rejects a negative
        // length before attempting to read that pathname.
        PathArg::Fd(_) if cfg!(target_os = "linux") && length < 0 => {
            io::Error::from_raw_os_error(libc::EINVAL)
        }
        PathArg::Fd(_) => io::Error::from_raw_os_error(libc::EFAULT),
    };
    Err(ctx.with_errno_filename(os_error(error), filename)?)
}

/// Audit the converted fd and length once, then truncate the borrowed descriptor.
pub(super) fn by_fd(ctx: &mut Context, fd: i32, length: i64) -> Result<Id> {
    let audit_fd = ctx.int(i64::from(fd));
    let audit_length = ctx.int(length);
    audit::event(ctx, "os.truncate", &[audit_fd, audit_length])?;
    loop {
        match native::truncate(fd, length) {
            Ok(()) => return Ok(NONE),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => ctx.dispatch_signals()?,
            Err(error) => return Err(os_error(error)),
        }
    }
}
