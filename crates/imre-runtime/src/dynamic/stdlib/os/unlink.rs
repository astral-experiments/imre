use super::{Context, Error, Id, NONE, Result, Value, builtins, c_path, native, posixpath};

/// Convert an optional dir_fd after the path and preceding arguments.
fn directory_fd(ctx: &mut Context, value: Option<Id>) -> Result<i32> {
    let Some(value) = value.filter(|&fd| fd != NONE) else {
        return Ok(libc::AT_FDCWD);
    };
    if matches!(ctx.heap.get(value), Value::Bool(_)) {
        let location = ctx.heap.current_location();
        let (file, line) = location.as_ref().map_or(("<string>", 1), |loc| {
            (loc.file.as_ref(), loc.line as isize)
        });
        let module = &ctx.scope_data(ctx.heap.current_scope()).module;
        crate::objects::runtime_warning(module, file, line, "bool is used as a file descriptor")?;
    }
    let fd = ctx.index_value(value)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "argument should be integer or None, not {}",
                ctx.type_name(value)
            ),
        )
    })?;
    Ok(native::directory_fd_number(&fd)?)
}

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
