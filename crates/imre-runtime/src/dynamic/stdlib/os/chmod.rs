use std::io;

use super::{
    Context, Error, Id, NONE, PathArg, Result, builtins, c_int, directory_fd, path_or_fd, stat,
};
use crate::dynamic::stdlib::audit;

/// Change mode on the named entry or open descriptor after all converters and audit hooks run.
pub(super) fn call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let names = ["path", "mode", "dir_fd", "follow_symlinks"];
    let [Some(path), Some(mode), dir_fd, follow] = (if args.len() > 2 {
        builtins::bind_method_arguments("chmod", args, kwargs, names, 2)?
    } else {
        builtins::bind_builtin_arguments("chmod", args, kwargs, names, 2)?
    }) else {
        unreachable!("chmod requires path and mode")
    };
    let (path, filename) = path_or_fd(ctx, "chmod", path, true, stat::warn_bool_fd)?;
    let mode = c_int(ctx, mode)?;
    let dir_fd = directory_fd(ctx, dir_fd)?;
    let follow = follow.map(|v| ctx.truth(v)).transpose()?.unwrap_or(true);
    let audit_mode = ctx.int(i64::from(mode));
    let audit_fd = ctx.int(i64::from(if dir_fd == libc::AT_FDCWD {
        -1
    } else {
        dir_fd
    }));
    audit::event(ctx, "os.chmod", &[filename, audit_mode, audit_fd])?;

    let mode = mode as libc::mode_t;
    let (rc, check_nofollow) = match &path {
        // CPython reserves -1 in path_t for a missing descriptor and has no pathname here.
        PathArg::Fd(-1) => {
            let error = crate::objects::errno_error(io::Error::from_raw_os_error(libc::EFAULT));
            return Err(ctx.with_errno_filename(error.into(), filename)?);
        }
        PathArg::Fd(fd) => (set_fd_mode(*fd, mode), false),
        PathArg::Name(name) if follow && dir_fd == libc::AT_FDCWD => {
            // SAFETY: name is a live, NUL-terminated pathname.
            (unsafe { libc::chmod(name.as_ptr(), mode) }, false)
        }
        #[cfg(target_os = "macos")]
        PathArg::Name(name) if !follow && dir_fd == libc::AT_FDCWD => {
            // SAFETY: name is a live, NUL-terminated pathname.
            (unsafe { libc::lchmod(name.as_ptr(), mode) }, false)
        }
        PathArg::Name(name) => {
            let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
            // SAFETY: name is NUL-terminated; fchmodat checks the directory fd and flags.
            let rc = unsafe { libc::fchmodat(dir_fd, name.as_ptr(), mode, flags) };
            (rc, !follow)
        }
    };
    if rc != 0 {
        let error = io::Error::last_os_error();
        if check_nofollow
            && error
                .raw_os_error()
                .is_some_and(|e| [libc::ENOTSUP, libc::EOPNOTSUPP].contains(&e))
        {
            return Err(if dir_fd == libc::AT_FDCWD {
                Error::new(
                    "NotImplementedError",
                    "chmod: follow_symlinks unavailable on this platform",
                )
            } else {
                Error::new(
                    "ValueError",
                    "chmod: cannot use dir_fd and follow_symlinks together",
                )
            });
        }
        let error = crate::objects::errno_error(error);
        return Err(ctx.with_errno_filename(error.into(), filename)?);
    }
    Ok(NONE)
}

/// Convert both arguments to C integers before auditing and changing the open inode.
pub(super) fn fchmod(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(fd), Some(mode)] =
        builtins::bind_builtin_arguments("fchmod", args, kwargs, ["fd", "mode"], 2)?
    else {
        unreachable!("fchmod requires fd and mode")
    };
    let fd = c_int(ctx, fd)?;
    let mode = c_int(ctx, mode)?;
    let event_args = [
        ctx.int(i64::from(fd)),
        ctx.int(i64::from(mode)),
        ctx.int(-1),
    ];
    audit::event(ctx, "os.chmod", &event_args)?;
    loop {
        if set_fd_mode(fd, mode as libc::mode_t) == 0 {
            return Ok(NONE);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(crate::objects::errno_error(error).into());
        }
    }
}

fn set_fd_mode(fd: i32, mode: libc::mode_t) -> i32 {
    // SAFETY: fchmod validates the descriptor and borrows it only for this call.
    unsafe { libc::fchmod(fd, mode) }
}
