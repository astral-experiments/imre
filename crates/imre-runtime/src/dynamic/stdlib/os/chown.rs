use std::io;

use super::{
    Context, Error, Id, NONE, PathArg, Result, builtins, c_int, directory_fd, native, path_or_fd,
    stat,
};
use crate::dynamic::stdlib::audit;

pub(in crate::dynamic::stdlib) fn owner_id(ctx: &mut Context, value: Id, group: bool) -> Result<u64> {
    let index = ctx.index_integer(value).map_err(|_| {
        let which = if group { "gid" } else { "uid" };
        Error::new(
            "TypeError",
            format!("{which} should be integer, not {}", ctx.type_name(value)),
        )
    })?;
    Ok(native::credential_id(&index, group)?)
}

/// Convert ownership arguments and audit them before changing the inode selected by the caller.
pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let bound = if name == "chown" {
        let names = ["path", "uid", "gid", "dir_fd", "follow_symlinks"];
        if args.len() > 3 {
            builtins::bind_method_arguments(name, args, kwargs, names, 3)?
        } else {
            builtins::bind_builtin_arguments(name, args, kwargs, names, 3)?
        }
    } else {
        let target = if name == "fchown" { "fd" } else { "path" };
        let [path, uid, gid] =
            builtins::bind_builtin_arguments(name, args, kwargs, [target, "uid", "gid"], 3)?;
        [path, uid, gid, None, None]
    };
    let [Some(path), Some(uid), Some(gid), dir_fd, follow] = bound else {
        unreachable!("chown functions require a target, uid and gid")
    };
    let (path, filename) = if name == "fchown" {
        let fd = c_int(ctx, path)?;
        (PathArg::Fd(fd), ctx.int(i64::from(fd)))
    } else {
        path_or_fd(ctx, name, path, name == "chown", stat::warn_bool_fd)?
    };
    let uid = owner_id(ctx, uid, false)? as libc::uid_t;
    let gid = owner_id(ctx, gid, true)? as libc::gid_t;
    let dir_fd = directory_fd(ctx, dir_fd)?;
    let follow = ctx.optional_truth(follow, name != "lchown")?;
    if let PathArg::Fd(fd) = path {
        if fd != -1 && dir_fd != libc::AT_FDCWD {
            return Err(Error::new(
                "ValueError",
                "chown: can't specify both dir_fd and fd",
            ));
        }
        if fd >= 0 && !follow {
            return Err(Error::new(
                "ValueError",
                "chown: cannot use fd and follow_symlinks together",
            ));
        }
    }
    let audit_args = [
        filename,
        ctx.int(i64::from(uid)),
        ctx.int(i64::from(gid)),
        ctx.int(i64::from(if dir_fd == libc::AT_FDCWD {
            -1
        } else {
            dir_fd
        })),
    ];
    audit::event(ctx, "os.chown", &audit_args)?;
    loop {
        let rc = match &path {
            // CPython reserves -1 for a pathname, but an integer supplies no pathname.
            PathArg::Fd(-1) if name == "chown" => {
                let error = crate::objects::errno_error(io::Error::from_raw_os_error(libc::EFAULT));
                return Err(ctx.with_errno_filename(error.into(), filename)?);
            }
            // SAFETY: fchown borrows the descriptor and validates it in the kernel.
            PathArg::Fd(fd) => unsafe { libc::fchown(*fd, uid, gid) },
            // SAFETY: the owned C string is NUL-terminated and lives through the call.
            PathArg::Name(path) => unsafe {
                if dir_fd != libc::AT_FDCWD {
                    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
                    libc::fchownat(dir_fd, path.as_ptr(), uid, gid, flags)
                } else if follow {
                    libc::chown(path.as_ptr(), uid, gid)
                } else {
                    libc::lchown(path.as_ptr(), uid, gid)
                }
            },
        };
        if rc == 0 {
            return Ok(NONE);
        }
        let error = io::Error::last_os_error();
        if name == "fchown" {
            if error.kind() == io::ErrorKind::Interrupted {
                ctx.dispatch_signals()?;
                continue;
            }
            return Err(crate::objects::errno_error(error).into());
        }
        return Err(ctx.with_errno_filename(crate::objects::errno_error(error).into(), filename)?);
    }
}
