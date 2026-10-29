use std::ffi::CString;

use super::{Context, Error, Id, NONE, Result, Value, builtins, posixpath};
use crate::objects::os as native;

/// The posix module caches this dictionary. os.environ wraps the same byte keys and values.
pub(super) fn environment(ctx: &mut Context) -> Id {
    let entries = crate::objects::initial_environment()
        .into_iter()
        .map(|(key, value)| {
            let key = ctx.alloc(Value::Bytes(key));
            let value = ctx.alloc(Value::Bytes(value));
            (key, value)
        })
        .collect();
    ctx.dict(entries)
}

fn environment_argument(ctx: &mut Context, value: Id) -> Result<CString> {
    let path = posixpath::fspath(ctx, value)?;
    c_path(ctx, path, "embedded null byte")
}

/// Terminate the validated filesystem spelling before evaluating any later Python argument.
fn c_path(ctx: &Context, path: Id, nul_error: &str) -> Result<CString> {
    let bytes = match ctx.heap.get(ctx.scalar_value(path)) {
        Value::Str(text) => text.as_bytes(),
        Value::Bytes(bytes) => bytes,
        _ => unreachable!("fspath returns str or bytes"),
    };
    CString::new(bytes).map_err(|_| Error::new("ValueError", nul_error))
}

/// Direct putenv and unsetenv update libc. Only os.environ assignments also change the dictionary.
pub(super) fn change_environment(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let set = name == "posix.putenv";
    let expected = if set { 2 } else { 1 };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    if args.len() != expected {
        return Err(Error::new(
            "TypeError",
            if set {
                format!("putenv expected 2 arguments, got {}", args.len())
            } else {
                format!(
                    "posix.unsetenv() takes exactly one argument ({} given)",
                    args.len()
                )
            },
        ));
    }
    // posix.putenv can run before os is imported. Preserve the initial dictionary first.
    let posix = ctx.modules["posix"];
    ctx.lookup_attr(posix, "environ")?;
    let key = environment_argument(ctx, args[0])?;
    let status = if set {
        let value = environment_argument(ctx, args[1])?;
        if key.as_bytes().contains(&b'=') {
            return Err(Error::new(
                "ValueError",
                "illegal environment variable name",
            ));
        }
        // SAFETY: the standalone interpreter runs Python on one thread. setenv copies its inputs.
        unsafe { libc::setenv(key.as_ptr(), value.as_ptr(), 1) }
    } else {
        // SAFETY: key stays terminated and live through the synchronous unsetenv call.
        unsafe { libc::unsetenv(key.as_ptr()) }
    };
    if status != 0 {
        return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
    }
    Ok(NONE)
}

/// POSIX functions exported as the same builtins by both os and posix.
pub(super) fn is_descriptor_function(name: &str) -> bool {
    matches!(
        name,
        "open"
            | "dup"
            | "dup2"
            | "isatty"
            | "lseek"
            | "pipe"
            | "close"
            | "get_blocking"
            | "set_blocking"
            | "get_inheritable"
            | "set_inheritable"
    )
}

/// Convert an index argument before issuing the descriptor syscall.
fn c_int(ctx: &mut Context, value: Id) -> Result<i32> {
    ctx.index_integer(value)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

/// Bind and convert open arguments in Python order before opening the named entry.
fn open(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let names = ["path", "flags", "mode", "dir_fd"];
    let [Some(path), Some(flags), mode, dir_fd] = (if args.len() > 3 {
        builtins::bind_method_arguments("open", args, kwargs, names, 3)?
    } else {
        builtins::bind_builtin_arguments("open", args, kwargs, names, 2)?
    }) else {
        unreachable!("open requires path and flags")
    };
    let path = posixpath::path_arg(
        ctx,
        path,
        "open: path should be string, bytes or os.PathLike",
    )?;
    let pathname = c_path(ctx, path, "open: embedded null character in path")?;
    let flags = c_int(ctx, flags)?;
    let mode = mode
        .map(|value| c_int(ctx, value))
        .transpose()?
        .unwrap_or(0o777);
    let dir_fd = if let Some(value) = dir_fd.filter(|&fd| fd != NONE) {
        let fd = ctx.index_value(value)?.ok_or_else(|| {
            Error::new(
                "TypeError",
                format!(
                    "argument should be integer or None, not {}",
                    ctx.type_name(value)
                ),
            )
        })?;
        native::directory_fd_number(&fd)?
    } else {
        libc::AT_FDCWD
    };
    match native::open_at(&pathname, flags, mode, dir_fd) {
        Ok(fd) => Ok(ctx.int(i64::from(fd))),
        Err(error) => {
            Err(ctx.with_errno_filename(crate::objects::errno_error(error).into(), path)?)
        }
    }
}

/// Run the shared descriptor operation after binding its Python arguments.
pub(super) fn descriptor(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "open" {
        return open(ctx, args, kwargs);
    }
    if name == "dup2" {
        let [Some(fd), Some(fd2), inheritable] = builtins::bind_builtin_arguments(
            "dup2",
            args,
            kwargs,
            ["fd", "fd2", "inheritable"],
            2,
        )?
        else {
            unreachable!("dup2 requires both descriptors")
        };
        let fd = c_int(ctx, fd)?;
        let fd2 = c_int(ctx, fd2)?;
        let inheritable = inheritable
            .map(|value| ctx.truth(value))
            .transpose()?
            .unwrap_or(true);
        return Ok(ctx.int(i64::from(native::dup2(fd, fd2, inheritable)?)));
    }

    if name == "close" {
        let [Some(fd)] = super::builtins::bind_builtin_arguments("close", args, kwargs, ["fd"], 1)?
        else {
            unreachable!("fd is required")
        };
        native::close(c_int(ctx, fd)?)?;
        return Ok(NONE);
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("posix.{name}() takes no keyword arguments"),
        ));
    }
    if name == "pipe" {
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("posix.pipe() takes no arguments ({} given)", args.len()),
            ));
        }
        let mut result = Vec::new();
        result
            .try_reserve_exact(2)
            .map_err(|_| Error::new("MemoryError", ""))?;
        for fd in native::pipe(libc::O_CLOEXEC)? {
            result.push(ctx.int(i64::from(fd)));
        }
        return Ok(ctx.alloc(Value::Tuple(result)));
    }
    if name == "lseek" {
        let [fd, position, whence] = args else {
            return Err(Error::new(
                "TypeError",
                format!("lseek expected 3 arguments, got {}", args.len()),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        let position = ctx.index_integer(*position)?.to_i64().ok_or_else(|| {
            Error::new("OverflowError", "Python int too large to convert to C long")
        })?;
        let whence = c_int(ctx, *whence)?;
        return Ok(ctx.int(native::seek(fd, position, whence)?));
    }
    if matches!(name, "dup" | "isatty" | "get_blocking" | "get_inheritable") {
        let [fd] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "posix.{name}() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        return Ok(match name {
            "dup" => ctx.int(i64::from(native::dup(fd)?)),
            "isatty" => builtins::boolean(native::isatty(fd)),
            "get_blocking" => builtins::boolean(native::BLOCKING.enabled(fd)?),
            _ => builtins::boolean(native::INHERITABLE.enabled(fd)?),
        });
    }
    let flag = match name {
        "set_blocking" => &native::BLOCKING,
        "set_inheritable" => &native::INHERITABLE,
        _ => unreachable!(),
    };
    let [fd, value] = args else {
        return Err(Error::new(
            "TypeError",
            format!("{name} expected 2 arguments, got {}", args.len()),
        ));
    };
    let fd = c_int(ctx, *fd)?;
    let enabled = if name == "set_blocking" {
        ctx.truth(*value)?
    } else {
        c_int(ctx, *value)? != 0
    };
    flag.update(fd, enabled)?;
    Ok(NONE)
}
