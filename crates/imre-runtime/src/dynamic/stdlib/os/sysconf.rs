//! Linux sysconf uses target libc constants and the names mapping on the defining module.

use std::ffi::{CStr, c_char, c_int, c_long};

use super::{Context, Error, Id, Result, Value};

#[repr(C)]
struct Entry {
    name: *const c_char,
    code: c_int,
}

unsafe extern "C" {
    fn imre_platform_sysconf_entries(length: *mut usize) -> *const Entry;
    fn imre_platform_sysconf(name: c_int, error: *mut c_int) -> c_long;
}

pub(in crate::dynamic) fn initialize(ctx: &mut Context, module: Id, scope: Id) {
    let mut length = 0;
    // SAFETY: the compiled target table has static lifetime and contains length entries.
    let entries = unsafe {
        let ptr = imre_platform_sysconf_entries(&mut length);
        std::slice::from_raw_parts(ptr, length)
    };
    let names = entries
        .iter()
        .map(|entry| {
            // SAFETY: the generated table points at NUL-terminated ASCII string literals.
            let name = unsafe { CStr::from_ptr(entry.name) }
                .to_str()
                .expect("ASCII sysconf name");
            (ctx.text(name), ctx.int(i64::from(entry.code)))
        })
        .collect();
    let names = ctx.dict(names);
    ctx.bind(scope, "sysconf_names", names);
    let callable = ctx.builtin("posix.sysconf");
    let function = ctx.alloc(Value::Method {
        callable,
        receiver: module,
        explicit: false,
    });
    ctx.bind(scope, "sysconf", function);
}

pub(in crate::dynamic::stdlib) fn call(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "posix.sysconf() takes no keyword arguments",
        ));
    }
    let [module, name] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "posix.sysconf() takes exactly one argument ({} given)",
                args.len().saturating_sub(1)
            ),
        ));
    };
    let name = if ctx.string_storage(*name).is_some() {
        let names = ctx.lookup_attr(*module, "sysconf_names")?;
        ctx.get_item(names, *name)
            .map_err(|_| Error::new("ValueError", "unrecognized configuration name"))?
    } else {
        *name
    };
    let code = ctx.index_value(name)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            "configuration names must be strings or integers",
        )
    })?;
    let code = code
        .to_i64()
        .and_then(|n| c_int::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))?;
    let mut errno = 0;
    // SAFETY: the shim writes errno through a live c_int. -1 with errno zero is a valid result.
    let value = unsafe { imre_platform_sysconf(code, &mut errno) };
    if errno != 0 {
        return Err(crate::objects::errno_error(std::io::Error::from_raw_os_error(errno)).into());
    }
    Ok(ctx.int(value as i64))
}
