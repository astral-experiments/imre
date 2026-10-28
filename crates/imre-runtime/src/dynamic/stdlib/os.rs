use std::ffi::CString;

use super::{Context, Error, Id, NONE, Result, Value, posixpath};

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
    let bytes = match ctx.heap.get(ctx.scalar_value(path)) {
        Value::Str(text) => text.as_bytes(),
        Value::Bytes(bytes) => bytes,
        _ => unreachable!("fspath returns str or bytes"),
    };
    CString::new(bytes).map_err(|_| Error::new("ValueError", "embedded null byte"))
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
        "pipe" | "close" | "get_blocking" | "set_blocking" | "get_inheritable" | "set_inheritable"
    )
}

/// Convert an index argument before issuing the descriptor syscall.
fn c_int(ctx: &mut Context, value: Id) -> Result<i32> {
    ctx.index_integer(value)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

/// Run the shared descriptor operation after binding its Python arguments.
pub(super) fn descriptor(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    use crate::objects::os as native;

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
    let flag = match name {
        "get_blocking" | "set_blocking" => &native::BLOCKING,
        "get_inheritable" | "set_inheritable" => &native::INHERITABLE,
        _ => unreachable!(),
    };
    if name.starts_with("get_") {
        let [fd] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "posix.{name}() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        return Ok(super::builtins::boolean(flag.enabled(c_int(ctx, *fd)?)?));
    }
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
