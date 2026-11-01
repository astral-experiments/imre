use super::{Context, Error, Id, NONE, Result, builtins, c_path, directory_fd, native, posixpath};

/// Remove an entry or an empty directory using the same path and dir_fd converters.
pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [Some(path), dir_fd] =
        builtins::bind_builtin_arguments(name, args, kwargs, ["path", "dir_fd"], 1)?
    else {
        unreachable!("all three functions require path")
    };
    if args.len() > 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes exactly 1 positional argument ({} given)",
                args.len()
            ),
        ));
    }
    let path = posixpath::path_arg(
        ctx,
        path,
        &format!("{name}: path should be string, bytes or os.PathLike"),
    )?;
    let pathname = c_path(
        ctx,
        path,
        &format!("{name}: embedded null character in path"),
    )?;
    let dir_fd = directory_fd(ctx, dir_fd)?;
    match native::unlink_at(&pathname, dir_fd, name == "rmdir") {
        Ok(()) => Ok(NONE),
        Err(error) => {
            Err(ctx.with_errno_filename(crate::objects::errno_error(error).into(), path)?)
        }
    }
}
