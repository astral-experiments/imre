use super::{Context, Id, NONE, Result, native, path_and_dir_fd};

/// Remove an entry or an empty directory using the same path and dir_fd converters.
pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (path, pathname, dir_fd) = path_and_dir_fd(ctx, name, args, kwargs)?;
    match native::unlink_at(&pathname, dir_fd, name == "rmdir") {
        Ok(()) => Ok(NONE),
        Err(error) => {
            Err(ctx.with_errno_filename(crate::objects::errno_error(error).into(), path)?)
        }
    }
}
