use super::{
    Context, Id, NONE, Result, audit_fd, builtins, c_path, directory_fd, native, posixpath,
};
use crate::dynamic::stdlib::audit;
use std::ffi::CString;

fn path_argument(
    ctx: &mut Context,
    name: &str,
    value: Id,
    argument: &str,
) -> Result<(Id, CString)> {
    let path = posixpath::path_arg(
        ctx,
        value,
        &format!("{name}: {argument} should be string, bytes or os.PathLike"),
    )?;
    let spelling = c_path(
        ctx,
        path,
        &format!("{name}: embedded null character in {argument}"),
    )?;
    Ok((path, spelling))
}

pub(super) fn symlink(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let names = ["src", "dst", "target_is_directory", "dir_fd"];
    let [Some(src), Some(dst), target_is_directory, dir_fd] = (if args.len() > 3 {
        builtins::bind_method_arguments("symlink", args, kwargs, names, 3)?
    } else {
        builtins::bind_builtin_arguments("symlink", args, kwargs, names, 2)?
    }) else {
        unreachable!("symlink requires src and dst")
    };
    let (src, source) = path_argument(ctx, "symlink", src, "src")?;
    let (dst, destination) = path_argument(ctx, "symlink", dst, "dst")?;
    if let Some(flag) = target_is_directory {
        ctx.truth(flag)?;
    }
    let dir_fd = directory_fd(ctx, dir_fd)?;
    let audit_fd = audit_fd(ctx, dir_fd);
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

/// Bind both paths and descriptors in signature order before auditing the hard link.
pub(super) fn link(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let names = ["src", "dst", "src_dir_fd", "dst_dir_fd", "follow_symlinks"];
    let [Some(src), Some(dst), src_fd, dst_fd, follow] =
        super::bind_two_positional("link", args, kwargs, names)?
    else {
        unreachable!("link requires src and dst")
    };
    let (src, source) = path_argument(ctx, "link", src, "src")?;
    let (dst, destination) = path_argument(ctx, "link", dst, "dst")?;
    let src_fd = directory_fd(ctx, src_fd)?;
    let dst_fd = directory_fd(ctx, dst_fd)?;
    let follow = follow.map(|v| ctx.truth(v)).transpose()?.unwrap_or(true);
    let audit_src_fd = audit_fd(ctx, src_fd);
    let audit_dst_fd = audit_fd(ctx, dst_fd);
    audit::event(ctx, "os.link", &[src, dst, audit_src_fd, audit_dst_fd])?;
    if let Err(error) = native::link_at(src_fd, &source, dst_fd, &destination, follow) {
        return Err(ctx.with_errno_filenames(
            crate::objects::errno_error(error).into(),
            src,
            Some(dst),
        )?);
    }
    Ok(NONE)
}

/// Move a directory entry atomically after auditing both paths and their directory descriptors.
pub(super) fn rename(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let names = ["src", "dst", "src_dir_fd", "dst_dir_fd"];
    let [Some(src), Some(dst), src_fd, dst_fd] =
        super::bind_two_positional(name, args, kwargs, names)?
    else {
        unreachable!("rename and replace require src and dst")
    };
    let (src, source) = path_argument(ctx, name, src, "src")?;
    let (dst, destination) = path_argument(ctx, name, dst, "dst")?;
    let src_fd = directory_fd(ctx, src_fd)?;
    let dst_fd = directory_fd(ctx, dst_fd)?;
    let audit_src_fd = audit_fd(ctx, src_fd);
    let audit_dst_fd = audit_fd(ctx, dst_fd);
    audit::event(ctx, "os.rename", &[src, dst, audit_src_fd, audit_dst_fd])?;

    // SAFETY: both C strings live through the syscall. The kernel validates the raw directory fds.
    let result = unsafe {
        if src_fd == libc::AT_FDCWD && dst_fd == libc::AT_FDCWD {
            libc::rename(source.as_ptr(), destination.as_ptr())
        } else {
            libc::renameat(src_fd, source.as_ptr(), dst_fd, destination.as_ptr())
        }
    };
    if result < 0 {
        let error = crate::objects::errno_error(std::io::Error::last_os_error());
        return Err(ctx.with_errno_filenames(error.into(), src, Some(dst))?);
    }
    Ok(NONE)
}
