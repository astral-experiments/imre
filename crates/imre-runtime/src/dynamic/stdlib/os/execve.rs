use std::io;

use super::{Context, Error, Id, PathArg, Result, builtins, path_or_fd, stat};
use crate::dynamic::stdlib::{audit, os_error, process_arguments};

/// Validate and audit a complete process image before replacing the caller.
pub(in crate::dynamic::stdlib) fn call(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [Some(path), Some(argv), Some(env)] =
        builtins::bind_builtin_arguments("execve", args, kwargs, ["path", "argv", "env"], 3)?
    else {
        unreachable!("execve requires path, argv, env")
    };
    let (path, filename) = path_or_fd(ctx, "execve", path, true, stat::warn_bool_fd)?;
    ctx.heap.keep(filename);
    let argc = process_arguments::argv_len(ctx, "execve", argv)?;
    if !ctx.has_mapping_subscription(env) {
        return Err(Error::new(
            "TypeError",
            "execve: environment must be a mapping object",
        ));
    }
    let arguments = process_arguments::argv(ctx, "execve", argv, argc)?;
    let environment = process_arguments::environment(ctx, env)?;
    audit::event(ctx, "os.exec", &[filename, argv, env])?;
    let error = match path {
        PathArg::Name(path) => arguments.execve(&path, &environment),
        PathArg::Fd(fd) if fd >= 0 => arguments.fexecve(fd, &environment),
        // CPython falls back to its null pathname for a negative descriptor.
        PathArg::Fd(_) => io::Error::from_raw_os_error(libc::EFAULT),
    };
    Err(ctx.with_errno_filename(os_error(error), filename)?)
}
