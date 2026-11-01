pub(in crate::dynamic) mod scandir;
pub(in crate::dynamic) mod stat;

use std::ffi::CString;

use super::{Context, Error, Id, NONE, Result, Value, builtins, posixpath};
use crate::objects::os as native;

/// Publish the supported POSIX operations from CPython's Lib/os.py capability tables whose
/// descriptor and no-follow forms are implemented by the boxed runtime.
#[cfg(unix)]
pub(in crate::dynamic) fn install_capabilities(
    ctx: &mut Context,
    module: Id,
    scope: Id,
) -> Result<()> {
    for (capability, functions) in [
        (
            "supports_dir_fd",
            &["lstat", "open", "rmdir", "stat", "unlink"][..],
        ),
        ("supports_fd", &["listdir", "scandir", "stat"][..]),
        ("supports_follow_symlinks", &["stat"][..]),
        ("supports_effective_ids", &[][..]),
    ] {
        let set = ctx.alloc(Value::Set {
            entries: Default::default(),
            frozen: false,
        });
        for &function in functions {
            let value = ctx.lookup_attr(module, function)?;
            builtins::set_add(ctx, set, value)?;
        }
        ctx.bind(scope, capability, set);
    }
    Ok(())
}

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

/// Fetch a fresh kernel pathname on each call, preserving all POSIX filename bytes.
pub(super) fn getcwd(ctx: &mut Context, as_bytes: bool) -> Result<Id> {
    let bytes = std::env::current_dir()
        .map_err(crate::objects::errno_error)?
        .into_os_string()
        .into_encoded_bytes();
    if as_bytes {
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
    Ok(ctx.text_codepoints(codec.decode_filesystem(&bytes)?))
}

fn environment_argument(ctx: &mut Context, value: Id) -> Result<CString> {
    let path = posixpath::fspath(ctx, value)?;
    c_path(ctx, path, "embedded null byte")
}

/// Terminate the validated filesystem spelling before evaluating any later Python argument.
pub(super) fn c_path(ctx: &mut Context, path: Id, nul_error: &str) -> Result<CString> {
    let encoded = match ctx.heap.get(ctx.scalar_value(path)) {
        Value::Bytes(bytes) => CString::new(bytes.as_slice()),
        Value::Str(_) | Value::SurrogateStr(_) => CString::new(ctx.encode_os_path(path)?),
        _ => unreachable!("fspath returns str or bytes"),
    };
    encoded.map_err(|_| Error::new("ValueError", nul_error))
}

/// Read cryptographically secure bytes from the operating system.
pub(super) fn urandom(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "posix.urandom() takes no keyword arguments",
        ));
    }
    let [size] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "posix.urandom() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    };
    let size = ctx
        .index_integer(*size)?
        .to_i64()
        .filter(|&n| isize::try_from(n).is_ok())
        .ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?;
    if size < 0 {
        return Err(Error::new("ValueError", "negative argument not allowed"));
    }
    let mut data = builtins::zeroed_bytes(size)?;
    getrandom::fill(&mut data).map_err(|e| crate::objects::errno_error(e.into()))?;
    Ok(ctx.alloc(Value::Bytes(data)))
}

/// Read directory entries in filesystem order, preserving the caller's bytes or text spelling.
pub(super) fn listdir(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    use std::os::fd::{AsFd, FromRawFd, OwnedFd};

    use rustix::fs::{Dir, Mode, OFlags, SeekFrom};

    let count = args.len() + kwargs.len();
    if count > 1 {
        let keyword = if args.is_empty() { "keyword " } else { "" };
        return Err(Error::new(
            "TypeError",
            format!("listdir() takes at most 1 {keyword}argument ({count} given)"),
        ));
    }
    if let Some((key, _)) = kwargs.first()
        && key != "path"
    {
        return Err(Error::new(
            "TypeError",
            format!("listdir() got an unexpected keyword argument '{key}'"),
        ));
    }
    let input = super::option(args, 0, kwargs, "path")?.unwrap_or(NONE);
    let spelling = matches!(
        ctx.heap.get(ctx.scalar_value(input)),
        Value::Str(_) | Value::SurrogateStr(_) | Value::Bytes(_)
    );
    let fd = if input == NONE || spelling {
        None
    } else if let Some(index) = ctx.index_value(input)? {
        if matches!(ctx.heap.get(input), Value::Bool(_)) {
            crate::objects::runtime_warning(
                "os",
                "<imre stdlib>/os.py",
                1,
                "bool is used as a file descriptor",
            )?;
        }
        Some(native::directory_fd_number(&index)?)
    } else {
        None
    };

    let filename;
    let bytes;
    let supplied_fd = fd.is_some_and(|fd| fd != -1);
    let handle = if let Some(fd) = fd.filter(|&fd| fd != -1) {
        filename = input;
        bytes = false;
        // SAFETY: fcntl checks arbitrary Python descriptor numbers before returning an owned fd.
        let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if copy < 0 {
            return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
        }
        // SAFETY: a successful F_DUPFD_CLOEXEC created a fresh descriptor.
        unsafe { OwnedFd::from_raw_fd(copy) }
    } else {
        let pathname = if input == NONE || fd == Some(-1) {
            filename = NONE;
            bytes = false;
            CString::from(c".")
        } else {
            filename = posixpath::path_arg(
                ctx,
                input,
                "listdir: path should be string, bytes, os.PathLike, integer or None",
            )?;
            bytes = matches!(ctx.heap.get(ctx.scalar_value(filename)), Value::Bytes(_));
            c_path(ctx, filename, "listdir: embedded null character in path")?
        };
        rustix::fs::open(
            &*pathname,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|e| directory_error(ctx, e, filename))?
    };
    // Keep the stream's fd open until the final rewind. A regular file's offset must not move.
    if supplied_fd {
        let stat = rustix::fs::fstat(&handle).map_err(|e| directory_error(ctx, e, filename))?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory {
            return Err(directory_error(ctx, rustix::io::Errno::NOTDIR, filename));
        }
    }
    let mut dir = Dir::new(handle).map_err(|e| directory_error(ctx, e, filename))?;
    let mut names = Vec::new();
    let read = (|| {
        for entry in dir.by_ref() {
            let entry = entry.map_err(|e| directory_error(ctx, e, filename))?;
            let name = entry.file_name().to_bytes();
            if name != b"." && name != b".." {
                names.push(if bytes {
                    ctx.alloc(Value::Bytes(name.to_vec()))
                } else {
                    ctx.text_codepoints(
                        crate::objects::interpreter_stdlib::fsdecode(name).collect(),
                    )
                });
            }
        }
        Ok(ctx.alloc(Value::List(names)))
    })();
    if supplied_fd && let Ok(fd) = dir.fd() {
        let _ = rustix::fs::seek(fd.as_fd(), SeekFrom::Start(0));
    }
    read
}

/// Attach the filesystem-protocol spelling to errors arising after path conversion.
fn directory_error(ctx: &mut Context, errno: rustix::io::Errno, filename: Id) -> Error {
    let error = crate::objects::errno_error(errno.into()).into();
    if filename == NONE {
        error
    } else {
        ctx.with_errno_filename(error, filename)
            .unwrap_or_else(|error| error)
    }
}

/// SourceFileLoader obtains size and mtime in one following stat or fstat.
pub(super) fn source_path_stats(ctx: &mut Context, path: Id) -> Result<Id> {
    let stat = stat::query(ctx, "stat", &[path], &[], |ctx, value| {
        if matches!(ctx.heap.get(value), Value::Bool(_)) {
            crate::objects::runtime_warning(
                "importlib._bootstrap_external",
                "<frozen importlib._bootstrap_external>",
                152,
                "bool is used as a file descriptor",
            )?;
        }
        Ok(())
    })?;
    let mtime_key = ctx.text("mtime");
    let mtime = ctx.alloc(Value::Float(native::float_seconds(
        stat.st_mtime as f64,
        stat.st_mtime_nsec,
    )));
    let size_key = ctx.text("size");
    let size = ctx.int(stat.st_size);
    Ok(ctx.dict(vec![(mtime_key, mtime), (size_key, size)]))
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
    let posix = ctx.module("posix")?;
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

mod unlink;

/// POSIX functions exported as the same builtins by both os and posix.
pub(super) fn is_descriptor_function(name: &str) -> bool {
    matches!(
        name,
        "open"
            | "scandir"
            | "remove"
            | "unlink"
            | "rmdir"
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
    if name == "scandir" {
        return scandir::open(ctx, args, kwargs);
    }
    if matches!(name, "remove" | "unlink" | "rmdir") {
        return unlink::call(ctx, name, args, kwargs);
    }
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
