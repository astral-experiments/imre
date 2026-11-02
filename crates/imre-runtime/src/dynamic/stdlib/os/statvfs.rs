//! Linux filesystem capacity queries share one result layout for paths and borrowed descriptors.

use super::{Context, Error, Id, PathArg, Result, builtins, c_int, path_or_fd, stat};
use std::{io, mem::MaybeUninit};

pub(in crate::dynamic::stdlib) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (arg, filename) = if name == "fstatvfs" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "posix.fstatvfs() takes no keyword arguments",
            ));
        }
        let [fd] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "posix.fstatvfs() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        (PathArg::Fd(c_int(ctx, *fd)?), None)
    } else {
        let [Some(path)] = builtins::bind_builtin_arguments("statvfs", args, kwargs, ["path"], 1)?
        else {
            unreachable!("required path")
        };
        let (arg, filename) = path_or_fd(ctx, name, path, true, stat::warn_bool_fd)?;
        // CPython's path converter reserves -1 for the pathname form.
        if matches!(arg, PathArg::Fd(-1)) {
            let error = crate::objects::errno_error(io::Error::from_raw_os_error(libc::EFAULT));
            return Err(ctx.with_errno_filename(error.into(), filename)?);
        }
        (arg, Some(filename))
    };
    // libc may leave reserved fields untouched. Start with initialized integer storage.
    let mut output = MaybeUninit::<libc::statvfs>::zeroed();
    loop {
        let rc = match &arg {
            PathArg::Name(path) => {
                // SAFETY: the owned pathname is NUL-terminated and the result buffer is writable.
                unsafe { libc::statvfs(path.as_ptr(), output.as_mut_ptr()) }
            }
            PathArg::Fd(fd) => {
                // SAFETY: the kernel validates the integer descriptor; this call never owns it.
                unsafe { libc::fstatvfs(*fd, output.as_mut_ptr()) }
            }
        };
        if rc == 0 {
            break;
        }
        let error = io::Error::last_os_error();
        if filename.is_none() && error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        let error = crate::objects::errno_error(error).into();
        return Err(match filename {
            Some(path) => ctx.with_errno_filename(error, path)?,
            None => error,
        });
    }
    // SAFETY: statvfs and fstatvfs initialize the structure on success.
    let s = unsafe { output.assume_init() };
    // CPython converts the ten tuple fields through C long/long long, but f_fsid is unsigned.
    let values = [
        s.f_bsize as libc::c_long as i64,
        s.f_frsize as libc::c_long as i64,
        s.f_blocks as i64,
        s.f_bfree as i64,
        s.f_bavail as i64,
        s.f_files as i64,
        s.f_ffree as i64,
        s.f_favail as i64,
        s.f_flag as libc::c_long as i64,
        s.f_namemax as libc::c_long as i64,
    ]
    .into_iter()
    .map(|n| ctx.int(n))
    .collect();
    let fsid = stat::integer(ctx, s.f_fsid);
    Ok(stat::wrap(ctx, &stat::VFS, values, vec![fsid]))
}
