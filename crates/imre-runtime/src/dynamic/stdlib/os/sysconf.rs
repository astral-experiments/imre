//! Linux configuration queries use target libc constants and names from the defining module.

use std::ffi::{CStr, c_char, c_int, c_long};

use super::{Context, Error, Id, Result, Value};

#[repr(C)]
struct Entry {
    name: *const c_char,
    code: c_int,
}

unsafe extern "C" {
    fn imre_platform_confstr_entries(length: *mut usize) -> *const Entry;
    fn imre_platform_sysconf_entries(length: *mut usize) -> *const Entry;
    fn imre_platform_pathconf_entries(length: *mut usize) -> *const Entry;
    fn imre_platform_pathconf(
        path: *const c_char,
        fd: c_int,
        name: c_int,
        error: *mut c_int,
    ) -> c_long;
    fn imre_platform_sysconf(name: c_int, error: *mut c_int) -> c_long;
}

pub(in crate::dynamic) fn initialize(ctx: &mut Context, module: Id, scope: Id) {
    for (name, table) in [
        (
            "sysconf_names",
            imre_platform_sysconf_entries as unsafe extern "C" fn(*mut usize) -> *const Entry,
        ),
        ("confstr_names", imre_platform_confstr_entries),
        ("pathconf_names", imre_platform_pathconf_entries),
    ] {
        let mut length = 0;
        // SAFETY: each generated table has static lifetime and contains length entries.
        let entries = unsafe {
            let ptr = table(&mut length);
            std::slice::from_raw_parts(ptr, length)
        };
        let names = entries
            .iter()
            .map(|entry| {
                // SAFETY: the generated table points at NUL-terminated ASCII string literals.
                let name = unsafe { CStr::from_ptr(entry.name) }
                    .to_str()
                    .expect("ASCII configuration name");
                (ctx.text(name), ctx.int(i64::from(entry.code)))
            })
            .collect();
        let names = ctx.dict(names);
        ctx.bind(scope, name, names);
    }
    for name in ["confstr", "sysconf", "pathconf", "fpathconf"] {
        let callable = ctx.builtin(&format!("posix.{name}"));
        let function = ctx.alloc(Value::Method {
            callable,
            receiver: module,
            explicit: false,
        });
        ctx.bind(scope, name, function);
    }
}

pub(in crate::dynamic::stdlib) fn call(
    ctx: &mut Context,
    func: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{func}() takes no keyword arguments"),
        ));
    }
    let [module, name] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "{func}() takes exactly one argument ({} given)",
                args.len().saturating_sub(1)
            ),
        ));
    };
    if func == "posix.confstr" {
        let code = confname(ctx, *module, *name, "confstr_names")?;
        let Some(bytes) = crate::objects::interpreter_stdlib::confstr_bytes(code)
            .map_err(super::super::os_error)?
        else {
            return Ok(crate::dynamic::NONE);
        };
        let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
        return Ok(ctx.text_codepoints(codec.decode_filesystem(&bytes)?));
    }
    let code = confname(ctx, *module, *name, "sysconf_names")?;
    let mut errno = 0;
    // SAFETY: the shim writes errno through a live c_int. -1 with errno zero is a valid result.
    let value = unsafe { imre_platform_sysconf(code, &mut errno) };
    if errno != 0 {
        return Err(crate::objects::errno_error(std::io::Error::from_raw_os_error(errno)).into());
    }
    Ok(ctx.int(value as i64))
}

/// Resolve names through the module's current table, or convert an integer code directly.
fn confname(ctx: &mut Context, module: Id, name: Id, table: &str) -> Result<c_int> {
    let name = if ctx.string_storage(name).is_some() {
        let names = ctx.lookup_attr(module, table)?;
        ctx.get_item(names, name)
            .map_err(|_| Error::new("ValueError", "unrecognized configuration name"))?
    } else {
        name
    };
    let code = ctx.index_value(name)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            "configuration names must be strings or integers",
        )
    })?;
    code.to_i64()
        .and_then(|n| c_int::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

/// Query the target limit with the path or file-descriptor conversion of the selected API.
pub(in crate::dynamic::stdlib) fn pathconf(
    ctx: &mut Context,
    func: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    use super::{PathArg, builtins, path_or_fd, stat};
    let Some((&module, operands)) = args.split_first() else {
        unreachable!("bound posix module")
    };
    let (path, spelling, code) = if func == "posix.pathconf" {
        let [Some(path), Some(name)] =
            builtins::bind_builtin_arguments("pathconf", operands, kwargs, ["path", "name"], 2)?
        else {
            unreachable!("missing arguments rejected")
        };
        let (path, spelling) = path_or_fd(ctx, "pathconf", path, true, stat::warn_bool_fd)?;
        (
            path,
            Some(spelling),
            confname(ctx, module, name, "pathconf_names")?,
        )
    } else {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "posix.fpathconf() takes no keyword arguments",
            ));
        }
        let [fd, name] = operands else {
            return Err(Error::new(
                "TypeError",
                format!("fpathconf expected 2 arguments, got {}", operands.len()),
            ));
        };
        stat::warn_bool_fd(ctx, *fd)?;
        let fd = super::super::select::fd(ctx, *fd)?;
        (
            PathArg::Fd(fd),
            None,
            confname(ctx, module, *name, "pathconf_names")?,
        )
    };
    let (pathname, descriptor) = match &path {
        PathArg::Name(path) => (path.as_ptr(), 0),
        PathArg::Fd(fd) => (std::ptr::null(), *fd),
    };
    let mut error = 0;
    // SAFETY: the optional C string and errno slot live through this single synchronous call.
    let value = unsafe { imre_platform_pathconf(pathname, descriptor, code, &mut error) };
    if error != 0 {
        let exc = super::super::os_error(std::io::Error::from_raw_os_error(error));
        if let Some(spelling) = spelling
            && error != libc::EINVAL
        {
            return Err(ctx.with_errno_filename(exc, spelling)?);
        }
        return Err(exc);
    }
    Ok(ctx.int(value as i64))
}
