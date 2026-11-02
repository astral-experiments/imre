use super::{Context, Id, NONE, Result, builtins, c_path, directory_fd, native, posixpath};
use crate::dynamic::stdlib::audit;
use std::ffi::CString;

fn path_argument(ctx: &mut Context, value: Id, argument: &str) -> Result<(Id, CString)> {
    let path = posixpath::path_arg(
        ctx,
        value,
        &format!("symlink: {argument} should be string, bytes or os.PathLike"),
    )?;
    let spelling = c_path(
        ctx,
        path,
        &format!("symlink: embedded null character in {argument}"),
    )?;
    Ok((path, spelling))
}

pub(super) fn call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let names = ["src", "dst", "target_is_directory", "dir_fd"];
    let [Some(src), Some(dst), target_is_directory, dir_fd] = (if args.len() > 3 {
        builtins::bind_method_arguments("symlink", args, kwargs, names, 3)?
    } else {
        builtins::bind_builtin_arguments("symlink", args, kwargs, names, 2)?
    }) else {
        unreachable!("symlink requires src and dst")
    };
    let (src, source) = path_argument(ctx, src, "src")?;
    let (dst, destination) = path_argument(ctx, dst, "dst")?;
    if let Some(flag) = target_is_directory {
        ctx.truth(flag)?;
    }
    let dir_fd = directory_fd(ctx, dir_fd)?;
    let audit_fd = ctx.int(i64::from(if dir_fd == libc::AT_FDCWD {
        -1
    } else {
        dir_fd
    }));
    audit::event(ctx, "os.symlink", &[src, dst, audit_fd])?;
    if let Err(error) = native::symlink_at(&source, dir_fd, &destination) {
        return Err(ctx.with_errno_filenames(
            crate::objects::errno_error(error).into(),
            src,
            Some(dst),
        )?);
    }
    Ok(NONE)
}
