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
    if let Value::DirEntry(entry) = ctx.heap.get(path) {
        return Ok(entry.path);
    }
    if spelling(ctx, path) {
        return Ok(path);
    }
    // PyOS_FSPath replaces lookup errors, but propagates errors raised by the call.
    let method = ctx.bound_slot(path, "__fspath__").ok().flatten();
    if let Some(method) = method.filter(|&method| method != NONE) {
        let value = ctx.call(method, Vec::new(), Vec::new())?;
        if spelling(ctx, value) {
            // A native slot can return a borrowed value. Keep it through later argument conversions.
            ctx.heap.keep(value);
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
        Value::Str(_) => Value::Str(
            String::from_utf8(normalized)
                .expect("whole UTF-8 components")
                .into(),
        ),
        _ => Value::Bytes(normalized),
    };
    Ok(ctx.alloc(result))
}

fn splitroot(ctx: &mut Context, path: Id) -> Result<Id> {
    use crate::objects::posixpath::root_length;

    let path = path_arg(
        ctx,
        path,
        "_path_splitroot_ex: path should be string, bytes or os.PathLike",
    )?;
    let path = ctx.scalar_value(path);
    let (cut, drive) = match ctx.heap.get(path) {
        Value::Str(text) => (root_length(text.as_bytes()), ctx.text("")),
        Value::SurrogateStr(points) => (root_length(points), ctx.text("")),
        Value::Bytes(bytes) => (root_length(bytes), ctx.alloc(Value::Bytes(Vec::new()))),
        _ => unreachable!("filesystem spelling"),
    };
    let cut = ctx.int(cut as i64);
    let prefix = ctx.alloc(Value::Slice([NONE, cut, NONE]));
    let suffix = ctx.alloc(Value::Slice([cut, NONE, NONE]));
    let root = ctx.get_item(path, prefix)?;
    let tail = ctx.get_item(path, suffix)?;
    Ok(ctx.alloc(Value::Tuple(vec![drive, root, tail])))
}

/// Join unmatched components of two normalized POSIX spellings into a relative path.
fn relative_parts<T: Copy + Eq + From<u8>>(path: &[T], start: &[T]) -> Vec<T> {
    let slash = T::from(b'/');
    let dot = T::from(b'.');
    let mut path = path
        .split(|&c| c == slash)
        .filter(|s| !s.is_empty())
        .peekable();
    let mut start = start
        .split(|&c| c == slash)
        .filter(|s| !s.is_empty())
        .peekable();
    while path.peek().is_some() && path.peek() == start.peek() {
        path.next();
        start.next();
    }
    let mut result = Vec::new();
    let parent = [dot, dot];
    for part in start.map(|_| parent.as_slice()).chain(path) {
        if !result.is_empty() {
            result.push(slash);
        }
        result.extend_from_slice(part);
    }
    if result.is_empty() {
        result.push(dot);
    }
    result
}

/// POSIX spelling operations for realpath preserve Python code points until each filesystem lookup.
fn realpath_op(ctx: &mut Context, op: &str, args: &[Id]) -> Result<Id> {
    use std::os::unix::ffi::OsStrExt;

    let [path] = args else {
        return Err(Error::new("TypeError", "expected one path argument"));
    };
    let path = fspath(ctx, *path)?;
    let raw = matches!(ctx.heap.get(ctx.scalar_value(path)), Value::Bytes(_));
    if op == "parts" || op == "parent" {
        let chars: Vec<u32> = if raw {
            ctx.bytes_like(path)?
                .iter()
                .map(|&c| u32::from(c))
                .collect()
        } else {
            ctx.string_codepoints(path)?.into_owned()
        };
        let spelling = |ctx: &mut Context, part: &[u32]| {
            if raw {
                ctx.alloc(Value::Bytes(part.iter().map(|&c| c as u8).collect()))
            } else {
                ctx.text_codepoints(part.to_vec())
            }
        };
        if op == "parent" {
            let end = chars
                .iter()
                .rposition(|&c| c == u32::from(b'/'))
                .unwrap_or(0)
                .max(1);
            return Ok(spelling(ctx, &chars[..end.min(chars.len())]));
        }
        let parts = chars
            .split(|&c| c == u32::from(b'/'))
            .map(|part| spelling(ctx, part))
            .collect();
        return Ok(ctx.alloc(Value::List(parts)));
    }
    let errno = |ctx: &mut Context, error| {
        let error = crate::objects::errno_error(error).into();
        ctx.with_errno_filename(error, path)
            .unwrap_or_else(|error| error)
    };
    if matches!(op, "loop" | "notdir") {
        let code = if op == "loop" {
            libc::ELOOP
        } else {
            libc::ENOTDIR
        };
        return Err(errno(ctx, std::io::Error::from_raw_os_error(code)));
    }
    let nul_error = if op == "kind" {
        "lstat: embedded null character in path"
    } else {
        "readlink: embedded null character in path"
    };
    let native = super::os::c_path(ctx, path, nul_error)?;
    let native = std::ffi::OsStr::from_bytes(native.as_bytes());
    if op == "kind" {
        let metadata = std::fs::symlink_metadata(native).map_err(|e| errno(ctx, e))?;
        return Ok(ctx.text(if metadata.is_symlink() {
            "link"
        } else if metadata.is_dir() {
            "dir"
        } else {
            "other"
        }));
    }
    let target = std::fs::read_link(native).map_err(|e| errno(ctx, e))?;
    let bytes = target.as_os_str().as_bytes();
    Ok(if raw {
        ctx.alloc(Value::Bytes(bytes.to_vec()))
    } else {
        ctx.text_codepoints(crate::objects::interpreter_stdlib::fsdecode(bytes).collect())
    })
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if let Some(op) = name.strip_prefix("_imre_posixpath.realpath_") {
        return realpath_op(ctx, op, args);
    }
    if name == "_imre_posixpath.relative_parts" {
        let [path, start] = args else {
            return Err(Error::new("TypeError", "expected two path arguments"));
        };
        return match (ctx.heap.get(*path), ctx.heap.get(*start)) {
            (Value::Bytes(path), Value::Bytes(start)) => {
                let result = relative_parts(path, start);
                Ok(ctx.alloc(Value::Bytes(result)))
            }
            _ => {
                let result = relative_parts(
                    &ctx.string_codepoints(*path)?,
                    &ctx.string_codepoints(*start)?,
                );
                Ok(ctx.text_codepoints(result))
            }
        };
    }
    if let Some(function) = name.strip_prefix("posix.") {
        let count = args.len() + kwargs.len();
        if count > 1 {
            let kind = if args.is_empty() { "keyword " } else { "" };
            return Err(Error::new(
                "TypeError",
                format!("{function}() takes at most 1 {kind}argument ({count} given)"),
            ));
        }
        let parameter = if function == "_path_splitroot_ex" {
            "p"
        } else {
            "path"
        };
        let path = super::option(args, 0, kwargs, parameter)?.ok_or_else(|| {
            Error::new(
                "TypeError",
                format!("{function}() missing required argument '{parameter}' (pos 1)"),
            )
        })?;
        return match function {
            "fspath" => fspath(ctx, path),
            "_path_normpath" => normpath(ctx, path),
            "_path_splitroot_ex" => splitroot(ctx, path),
            _ => unreachable!("registered POSIX path builtin"),
        };
    }
    let [path] = args else {
        return Err(Error::new("TypeError", "expected one path argument"));
    };
    let path = *path;
    debug_assert_eq!(name, "_imre_posixpath.extension_start");
    let cut = match ctx.heap.get(path) {
        Value::Str(text) => text[..crate::objects::posixpath::extension_start(text.as_bytes())]
            .chars()
            .count(),
        Value::Bytes(bytes) => crate::objects::posixpath::extension_start(bytes),
        _ => return Err(Error::new("TypeError", "expected str or bytes")),
    };
    Ok(ctx.int(cut as i64))
}
