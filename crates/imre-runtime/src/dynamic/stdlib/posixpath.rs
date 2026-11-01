use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use super::{Context, Error, Id, NONE, Result, Value};

fn spelling(ctx: &Context, value: Id) -> bool {
    matches!(
        ctx.heap.get(ctx.scalar_value(value)),
        Value::Str(_) | Value::SurrogateStr(_) | Value::Bytes(_)
    )
}

/// Resolve the special method on the type, preserving str and bytes subclass identity.
pub(super) fn fspath(ctx: &mut Context, path: Id) -> Result<Id> {
    path_arg(ctx, path, "expected str, bytes or os.PathLike object")
}

pub(super) fn path_arg(ctx: &mut Context, path: Id, expected: &str) -> Result<Id> {
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
    Err(Error::new(
        "TypeError",
        format!("{expected}, not {}", ctx.type_name(path)),
    ))
}

/// Normalize the filesystem spelling lexically, preserving raw bytes and returning a base str or bytes.
fn normpath(ctx: &mut Context, path: Id) -> Result<Id> {
    let path = path_arg(
        ctx,
        path,
        "_path_normpath: path should be string, bytes or os.PathLike",
    )?;
    let value = ctx.heap.get(ctx.scalar_value(path));
    if let Value::SurrogateStr(points) = value {
        let normalized = crate::objects::posixpath::normalize(points, None)
            .map_err(|_| Error::new("MemoryError", ""))?;
        return Ok(ctx.text_codepoints(normalized));
    }
    let bytes = match value {
        Value::Str(text) => text.as_bytes(),
        Value::Bytes(bytes) => bytes,
        _ => unreachable!("filesystem spelling"),
    };
    let normalized = crate::objects::posixpath::normalize(bytes, None)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let result = match value {
        Value::Str(_) => Value::Str(String::from_utf8(normalized).expect("whole UTF-8 components")),
        _ => Value::Bytes(normalized),
    };
    Ok(ctx.alloc(result))
}

fn stat_predicate(ctx: &mut Context, path: Id, operation: &str) -> Result<bool> {
    let lstat = operation == "islink";
    if !lstat
        && !spelling(ctx, path)
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
                && (operation == "exists"
                    || stat.assume_init().st_mode & libc::S_IFMT == libc::S_IFREG)
        });
    }
    let value = path_arg(
        ctx,
        path,
        if lstat {
            "lstat: path should be string, bytes or os.PathLike"
        } else {
            "stat: path should be string, bytes, os.PathLike or integer"
        },
    )?;
    let encoded;
    let path = match ctx.heap.get(ctx.scalar_value(value)) {
        Value::Str(text) => Path::new(text),
        Value::SurrogateStr(_) => {
            encoded = ctx.encode_filesystem_string(value)?;
            Path::new(OsStr::from_bytes(&encoded))
        }
        Value::Bytes(bytes) => Path::new(OsStr::from_bytes(bytes)),
        _ => unreachable!(),
    };
    Ok(match operation {
        "exists" => crate::objects::exists(path),
        "isfile" => crate::objects::is_file(path),
        "islink" => std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()),
        _ => unreachable!("registered path predicate"),
    })
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if let Some(function) = name.strip_prefix("posix.") {
        let count = args.len() + kwargs.len();
        if count > 1 {
            let kind = if args.is_empty() { "keyword " } else { "" };
            return Err(Error::new(
                "TypeError",
                format!("{function}() takes at most 1 {kind}argument ({count} given)"),
            ));
        }
        let path = super::option(args, 0, kwargs, "path")?.ok_or_else(|| {
            Error::new(
                "TypeError",
                format!("{function}() missing required argument 'path' (pos 1)"),
            )
        })?;
        return match function {
            "fspath" => fspath(ctx, path),
            "_path_normpath" => normpath(ctx, path),
            _ => unreachable!("registered POSIX path builtin"),
        };
    }
    let [path] = args else {
        return Err(Error::new("TypeError", "expected one path argument"));
    };
    let path = *path;
    match name.strip_prefix("_imre_posixpath.") {
        Some("extension_start") => {
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
        Some(op @ ("exists" | "isfile" | "islink")) => Ok(super::super::builtins::boolean(
            stat_predicate(ctx, path, op)?,
        )),
        _ => unreachable!(),
    }
}
