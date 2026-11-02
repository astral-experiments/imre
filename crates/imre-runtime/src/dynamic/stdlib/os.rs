pub(in crate::dynamic) mod scandir;
pub(in crate::dynamic) mod stat;
#[cfg(target_os = "linux")]
pub(super) mod statvfs;

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
            &[
                "access", "chmod", "link", "lstat", "mkdir", "mkfifo", "open", "readlink",
                "rename", "rmdir", "stat", "symlink", "unlink", "utime",
            ][..],
        ),
        (
            "supports_fd",
            &[
                "chdir",
                "chmod",
                "listdir",
                "scandir",
                "stat",
                #[cfg(target_os = "linux")]
                "statvfs",
                "utime",
            ][..],
        ),
        (
            "supports_follow_symlinks",
            &[
                "access",
                #[cfg(target_os = "macos")]
                "chmod",
                "link",
                "stat",
                "utime",
            ][..],
        ),
        ("supports_effective_ids", &["access"][..]),
    ] {
        let set = ctx.alloc(Value::Set {
            entries: Default::default(),
            frozen: false,
        });
        for &function in functions {
            if function == "mkfifo" && mkfifo::at_function().is_none() {
                continue;
            }
            let value = ctx.lookup_attr(module, function)?;
            builtins::set_add(ctx, set, value)?;
        }
        ctx.bind(scope, capability, set);
    }
    Ok(())
}

/// Copy current POSIX environment bytes for the cached dictionary or an explicit reload.
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
    let value = if set {
        Some(environment_argument(ctx, args[1])?)
    } else {
        None
    };
    crate::objects::change_environment(&key, value.as_deref())?;
    Ok(NONE)
}

mod chmod;
mod links;
mod mkfifo;
#[cfg(target_os = "linux")]
pub(in crate::dynamic) mod scheduler;
#[cfg(target_os = "linux")]
pub(in crate::dynamic) mod sysconf;
mod unlink;
mod utime;

/// POSIX functions exported as the same builtins by both os and posix.
pub(super) fn is_descriptor_function(name: &str) -> bool {
    if name == "pread" {
        return cfg!(unix);
    }
    matches!(
        name,
        "access"
            | "open"
            | "getsid"
            | "waitpid"
            | "rename"
            | "replace"
            | "readlink"
            | "scandir"
            | "remove"
            | "unlink"
            | "rmdir"
            | "mkdir"
            | "mkfifo"
            | "chmod"
            | "fchmod"
            | "chdir"
            | "umask"
            | "utime"
            | "dup"
            | "dup2"
            | "isatty"
            | "lseek"
            | "ftruncate"
            | "pipe"
            | "read"
            | "write"
            | "close"
            | "closerange"
            | "get_blocking"
            | "set_blocking"
            | "get_inheritable"
            | "set_inheritable"
            | "link"
            | "symlink"
    ) || (cfg!(unix) && matches!(name, "kill" | "getppid" | "waitstatus_to_exitcode"))
        || (cfg!(target_os = "linux") && name == "sendfile")
}

/// Convert an index argument before issuing the descriptor syscall.
pub(super) fn c_int(ctx: &mut Context, value: Id) -> Result<i32> {
    ctx.index_integer(value)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

fn ssize_t(ctx: &mut Context, value: Id) -> Result<isize> {
    crate::signed_index(&ctx.index_integer(value)?).ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

fn file_offset(ctx: &mut Context, value: Id) -> Result<i64> {
    ctx.index_integer(value)?
        .to_i64()
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C long"))
}

enum PathArg {
    Fd(i32),
    Name(CString),
}

/// Convert a POSIX path and retain the Python spelling for audits and OSError.filename.
fn path_or_fd(
    ctx: &mut Context,
    name: &str,
    path: Id,
    allow_fd: bool,
    warn_bool_fd: fn(&mut Context, Id) -> Result<()>,
) -> Result<(PathArg, Id)> {
    let spelling = matches!(
        ctx.heap.get(ctx.scalar_value(path)),
        Value::Str(_) | Value::SurrogateStr(_) | Value::Bytes(_)
    );
    if allow_fd
        && !spelling
        && let Some(index) = ctx.index_value(path)?
    {
        warn_bool_fd(ctx, path)?;
        return Ok((PathArg::Fd(native::directory_fd_number(&index)?), path));
    }
    let kinds = if allow_fd {
        "string, bytes, os.PathLike or integer"
    } else {
        "string, bytes or os.PathLike"
    };
    let filename = posixpath::path_arg(ctx, path, &format!("{name}: path should be {kinds}"))?;
    let pathname = c_path(
        ctx,
        filename,
        &format!("{name}: embedded null character in path"),
    )?;
    Ok((PathArg::Name(pathname), filename))
}

/// Convert an optional dir_fd after the path and preceding arguments.
fn directory_fd(ctx: &mut Context, value: Option<Id>) -> Result<i32> {
    let Some(value) = value.filter(|&fd| fd != NONE) else {
        return Ok(libc::AT_FDCWD);
    };
    stat::warn_bool_fd(ctx, value)?;
    let fd = ctx.index_value(value)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "argument should be integer or None, not {}",
                ctx.type_name(value)
            ),
        )
    })?;
    Ok(native::directory_fd_number(&fd)?)
}

fn audit_fd(ctx: &mut Context, fd: i32) -> Id {
    ctx.int(i64::from(if fd == libc::AT_FDCWD { -1 } else { fd }))
}

/// Bind the path and keyword-only directory descriptor in Argument Clinic order.
fn path_and_dir_fd(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<(Id, CString, i32)> {
    let [Some(path), dir_fd] =
        builtins::bind_builtin_arguments(name, args, kwargs, ["path", "dir_fd"], 1)?
    else {
        unreachable!("path is required")
    };
    if args.len() > 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes exactly 1 positional argument ({} given)",
                args.len()
            ),
        ));
    }
    let path = posixpath::path_arg(
        ctx,
        path,
        &format!("{name}: path should be string, bytes or os.PathLike"),
    )?;
    let pathname = c_path(
        ctx,
        path,
        &format!("{name}: embedded null character in path"),
    )?;
    let dir_fd = directory_fd(ctx, dir_fd)?;
    Ok((path, pathname, dir_fd))
}

/// Change the process working directory after converting and auditing the supplied path or fd.
fn chdir(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(path)] = builtins::bind_builtin_arguments("chdir", args, kwargs, ["path"], 1)? else {
        unreachable!("chdir requires a path")
    };
    let (path, filename) = path_or_fd(ctx, "chdir", path, true, stat::warn_bool_fd)?;
    super::audit::event(ctx, "os.chdir", &[filename])?;
    let rc = match path {
        // CPython reserves -1 for a missing fd and has no pathname in this case.
        PathArg::Fd(-1) => {
            let error = std::io::Error::from_raw_os_error(libc::EFAULT);
            return Err(
                ctx.with_errno_filename(crate::objects::errno_error(error).into(), filename)?
            );
        }
        // SAFETY: fchdir checks the caller-supplied descriptor without taking ownership.
        PathArg::Fd(fd) => unsafe { libc::fchdir(fd) },
        // SAFETY: the converted, NUL-terminated pathname lives through the call.
        PathArg::Name(name) => unsafe { libc::chdir(name.as_ptr()) },
    };
    if rc < 0 {
        let error = crate::objects::errno_error(std::io::Error::last_os_error());
        return Err(ctx.with_errno_filename(error.into(), filename)?);
    }
    Ok(NONE)
}

/// Return the link contents with the string type of its converted path.
fn readlink(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (path, pathname, dir_fd) = path_and_dir_fd(ctx, "readlink", args, kwargs)?;
    let target = match native::readlink_at(&pathname, dir_fd) {
        Ok(target) => target,
        Err(error) => {
            return Err(ctx.with_errno_filename(crate::objects::errno_error(error).into(), path)?);
        }
    };
    if matches!(ctx.heap.get(ctx.scalar_value(path)), Value::Bytes(_)) {
        return Ok(ctx.alloc(Value::Bytes(target)));
    }
    let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
    Ok(ctx.text_codepoints(codec.decode_filesystem(&target)?))
}

/// Bind two required positional-or-keyword operands followed by keyword-only options.
fn bind_two_positional<const N: usize>(
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    names: [&str; N],
) -> Result<[Option<Id>; N]> {
    if args.len() > 2 && args.len() + kwargs.len() <= N {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes exactly 2 positional arguments ({} given)",
                args.len()
            ),
        ));
    }
    builtins::bind_builtin_arguments(name, args, kwargs, names, 2)
}

/// Test filesystem access using the credentials and link policy requested by the caller.
fn access(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let names = ["path", "mode", "dir_fd", "effective_ids", "follow_symlinks"];
    let [Some(path), Some(mode), dir_fd, effective, follow] =
        bind_two_positional("access", args, kwargs, names)?
    else {
        unreachable!("access requires path and mode")
    };
    let path = posixpath::path_arg(
        ctx,
        path,
        "access: path should be string, bytes or os.PathLike",
    )?;
    let path = c_path(ctx, path, "access: embedded null character in path")?;
    let mode = c_int(ctx, mode)?;
    let dir_fd = directory_fd(ctx, dir_fd)?;
    let effective = effective
        .map(|v| ctx.truth(v))
        .transpose()?
        .unwrap_or(false);
    let follow = follow.map(|v| ctx.truth(v)).transpose()?.unwrap_or(true);
    let flags = (if effective { libc::AT_EACCESS } else { 0 })
        | (if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW });
    // SAFETY: path is NUL-terminated and remains live through the synchronous check.
    let status = unsafe {
        if dir_fd == libc::AT_FDCWD && flags == 0 {
            libc::access(path.as_ptr(), mode)
        } else {
            libc::faccessat(dir_fd, path.as_ptr(), mode, flags)
        }
    };
    Ok(builtins::boolean(status == 0))
}

/// Create a directory or named pipe with the caller's mode and process umask.
fn create_node(ctx: &mut Context, name: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let names = ["path", "mode", "dir_fd"];
    let [Some(path), mode, dir_fd] = (if args.len() > 2 {
        builtins::bind_method_arguments(name, args, kwargs, names, 2)?
    } else {
        builtins::bind_builtin_arguments(name, args, kwargs, names, 1)?
    }) else {
        unreachable!("path is required")
    };
    let path = posixpath::path_arg(
        ctx,
        path,
        &format!("{name}: path should be string, bytes or os.PathLike"),
    )?;
    let pathname = c_path(
        ctx,
        path,
        &format!("{name}: embedded null character in path"),
    )?;
    let mode = mode
        .map(|value| c_int(ctx, value))
        .transpose()?
        .unwrap_or(if name == "mkdir" { 0o777 } else { 0o666 });
    let dir_fd = directory_fd(ctx, dir_fd)?;
    if name == "mkfifo" {
        return mkfifo::create(&pathname, mode as libc::mode_t, dir_fd);
    }
    // SAFETY: pathname is NUL-terminated and stays live through the synchronous call.
    if unsafe { libc::mkdirat(dir_fd, pathname.as_ptr(), mode as libc::mode_t) } != 0 {
        let error = crate::objects::errno_error(std::io::Error::last_os_error());
        return Err(ctx.with_errno_filename(error.into(), path)?);
    }
    Ok(NONE)
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
    if !ctx.audit_hooks.is_empty() {
        let audit_flags = ctx.int(i64::from(flags | libc::O_CLOEXEC));
        super::audit::event(ctx, "open", &[path, NONE, audit_flags])?;
    }
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
    if name == "access" {
        return access(ctx, args, kwargs);
    }
    if matches!(name, "rename" | "replace") {
        return links::rename(ctx, name, args, kwargs);
    }
    if name == "symlink" {
        return links::symlink(ctx, args, kwargs);
    }
    if name == "link" {
        return links::link(ctx, args, kwargs);
    }
    if name == "scandir" {
        return scandir::open(ctx, args, kwargs);
    }
    if matches!(name, "remove" | "unlink" | "rmdir") {
        return unlink::call(ctx, name, args, kwargs);
    }
    if name == "utime" {
        return utime::call(ctx, args, kwargs);
    }
    if name == "chdir" {
        return chdir(ctx, args, kwargs);
    }
    if name == "chmod" {
        return chmod::call(ctx, args, kwargs);
    }
    if name == "fchmod" {
        return chmod::fchmod(ctx, args, kwargs);
    }
    if matches!(name, "mkdir" | "mkfifo") {
        return create_node(ctx, name, args, kwargs);
    }
    if name == "readlink" {
        return readlink(ctx, args, kwargs);
    }
    if name == "umask" {
        let mask = builtins::one_positional_arg("posix.umask", args, kwargs)?;
        let mask = c_int(ctx, mask)?;
        // SAFETY: umask takes a numeric mode and returns the process's previous mask.
        let previous = unsafe { libc::umask(mask as libc::mode_t) };
        return Ok(ctx.int(i64::from(previous)));
    }
    if name == "open" {
        return open(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "sendfile" {
        let [Some(out_fd), Some(in_fd), Some(offset), Some(count)] =
            builtins::bind_builtin_arguments(
                name,
                args,
                kwargs,
                ["out_fd", "in_fd", "offset", "count"],
                4,
            )?
        else {
            unreachable!("sendfile requires both descriptors, offset and count")
        };
        let out_fd = c_int(ctx, out_fd)?;
        let in_fd = c_int(ctx, in_fd)?;
        // Argument Clinic converts count before sendfile converts an explicit offset.
        let count = ssize_t(ctx, count)?;
        let offset = if offset == NONE {
            None
        } else {
            Some(file_offset(ctx, offset)?)
        };
        return Ok(ctx.int(native::sendfile(out_fd, in_fd, offset, count)? as i64));
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
    #[cfg(unix)]
    if name == "waitstatus_to_exitcode" {
        let [Some(status)] = builtins::bind_builtin_arguments(name, args, kwargs, ["status"], 1)?
        else {
            unreachable!("status is required")
        };
        let status = c_int(ctx, status)?;
        let code = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else if libc::WIFSIGNALED(status) {
            -libc::WTERMSIG(status)
        } else if libc::WIFSTOPPED(status) {
            return Err(Error::new(
                "ValueError",
                format!(
                    "process stopped by delivery of signal {}",
                    libc::WSTOPSIG(status)
                ),
            ));
        } else {
            return Err(Error::new(
                "ValueError",
                format!("invalid wait status: {status}"),
            ));
        };
        return Ok(ctx.int(i64::from(code)));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("posix.{name}() takes no keyword arguments"),
        ));
    }
    if name == "getppid" {
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("posix.getppid() takes no arguments ({} given)", args.len()),
            ));
        }
        // SAFETY: getppid reads the calling process's parent PID and takes no arguments.
        return Ok(ctx.int(i64::from(unsafe { libc::getppid() })));
    }
    if name == "kill" {
        let [pid, signal] = args else {
            return Err(Error::new(
                "TypeError",
                format!("kill expected 2 arguments, got {}", args.len()),
            ));
        };
        let pid = c_int(ctx, *pid)?;
        let signal = ssize_t(ctx, *signal)?;
        let audit_pid = ctx.int(i64::from(pid));
        let audit_signal = ctx.int(signal as i64);
        super::audit::event(ctx, "os.kill", &[audit_pid, audit_signal])?;
        // CPython audits the full Py_ssize_t, then narrows it to the POSIX int.
        native::kill(pid, signal as libc::c_int)?;
        ctx.dispatch_signals()?;
        return Ok(NONE);
    }
    if name == "getsid" {
        let [pid] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "posix.getsid() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        let sid = native::getsid(c_int(ctx, *pid)?)?;
        return Ok(ctx.int(i64::from(sid)));
    }
    if name == "waitpid" {
        let [pid, options] = args else {
            return Err(Error::new(
                "TypeError",
                format!("waitpid expected 2 arguments, got {}", args.len()),
            ));
        };
        let pid = c_int(ctx, *pid)?;
        let options = c_int(ctx, *options)?;
        // Reserve the result before consuming a child status from the kernel.
        let mut result = Vec::new();
        result
            .try_reserve_exact(2)
            .map_err(|_| Error::new("MemoryError", ""))?;
        for value in native::waitpid(pid, options, || ctx.dispatch_signals())? {
            result.push(ctx.int(i64::from(value)));
        }
        return Ok(ctx.alloc(Value::Tuple(result)));
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
    if name == "pread" {
        let [fd, length, offset] = args else {
            return Err(Error::new(
                "TypeError",
                format!("pread expected 3 arguments, got {}", args.len()),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        let length = ssize_t(ctx, *length)?;
        let offset = file_offset(ctx, *offset)?;
        let bytes = native::read(fd, length, Some(offset), || ctx.dispatch_signals())?;
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    if matches!(name, "read" | "write") {
        let [fd, data] = args else {
            return Err(Error::new(
                "TypeError",
                format!("{name} expected 2 arguments, got {}", args.len()),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        return if name == "read" {
            let length = ssize_t(ctx, *data)?;
            let bytes = native::read(fd, length, None, || ctx.dispatch_signals())?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        } else {
            // Keep the exporter alive and its size fixed while a handler runs.
            // Reborrow on retry so in-place changes made by the handler are visible.
            ctx.with_buffer_export(*data, |ctx| {
                loop {
                    let bytes = match ctx.heap.get(ctx.scalar_value(*data)) {
                        Value::Array(array) => array.as_bytes(),
                        _ => ctx.bytes_like(*data)?,
                    };
                    if let Some(count) = native::try_write(fd, bytes)? {
                        return Ok(ctx.int(count as i64));
                    }
                    ctx.dispatch_signals()?;
                }
            })
        };
    }
    if name == "closerange" {
        let [low, high] = args else {
            return Err(Error::new(
                "TypeError",
                format!("closerange expected 2 arguments, got {}", args.len()),
            ));
        };
        let low = c_int(ctx, *low)?;
        let high = c_int(ctx, *high)?;
        native::close_range(low, high);
        return Ok(NONE);
    }
    if name == "ftruncate" {
        let [fd, length] = args else {
            return Err(Error::new(
                "TypeError",
                format!("ftruncate expected 2 arguments, got {}", args.len()),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        let length = file_offset(ctx, *length)?;
        let audit_fd = ctx.int(i64::from(fd));
        let audit_length = ctx.int(length);
        super::audit::event(ctx, "os.truncate", &[audit_fd, audit_length])?;
        loop {
            match native::truncate(fd, length) {
                Ok(()) => return Ok(NONE),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(super::os_error(error)),
            }
        }
    }
    if name == "lseek" {
        let [fd, position, whence] = args else {
            return Err(Error::new(
                "TypeError",
                format!("lseek expected 3 arguments, got {}", args.len()),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        let position = file_offset(ctx, *position)?;
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
