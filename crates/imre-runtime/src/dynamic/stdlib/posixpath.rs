use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use super::{Context, Error, Id, NONE, Result, Value};

fn spelling(ctx: &Context, value: Id) -> bool {
    matches!(
        ctx.heap.get(ctx.scalar_value(value)),
        Value::Str(_) | Value::Bytes(_)
    )
}

/// Resolve the special method on the type, preserving str and bytes subclass identity.
pub(super) fn fspath(ctx: &mut Context, path: Id) -> Result<Id> {
    path_arg(ctx, path, false)
}

fn path_arg(ctx: &mut Context, path: Id, stat: bool) -> Result<Id> {
    if spelling(ctx, path) {
        return Ok(path);
    }
    // PyOS_FSPath replaces lookup errors, but propagates errors raised by the call.
    let method = ctx.bound_slot(path, "__fspath__").ok().flatten();
    if let Some(method) = method.filter(|&method| method != NONE) {
        let value = ctx.call(method, Vec::new(), Vec::new())?;
        if spelling(ctx, value) {
            return Ok(value);
        }
        return Err(Error::new(
            "TypeError",
            format!(
                "expected {}.__fspath__() to return str or bytes, not {}",
                ctx.type_name(path),
                ctx.type_name(value)
            ),
        ));
    }
    let expected = if stat {
        "stat: path should be string, bytes, os.PathLike or integer"
    } else {
        "expected str, bytes or os.PathLike object"
    };
    Err(Error::new(
        "TypeError",
        format!("{expected}, not {}", ctx.type_name(path)),
    ))
}

fn isfile(ctx: &mut Context, path: Id) -> Result<bool> {
    if !spelling(ctx, path)
        && let Some(index) = ctx.index_value(path)?
    {
        let fd = index
            .to_i64()
            .and_then(|n| libc::c_int::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    if index.big().sign() == num_bigint::Sign::Minus {
                        "fd is less than minimum"
                    } else {
                        "fd is greater than maximum"
                    },
                )
            })?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstat initializes the full buffer on success and never takes ownership of fd.
        return Ok(unsafe {
            libc::fstat(fd, stat.as_mut_ptr()) == 0
                && stat.assume_init().st_mode & libc::S_IFMT == libc::S_IFREG
        });
    }
    let value = path_arg(ctx, path, true)?;
    let path = match ctx.heap.get(ctx.scalar_value(value)) {
        Value::Str(text) => Path::new(text),
        Value::Bytes(bytes) => Path::new(OsStr::from_bytes(bytes)),
        _ => unreachable!(),
    };
    Ok(crate::objects::is_file(path))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "posix.fspath" {
        let count = args.len() + kwargs.len();
        if count > 1 {
            let kind = if args.is_empty() { "keyword " } else { "" };
            return Err(Error::new(
                "TypeError",
                format!("fspath() takes at most 1 {kind}argument ({count} given)"),
            ));
        }
        let path = super::option(args, 0, kwargs, "path")?.ok_or_else(|| {
            Error::new(
                "TypeError",
                "fspath() missing required argument 'path' (pos 1)",
            )
        })?;
        return fspath(ctx, path);
    }
    let [path] = args else {
        return Err(Error::new("TypeError", "expected one path argument"));
    };
    let path = *path;
    match name {
        "_imre_posixpath.extension_start" => {
            let cut = match ctx.heap.get(path) {
                Value::Str(text) => text
                    [..crate::objects::posixpath::extension_start(text.as_bytes())]
                    .chars()
                    .count(),
                Value::Bytes(bytes) => crate::objects::posixpath::extension_start(bytes),
                _ => return Err(Error::new("TypeError", "expected str or bytes")),
            };
            Ok(ctx.int(cut as i64))
        }
        "_imre_posixpath.isfile" => Ok(super::super::builtins::boolean(isfile(ctx, path)?)),
        _ => unreachable!(),
    }
}
