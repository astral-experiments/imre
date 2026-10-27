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
