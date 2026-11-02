use std::{
    ffi::{CStr, CString},
    io, ptr,
};

use super::{
    Context, Error, Id, NONE, PathArg, Result, Value, builtins, c_int, c_path, path_or_fd,
    posixpath, stat,
};
use crate::dynamic::stdlib::audit;

pub(in crate::dynamic) const FUNCTIONS: [&str; 4] =
    ["getxattr", "setxattr", "listxattr", "removexattr"];

pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) {
    for name in FUNCTIONS {
        let function = ctx.builtin(&format!("posix.{name}"));
        ctx.bind(scope, name, function);
    }
    for (attr, n) in [
        ("XATTR_CREATE", libc::XATTR_CREATE),
        ("XATTR_REPLACE", libc::XATTR_REPLACE),
        ("XATTR_SIZE_MAX", 65536),
    ] {
        let value = ctx.int(i64::from(n));
        ctx.bind(scope, attr, value);
    }
}

fn bind<const N: usize>(
    op: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    names: [&str; N],
    required: usize,
) -> Result<[Option<Id>; N]> {
    // follow_symlinks is always the last and only keyword-only parameter.
    if args.len() >= N {
        builtins::bind_method_arguments(op, args, kwargs, names, N - 1)
    } else {
        builtins::bind_builtin_arguments(op, args, kwargs, names, required)
    }
}

fn attribute(ctx: &mut Context, op: &str, arg: Id) -> Result<(CString, Id)> {
    let name = posixpath::path_arg(
        ctx,
        arg,
        &format!("{op}: attribute should be string, bytes or os.PathLike"),
    )?;
    Ok((
        c_path(
            ctx,
            name,
            &format!("{op}: embedded null character in attribute"),
        )?,
        name,
    ))
}

fn follow(ctx: &mut Context, op: &str, path: &PathArg, arg: Option<Id>) -> Result<bool> {
    let follow = ctx.optional_truth(arg, true)?;
    if !follow && matches!(path, PathArg::Fd(fd) if *fd >= 0) {
        return Err(Error::new(
            "ValueError",
            format!("{op}: cannot use fd and follow_symlinks together"),
        ));
    }
    Ok(follow)
}

fn checked(ctx: &mut Context, count: isize, filename: Id) -> Result<usize> {
    if count >= 0 {
        return Ok(count as usize);
    }
    let error = crate::objects::errno_error(io::Error::last_os_error());
    Err(ctx.with_errno_filename(error.into(), filename)?)
}

fn read(
    ctx: &mut Context,
    filename: Id,
    first: usize,
    fetch: impl Fn(&mut [u8]) -> isize,
) -> Result<Vec<u8>> {
    let mut data = vec![0; first];
    let mut count = fetch(&mut data);
    if count < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ERANGE) {
        data.resize(65536, 0); // Linux XATTR_SIZE_MAX and XATTR_LIST_MAX.
        count = fetch(&mut data);
    }
    data.truncate(checked(ctx, count, filename)?);
    Ok(data)
}

fn address(arg: &PathArg, default: Option<&CStr>) -> (i32, *const libc::c_char) {
    match arg {
        PathArg::Name(path) => (-1, path.as_ptr()),
        PathArg::Fd(fd) => (*fd, default.map_or(ptr::null(), CStr::as_ptr)),
    }
}

/// Access Linux inode attributes through a pathname, symbolic link, or borrowed descriptor.
pub(in crate::dynamic::stdlib) fn call(
    ctx: &mut Context,
    op: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (path, attr, value, flags, follow_arg) = match op {
        "listxattr" => {
            let [p, f] = bind(op, args, kwargs, ["path", "follow_symlinks"], 0)?;
            (p.unwrap_or(NONE), None, None, None, f)
        }
        "getxattr" | "removexattr" => {
            let [p, a, f] = bind(
                op,
                args,
                kwargs,
                ["path", "attribute", "follow_symlinks"],
                2,
            )?;
            (p.unwrap(), a, None, None, f)
        }
        "setxattr" => {
            let [p, a, v, flags, f] = bind(
                op,
                args,
                kwargs,
                ["path", "attribute", "value", "flags", "follow_symlinks"],
                3,
            )?;
            (p.unwrap(), a, v, flags, f)
        }
        _ => unreachable!(),
    };
    let list = op == "listxattr";
    let (path, filename) = if list && path == NONE {
        (PathArg::Name(c".".into()), NONE)
    } else if list {
        super::convert_path(
            ctx,
            op,
            path,
            "string, bytes, os.PathLike, integer or None",
            Some(stat::warn_bool_fd),
        )?
    } else {
        path_or_fd(ctx, op, path, true, stat::warn_bool_fd)?
    };
    let attr = attr.map(|arg| attribute(ctx, op, arg)).transpose()?;
    // CPython uses the pathname form for negative descriptor numbers; listxattr defaults
    // a missing pathname to ".", while the other syscalls receive a null pathname.
    let (fd, name) = address(&path, list.then_some(c"."));
    if let Some(value) = value {
        let (attr, attr_obj) = attr.unwrap();
        ctx.check_buffer_exporter(value)?;
        ctx.native_buffer_bytes(value)?;
        return ctx.with_buffer_export(value, |ctx| {
            let flags = flags.map(|id| c_int(ctx, id)).transpose()?.unwrap_or(0);
            let follow = follow(ctx, op, &path, follow_arg)?;
            if !ctx.audit_hooks.is_empty() {
                let data = ctx.alloc(Value::Bytes(ctx.native_buffer_bytes(value)?.to_vec()));
                let flag_obj = ctx.int(i64::from(flags));
                audit::event(ctx, "os.setxattr", &[filename, attr_obj, data, flag_obj])?;
            }
            // A hook may change an exported bytearray's contents in place.
            let value = ctx.native_buffer_bytes(value)?;
            let buf = value.as_ptr().cast();
            let len = value.len();
            // SAFETY: the buffer and C strings outlive the synchronous call. The kernel
            // validates the borrowed descriptor or pathname, including the null case above.
            let rc = unsafe {
                if fd >= 0 {
                    libc::fsetxattr(fd, attr.as_ptr(), buf, len, flags)
                } else if follow {
                    libc::setxattr(name, attr.as_ptr(), buf, len, flags)
                } else {
                    libc::lsetxattr(name, attr.as_ptr(), buf, len, flags)
                }
            };
            checked(ctx, rc as isize, filename)?;
            Ok(NONE)
        });
    }
    let follow = follow(ctx, op, &path, follow_arg)?;
    if let Some((attr, attr_obj)) = attr {
        let args = [filename, attr_obj];
        if op == "removexattr" {
            audit::event(ctx, "os.removexattr", &args)?;
            // SAFETY: the C strings remain live and the kernel checks the descriptor/path.
            let rc = unsafe {
                if fd >= 0 {
                    libc::fremovexattr(fd, attr.as_ptr())
                } else if follow {
                    libc::removexattr(name, attr.as_ptr())
                } else {
                    libc::lremovexattr(name, attr.as_ptr())
                }
            };
            checked(ctx, rc as isize, filename)?;
            return Ok(NONE);
        }
        audit::event(ctx, "os.getxattr", &args)?;
        let data = read(ctx, filename, 128, |buf| {
            let out = buf.as_mut_ptr().cast();
            // SAFETY: the kernel writes at most buf.len() bytes into this live slice.
            unsafe {
                if fd >= 0 {
                    libc::fgetxattr(fd, attr.as_ptr(), out, buf.len())
                } else if follow {
                    libc::getxattr(name, attr.as_ptr(), out, buf.len())
                } else {
                    libc::lgetxattr(name, attr.as_ptr(), out, buf.len())
                }
            }
        })?;
        return Ok(ctx.alloc(Value::Bytes(data)));
    }
    audit::event(ctx, "os.listxattr", &[filename])?;
    let names = read(ctx, filename, 256, |buf| {
        let out = buf.as_mut_ptr().cast();
        // SAFETY: the kernel writes at most buf.len() bytes into this live slice.
        unsafe {
            if fd >= 0 {
                libc::flistxattr(fd, out, buf.len())
            } else if follow {
                libc::listxattr(name, out, buf.len())
            } else {
                libc::llistxattr(name, out, buf.len())
            }
        }
    })?;
    let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
    let mut result = Vec::new();
    for name in names.split(|&b| b == 0).filter(|s| !s.is_empty()) {
        result.push(ctx.text_codepoints(codec.decode_filesystem(name)?));
    }
    Ok(ctx.alloc(Value::List(result)))
}
