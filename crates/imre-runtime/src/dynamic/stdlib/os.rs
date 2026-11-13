pub(in crate::dynamic) mod attributes;
#[cfg(target_os = "linux")]
pub(in crate::dynamic) mod device;
#[cfg(target_os = "linux")]
pub(in crate::dynamic) mod eventfd;
#[cfg(target_os = "linux")]
pub(super) mod execve;
pub(in crate::dynamic) mod scandir;
#[cfg(target_os = "linux")]
mod spawn_child;
#[cfg(target_os = "linux")]
mod timerfd;
#[cfg(target_os = "linux")]
mod vectored;
#[cfg(target_os = "linux")]
pub(in crate::dynamic) use spawn_child::available as fork_available;
#[cfg(target_os = "linux")]
pub(super) use spawn_child::fork_for_child_callbacks;
#[cfg(target_os = "linux")]
mod spawn_plain;
pub(in crate::dynamic) mod stat;
#[cfg(target_os = "linux")]
pub(super) mod statvfs;

use std::ffi::CString;
use std::os::fd::IntoRawFd;

use super::{Context, Error, Id, NONE, Result, Value, builtins, posixpath};
use crate::dynamic::builtins::bind_two_positional;
use crate::objects::os as native;

/// Publish the supported POSIX operations from CPython's Lib/os.py capability tables whose
/// descriptor and no-follow forms are implemented by the boxed runtime.
#[cfg(unix)]
pub(in crate::dynamic) fn install_capabilities(
    ctx: &mut Context,
    posix: Id,
    scope: Id,
) -> Result<()> {
    for (capability, functions) in [
        (
            "supports_dir_fd",
            &[
                "access",
                "chmod",
                "chown",
                "link",
                "lstat",
                "mkdir",
                "mkfifo",
                #[cfg(target_os = "linux")]
                "mknod",
                "open",
                "readlink",
                "rename",
                "rmdir",
                "stat",
                "symlink",
                "unlink",
                "utime",
            ][..],
        ),
        (
            "supports_fd",
            &[
                "chdir",
                "chmod",
                "chown",
                "listdir",
                "scandir",
                #[cfg(target_os = "linux")]
                "pathconf",
                "stat",
                #[cfg(target_os = "linux")]
                "statvfs",
                "truncate",
                "utime",
            ][..],
        ),
        (
            "supports_follow_symlinks",
            &[
                "access",
                #[cfg(target_os = "macos")]
                "chmod",
                "chown",
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
            let value = match ctx.lookup_attr(posix, function) {
                Ok(value) => value,
                Err(error) if ctx.is_attribute_error(&error) => continue,
                Err(error) => return Err(error),
            };
            builtins::set_add(ctx, set, value)?;
        }
        #[cfg(target_os = "linux")]
        if capability == "supports_fd" {
            match ctx.lookup_attr(posix, "execve") {
                Ok(execve) => builtins::set_add(ctx, set, execve)?,
                Err(error) if ctx.is_attribute_error(&error) => (),
                Err(error) => return Err(error),
            }
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

pub(super) fn fs_argument(ctx: &mut Context, value: Id) -> Result<CString> {
    let path = posixpath::fspath(ctx, value)?;
    c_path(ctx, path, "embedded null byte")
}

/// Convert argv slots in order, allowing each path conversion to mutate the list.
#[cfg(unix)]
pub(super) fn fs_arguments(ctx: &mut Context, argv: Id, count: usize) -> Result<Vec<CString>> {
    let mut arguments = Vec::new();
    arguments
        .try_reserve_exact(count)
        .map_err(|_| Error::new("MemoryError", ""))?;
    for i in 0..count {
        // A path conversion may mutate argv. Fetch and root just the current item.
        let mark = ctx.heap.temporary_mark();
        let index = ctx.int(i as i64);
        let argument = ctx.get_item(argv, index).and_then(|item| {
            ctx.heap.keep(item);
            fs_argument(ctx, item)
        });
        let collect = ctx.heap.discard_truth_operands(mark, ctx.modules);
        if let Err(error) = &argument {
            for root in error.roots() {
                ctx.heap.keep(root);
            }
        }
        if collect {
            ctx.collect_unreachable();
        }
        arguments.push(argument?);
    }
    Ok(arguments)
}

/// Encode a filesystem spelling without imposing a C string's NUL restriction.
pub(super) fn encoded_path(ctx: &mut Context, path: Id) -> Result<Vec<u8>> {
    match ctx.heap.get(ctx.scalar_value(path)) {
        Value::Bytes(bytes) => Ok(bytes.to_vec()),
        Value::Str(_) | Value::SurrogateStr(_) => ctx.encode_os_path(path),
        _ => unreachable!("fspath returns str or bytes"),
    }
}

/// Terminate the validated filesystem spelling before evaluating any later Python argument.
pub(super) fn c_path(ctx: &mut Context, path: Id, nul_error: &str) -> Result<CString> {
    CString::new(encoded_path(ctx, path)?).map_err(|_| Error::new("ValueError", nul_error))
}

/// Create an anonymous Linux file using CPython's filesystem-name and unsigned-mask converters.
#[cfg(target_os = "linux")]
pub(super) fn memfd_create(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    use num_traits::ToPrimitive;
    use rustix::fs::MemfdFlags;
    use std::os::fd::IntoRawFd;

    let [Some(name), flags] =
        builtins::bind_builtin_arguments("memfd_create", args, kwargs, ["name", "flags"], 1)?
    else {
        unreachable!("memfd_create requires a name")
    };
    let path = posixpath::fspath(ctx, name)?;
    let name = c_path(ctx, path, "embedded null byte")?;
    let flags = if let Some(flags) = flags {
        let flags = ctx.index_integer(flags)?;
        (&*flags.big() & num_bigint::BigInt::from(u32::MAX))
            .to_u32()
            .expect("flags masked to 32 bits")
    } else {
        libc::MFD_CLOEXEC
    };
    let fd = rustix::fs::memfd_create(name.as_c_str(), MemfdFlags::from_bits_retain(flags))
        .map_err(|e| super::os_error(e.into()))?;
    Ok(ctx.int(i64::from(fd.into_raw_fd())))
}

/// Audit after signature validation and before any Python fork handlers run.
#[cfg(target_os = "linux")]
fn audit_fork(ctx: &mut Context) -> Result<()> {
    if ctx
        .interpreter
        .finalizing
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err(Error::new(
            "PythonFinalizationError",
            "can't fork at interpreter shutdown",
        ));
    }
    super::audit::event(ctx, "os.fork", &[])
}

#[cfg(target_os = "linux")]
pub(super) fn fork(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    builtins::bind_builtin_arguments("fork", args, kwargs, [], 0)?;
    audit_fork(ctx)?;
    let pid = fork_for_child_callbacks(ctx)?;
    Ok(ctx.int(i64::from(pid)))
}

/// Spawn directly for plain arguments, or let the child run the captured Python callable.
#[cfg(target_os = "linux")]
pub(super) fn spawn_exec(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(path), Some(argv), Some(exec)] =
        builtins::bind_builtin_arguments("_spawn_exec", args, kwargs, ["path", "args", "exec"], 3)?
    else {
        unreachable!("all three arguments are required")
    };
    use crate::dynamic::at_fork::{self, Phase};

    audit_fork(ctx)?;
    let mut warn = false;
    let result = at_fork::around(ctx, |ctx| {
        // A before-handler can add child handlers or mutate the argument list.
        if ctx.fork_hooks[Phase::Child as usize].is_empty()
            && let Some(result) = spawn_plain::try_spawn(ctx, path, argv, exec)
        {
            warn = spawn_plain::multiple_threads(ctx);
            return result;
        }
        spawn_child::fork_current(ctx)
    });
    if warn {
        spawn_plain::warn_about_threads(ctx);
    }
    let pid = result?;
    Ok(ctx.int(i64::from(pid)))
}

/// Resolve arguments before auditing, then replace this process using the captured bytes.
#[cfg(unix)]
fn execv(ctx: &mut Context, args: &[Id]) -> Result<Id> {
    let &[path, argv] = args else {
        return Err(Error::new(
            "TypeError",
            format!("execv expected 2 arguments, got {}", args.len()),
        ));
    };
    let path = posixpath::path_arg(
        ctx,
        path,
        "execv: path should be string, bytes or os.PathLike",
    )?;
    ctx.heap.keep(path);
    let executable = c_path(ctx, path, "execv: embedded null character in path")?;
    if !ctx.builtin_slot_accepts(argv, "list") && !ctx.builtin_slot_accepts(argv, "tuple") {
        return Err(Error::new(
            "TypeError",
            "execv() arg 2 must be a tuple or list",
        ));
    }
    let mark = ctx.heap.temporary_mark();
    let count = match ctx.instance_len(argv) {
        Ok(Some(count)) => count,
        Ok(None) => ctx.sequence_at(argv, 0).map_or(0, |(length, _)| length),
        // CPython replaces a failed length check with the empty-argv error.
        Err(_) => 0,
    };
    ctx.discard_truth(mark);
    if count == 0 {
        return Err(Error::new("ValueError", "execv() arg 2 must not be empty"));
    }
    let arguments = fs_arguments(ctx, argv, count)?;
    if arguments[0].as_bytes().is_empty() {
        return Err(Error::new(
            "ValueError",
            "execv() arg 2 first element cannot be empty",
        ));
    }
    let arguments = crate::objects::CStrings::new(arguments)?;
    super::audit::event(ctx, "os.exec", &[path, argv, NONE])?;
    Err(arguments.execv(&executable).into())
}

/// Obtain random bytes with the caller's Linux getrandom flags.
#[cfg(target_os = "linux")]
pub(super) fn getrandom(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(size), flags] =
        builtins::bind_builtin_arguments("getrandom", args, kwargs, ["size", "flags"], 1)?
    else {
        unreachable!()
    };
    let size = ssize_t(ctx, size)?;
    let flags = flags
        .map(|value| c_int(ctx, value))
        .transpose()?
        .unwrap_or(0);
    let bytes = native::getrandom(size, flags, || ctx.dispatch_signals())?;
    Ok(ctx.alloc(Value::Bytes(bytes)))
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
    let key = fs_argument(ctx, args[0])?;
    let value = if set {
        Some(fs_argument(ctx, args[1])?)
    } else {
        None
    };
    crate::objects::change_environment(&key, value.as_deref())?;
    Ok(NONE)
}

mod chmod;
mod chown;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) use chown::owner_id;
mod links;
mod mkfifo;
#[cfg(target_os = "linux")]
pub(in crate::dynamic) mod scheduler;
#[cfg(target_os = "linux")]
pub(in crate::dynamic) mod sysconf;
mod truncate;
mod unlink;
mod utime;
#[cfg(target_os = "linux")]
pub(in crate::dynamic) mod waitid;
#[cfg(target_os = "linux")]
pub(in crate::dynamic) mod xattr;

#[cfg(unix)]
fn setgroups(ctx: &mut Context, groups: Id) -> Result<Id> {
    if !ctx.is_sequence(groups) {
        return Err(Error::new(
            "TypeError",
            "setgroups argument must be a sequence",
        ));
    }
    let count = ctx.sequence_len(groups)?;
    if count > native::max_groups() {
        return Err(Error::new("ValueError", "too many groups"));
    }
    let mut ids = Vec::new();
    ids.try_reserve_exact(count)
        .map_err(|_| Error::new("MemoryError", ""))?;
    for i in 0..count {
        let key = ctx.int(i as i64);
        let value = ctx.get_item(groups, key)?;
        if !ctx.builtin_slot_accepts(value, "int") && ctx.slot(value, "__index__").is_none() {
            return Err(Error::new("TypeError", "groups must be integers"));
        }
        ids.push(chown::owner_id(ctx, value, true)? as libc::gid_t);
    }
    native::setgroups(&ids)?;
    Ok(NONE)
}

/// Retry a POSIX call that returns its error number directly instead of setting errno.
#[cfg(target_os = "linux")]
fn posix_file_call(ctx: &mut Context, mut call: impl FnMut() -> i32) -> Result<Id> {
    loop {
        match call() {
            0 => return Ok(NONE),
            libc::EINTR => ctx.dispatch_signals()?,
            code => return Err(super::os_error(std::io::Error::from_raw_os_error(code))),
        }
    }
}

/// Convert an index argument before issuing the descriptor syscall.
#[cfg(target_os = "linux")]
fn unsigned(ctx: &mut Context, id: Id, max: u64, kind: &str) -> Result<u64> {
    use num_traits::{Signed, ToPrimitive};
    let integer = ctx.index_integer(id)?;
    let n = integer.big();
    if n.is_negative() {
        return Err(Error::new("ValueError", "Cannot convert negative int"));
    }
    n.to_u64().filter(|&n| n <= max).ok_or_else(|| {
        Error::new(
            "OverflowError",
            format!("Python int too large for C unsigned {kind}"),
        )
    })
}

#[cfg(target_os = "linux")]
pub(super) fn sendfile_to<F: std::os::fd::AsRawFd>(
    ctx: &mut Context,
    out_fd: impl FnMut() -> crate::objects::exceptions::Result<F>,
    in_fd: Id,
    offset: Id,
    count: Id,
) -> Result<Id> {
    let in_fd = c_int(ctx, in_fd)?;
    // Argument Clinic converts count before sendfile converts an explicit offset.
    let count = ssize_t(ctx, count)?;
    let offset = if offset == NONE {
        None
    } else {
        Some(file_offset(ctx, offset)?)
    };
    let sent = native::sendfile(out_fd, in_fd, offset, count, || ctx.dispatch_signals())?;
    Ok(ctx.int(sent as i64))
}

pub(super) fn c_int(ctx: &mut Context, value: Id) -> Result<i32> {
    ctx.index_integer(value)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

fn readinto(ctx: &mut Context, fd: i32, dst: Id) -> Result<Id> {
    let span = match ctx.writable_byte_span(dst) {
        Ok(span) => span,
        Err(error) if matches!(error.kind.as_ref(), "BufferError" | "ValueError") => None,
        Err(error) => return Err(error),
    };
    let Some((storage, range)) = span else {
        let kind = if dst == NONE {
            "None"
        } else {
            ctx.type_name(dst)
        };
        return Err(Error::new(
            "TypeError",
            format!("readinto() argument 2 must be read-write bytes-like object, not {kind}"),
        ));
    };
    // Keep the view and its storage exported while an interrupted read runs a handler.
    ctx.with_buffer_export(dst, |ctx| {
        ctx.with_buffer_export(storage, |ctx| {
            loop {
                let bytes = ctx.writable_storage_bytes(storage)?;
                if let Some(count) = native::try_readinto(fd, &mut bytes[range.clone()])? {
                    return Ok(ctx.int(count as i64));
                }
                ctx.dispatch_signals()?;
            }
        })
    })
}

pub(super) fn ssize_t(ctx: &mut Context, value: Id) -> Result<isize> {
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

/// Hold the buffer export over offset conversion and interrupted write retries.
fn write_fd(ctx: &mut Context, fd: i32, data: Id, offset: Option<Id>) -> Result<Id> {
    ctx.with_read_buffer(data, |ctx, export| {
        let offset = offset.map(|id| file_offset(ctx, id)).transpose()?;
        loop {
            let buffer = export.storage(ctx)?;
            let ptr = buffer.read_ptr()?;
            // SAFETY: the export holds this range live until the synchronous syscall returns.
            if let Some(count) = unsafe { native::try_write(fd, ptr, buffer.len(), offset) }? {
                return Ok(ctx.int(count as i64));
            }
            ctx.dispatch_signals()?;
        }
    })
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
    let kinds = if allow_fd {
        "string, bytes, os.PathLike or integer"
    } else {
        "string, bytes or os.PathLike"
    };
    convert_path(ctx, name, path, kinds, allow_fd.then_some(warn_bool_fd))
}

// An optional descriptor converter keeps __index__ errors distinct from path-type diagnostics.
fn convert_path(
    ctx: &mut Context,
    name: &str,
    path: Id,
    kinds: &str,
    fd_warning: Option<fn(&mut Context, Id) -> Result<()>>,
) -> Result<(PathArg, Id)> {
    let spelling = matches!(
        ctx.heap.get(ctx.scalar_value(path)),
        Value::Str(_) | Value::SurrogateStr(_) | Value::Bytes(_)
    );
    if let Some(warn_bool_fd) = fd_warning
        && !spelling
        && let Some(index) = ctx.index_value(path)?
    {
        warn_bool_fd(ctx, path)?;
        return Ok((PathArg::Fd(native::directory_fd_number(&index)?), path));
    }
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
    let fd = open_path(ctx, path, &pathname, flags, mode, dir_fd)?;
    Ok(ctx.int(i64::from(fd.into_raw_fd())))
}

/// Decorate only the failed system call. Signal and audit exceptions keep their identity.
pub(super) fn open_path(
    ctx: &mut Context,
    path: Id,
    pathname: &std::ffi::CStr,
    flags: i32,
    mode: i32,
    dir_fd: i32,
) -> Result<std::os::fd::OwnedFd> {
    match native::open_at(pathname, flags, mode, dir_fd, || ctx.dispatch_signals())? {
        Ok(fd) => Ok(fd),
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
    #[cfg(target_os = "linux")]
    if name.starts_with("timerfd_") {
        return timerfd::call(ctx, name, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "pidfd_open" {
        let [pid, flags] =
            builtins::bind_builtin_arguments(name, args, kwargs, ["pid", "flags"], 1)?;
        let pid = c_int(ctx, pid.unwrap())?;
        let flags = flags
            .map(|value| unsigned(ctx, value, u64::from(u32::MAX), "int"))
            .transpose()?
            .unwrap_or(0) as u32;
        return Ok(ctx.int(i64::from(native::pidfd_open(pid, flags)?)));
    }
    if name == "device_encoding" {
        let [Some(fd)] = builtins::bind_builtin_arguments(name, args, kwargs, ["fd"], 1)? else {
            unreachable!("device_encoding requires a descriptor");
        };
        return Ok(match native::device_encoding(c_int(ctx, fd)?) {
            None => NONE,
            Some(std::borrow::Cow::Borrowed(name)) => ctx.heap.code_name(name),
            Some(std::borrow::Cow::Owned(name)) => ctx.text(name),
        });
    }
    if name == "access" {
        return access(ctx, args, kwargs);
    }
    if matches!(name, "rename" | "replace") {
        return links::rename(ctx, name, args, kwargs);
    }
    if name == "symlink" {
        return links::symlink(ctx, args, kwargs);
    }
    if matches!(name, "chown" | "fchown" | "lchown") {
        return chown::call(ctx, name, args, kwargs);
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
    let fd_call: Option<unsafe extern "C" fn(i32) -> i32> = match name {
        "fchdir" => Some(libc::fchdir),
        "fsync" => Some(libc::fsync),
        #[cfg(target_os = "linux")]
        "fdatasync" => Some(libc::fdatasync),
        _ => None,
    };
    if let Some(call) = fd_call {
        let [Some(fd)] = builtins::bind_builtin_arguments(name, args, kwargs, ["fd"], 1)? else {
            unreachable!("a descriptor is required");
        };
        stat::warn_bool_fd(ctx, fd)?;
        let fd = super::select::fd(ctx, fd)?;
        if name == "fchdir" {
            let audit_fd = ctx.int(i64::from(fd));
            super::audit::event(ctx, "os.chdir", &[audit_fd])?;
        }
        loop {
            // SAFETY: these calls borrow the descriptor and validate it in the kernel.
            if unsafe { call(fd) } == 0 {
                return Ok(NONE);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(super::os_error(error));
            }
            ctx.dispatch_signals()?;
        }
    }
    if name == "chdir" {
        return chdir(ctx, args, kwargs);
    }
    if name == "truncate" {
        return truncate::call(ctx, args, kwargs);
    }
    if name == "chmod" {
        return chmod::call(ctx, args, kwargs);
    }
    if name == "fchmod" {
        return chmod::fchmod(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "mknod" {
        return device::mknod(ctx, args, kwargs);
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
    #[cfg(target_os = "linux")]
    if name == "tcgetpgrp" {
        let fd = builtins::one_positional_arg("posix.tcgetpgrp", args, kwargs)?;
        let group = native::tcgetpgrp(c_int(ctx, fd)?)?;
        return Ok(ctx.int(i64::from(group)));
    }
    #[cfg(unix)]
    if name == "setgroups" {
        let groups = builtins::one_positional_arg("posix.setgroups", args, kwargs)?;
        return setgroups(ctx, groups);
    }
    #[cfg(unix)]
    if name == "getpgid" {
        let [Some(pid)] = builtins::bind_builtin_arguments(name, args, kwargs, ["pid"], 1)? else {
            unreachable!("pid is required")
        };
        let group = native::getpgid(c_int(ctx, pid)?)?;
        return Ok(ctx.int(i64::from(group)));
    }
    #[cfg(unix)]
    if matches!(name, "setpgid" | "setregid" | "setreuid") {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("posix.{name}() takes no keyword arguments"),
            ));
        }
        let [first, second] = args else {
            return Err(Error::new(
                "TypeError",
                format!("{name} expected 2 arguments, got {}", args.len()),
            ));
        };
        if name == "setpgid" {
            native::setpgid(c_int(ctx, *first)?, c_int(ctx, *second)?)?;
        } else {
            let is_group = name == "setregid";
            let real = chown::owner_id(ctx, *first, is_group)?;
            let effective = chown::owner_id(ctx, *second, is_group)?;
            if is_group {
                native::setregid(real as libc::gid_t, effective as libc::gid_t)?;
            } else {
                native::setreuid(real as libc::uid_t, effective as libc::uid_t)?;
            }
        }
        return Ok(NONE);
    }
    if name == "open" {
        return open(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if matches!(name, "copy_file_range" | "splice") {
        let [
            Some(src),
            Some(dst),
            Some(count),
            offset_src,
            offset_dst,
            flags,
        ] = (if name == "splice" {
            builtins::bind_builtin_arguments(
                name,
                args,
                kwargs,
                ["src", "dst", "count", "offset_src", "offset_dst", "flags"],
                3,
            )?
        } else {
            let [src, dst, count, offset_src, offset_dst] = builtins::bind_builtin_arguments(
                name,
                args,
                kwargs,
                ["src", "dst", "count", "offset_src", "offset_dst"],
                3,
            )?;
            [src, dst, count, offset_src, offset_dst, None]
        })
        else {
            unreachable!("both descriptors and a count are required")
        };
        let src = c_int(ctx, src)?;
        let dst = c_int(ctx, dst)?;
        let count = ssize_t(ctx, count)?;
        // Argument Clinic converts flags before splice checks count or converts offsets.
        let operation = if name == "splice" {
            let flags = flags
                .map(|id| unsigned(ctx, id, u64::from(u32::MAX), "int"))
                .transpose()?
                .unwrap_or(0);
            native::RangeTransfer::Splice(flags as u32)
        } else {
            native::RangeTransfer::CopyFileRange
        };
        let count = usize::try_from(count)
            .map_err(|_| Error::new("ValueError", "negative value for 'count' not allowed"))?;
        let mut offset = |value| match value {
            None | Some(NONE) => Ok(None),
            Some(value) => file_offset(ctx, value).map(Some),
        };
        let offset_src = offset(offset_src)?;
        let offset_dst = offset(offset_dst)?;
        let copied = operation.run(src, dst, count, offset_src, offset_dst, || {
            ctx.dispatch_signals()
        })?;
        return Ok(ctx.int(copied as i64));
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
        return sendfile_to(ctx, || Ok(out_fd), in_fd, offset, count);
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
    if name == "_exit" {
        let [Some(status)] = builtins::bind_builtin_arguments(name, args, kwargs, ["status"], 1)?
        else {
            unreachable!("status is required")
        };
        let status = c_int(ctx, status)?;
        // SAFETY: _exit accepts any C int and terminates without running cleanup.
        unsafe { libc::_exit(status) }
    }
    #[cfg(unix)]
    if matches!(name, "WIFSTOPPED" | "waitstatus_to_exitcode") {
        let [Some(status)] = builtins::bind_builtin_arguments(name, args, kwargs, ["status"], 1)?
        else {
            unreachable!("status is required")
        };
        let status = c_int(ctx, status)?;
        if name == "WIFSTOPPED" {
            return Ok(builtins::boolean(libc::WIFSTOPPED(status)));
        }
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
    #[cfg(target_os = "linux")]
    if matches!(name, "posix_openpt" | "grantpt" | "unlockpt") {
        let value = builtins::one_positional_arg(&format!("posix.{name}"), args, kwargs)?;
        if name == "posix_openpt" {
            let flags = c_int(ctx, value)?;
            return Ok(ctx.int(i64::from(native::posix_openpt(flags)?)));
        }
        stat::warn_bool_fd(ctx, value)?;
        let fd = super::select::fd(ctx, value)?;
        // SAFETY: each function borrows a descriptor and reports failure through errno.
        let status = unsafe {
            if name == "grantpt" {
                libc::grantpt(fd)
            } else {
                libc::unlockpt(fd)
            }
        };
        if status == -1 {
            return Err(super::os_error(std::io::Error::last_os_error()));
        }
        return Ok(NONE);
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("posix.{name}() takes no keyword arguments"),
        ));
    }
    #[cfg(target_os = "linux")]
    if matches!(name, "setresgid" | "setresuid") {
        let [real, effective, saved] = args else {
            return Err(Error::new(
                "TypeError",
                format!("{name} expected 3 arguments, got {}", args.len()),
            ));
        };
        let group = name == "setresgid";
        let real = chown::owner_id(ctx, *real, group)?;
        let effective = chown::owner_id(ctx, *effective, group)?;
        let saved = chown::owner_id(ctx, *saved, group)?;
        if group {
            native::setresgid(
                real as libc::gid_t,
                effective as libc::gid_t,
                saved as libc::gid_t,
            )?;
        } else {
            native::setresuid(
                real as libc::uid_t,
                effective as libc::uid_t,
                saved as libc::uid_t,
            )?;
        }
        return Ok(NONE);
    }
    #[cfg(target_os = "linux")]
    if name == "tcsetpgrp" {
        let [fd, pgid] = args else {
            return Err(Error::new(
                "TypeError",
                format!("tcsetpgrp expected 2 arguments, got {}", args.len()),
            ));
        };
        native::tcsetpgrp(c_int(ctx, *fd)?, c_int(ctx, *pgid)?)?;
        return Ok(NONE);
    }
    #[cfg(target_os = "linux")]
    if name == "get_terminal_size" {
        let fd = match args {
            [] => libc::STDOUT_FILENO,
            [fd] => c_int(ctx, *fd)?,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "get_terminal_size expected at most 1 argument, got {}",
                        args.len()
                    ),
                ));
            }
        };
        let mut size = std::mem::MaybeUninit::<libc::winsize>::uninit();
        // SAFETY: TIOCGWINSZ writes a winsize on success. The kernel validates the borrowed fd.
        if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, size.as_mut_ptr()) } < 0 {
            return Err(super::os_error(std::io::Error::last_os_error()));
        }
        // SAFETY: the successful ioctl initialized size.
        let size = unsafe { size.assume_init() };
        let columns = ctx.int(i64::from(size.ws_col));
        let lines = ctx.int(i64::from(size.ws_row));
        return Ok(super::fixed_structseq::record(
            ctx,
            super::fixed_structseq::TERMINAL_SIZE,
            vec![columns, lines],
        ));
    }
    #[cfg(target_os = "linux")]
    if matches!(name, "ptsname" | "ttyname") {
        let [fd] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "posix.{name}() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        let bytes = if name == "ttyname" {
            native::ttyname(c_int(ctx, *fd)?)?
        } else {
            stat::warn_bool_fd(ctx, *fd)?;
            let fd = super::select::fd(ctx, *fd)?;
            native::ptsname(fd).map_err(super::os_error)?
        };
        let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
        return Ok(ctx.text_codepoints(codec.decode_filesystem(&bytes)?));
    }
    #[cfg(target_os = "linux")]
    if name == "posix_fadvise" {
        let [fd, offset, length, advice] = args else {
            return Err(Error::new(
                "TypeError",
                format!("posix_fadvise expected 4 arguments, got {}", args.len()),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        let offset = file_offset(ctx, *offset)?;
        let length = file_offset(ctx, *length)?;
        let advice = c_int(ctx, *advice)?;
        // SAFETY: posix_fadvise borrows the fd and takes the range and advice by value.
        return posix_file_call(ctx, || unsafe {
            libc::posix_fadvise(fd, offset, length, advice)
        });
    }
    #[cfg(target_os = "linux")]
    if name == "posix_fallocate" {
        let [fd, offset, length] = args else {
            return Err(Error::new(
                "TypeError",
                format!("posix_fallocate expected 3 arguments, got {}", args.len()),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        let offset = file_offset(ctx, *offset)?;
        let length = file_offset(ctx, *length)?;
        // SAFETY: posix_fallocate borrows the fd and takes signed offsets by value.
        return posix_file_call(ctx, || unsafe { libc::posix_fallocate(fd, offset, length) });
    }
    #[cfg(unix)]
    if matches!(name, "getpgrp" | "getppid" | "times") {
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("posix.{name}() takes no arguments ({} given)", args.len()),
            ));
        }
        if name == "times" {
            let values = native::times()?.map(|seconds| ctx.alloc(Value::Float(seconds)));
            return Ok(super::fixed_structseq::record(
                ctx,
                super::fixed_structseq::TIMES_RESULT,
                values.into(),
            ));
        }
        let id = if name == "getpgrp" {
            // SAFETY: getpgrp takes no arguments and returns the calling process's group ID.
            unsafe { libc::getpgrp() }
        } else {
            // SAFETY: getppid takes no arguments and returns the calling process's parent ID.
            unsafe { libc::getppid() }
        };
        return Ok(ctx.int(i64::from(id)));
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
    #[cfg(unix)]
    if name == "execv" {
        return execv(ctx, args);
    }
    #[cfg(target_os = "linux")]
    if name == "waitid" {
        return waitid::call(ctx, args);
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
    if matches!(name, "openpty" | "pipe" | "pipe2") {
        let mut result = Vec::new();
        result
            .try_reserve_exact(2)
            .map_err(|_| Error::new("MemoryError", ""))?;
        let fds = match (name, args) {
            ("openpty", []) => native::openpty()?,
            ("pipe", []) => native::pipe(libc::O_CLOEXEC)?,
            ("pipe2", [flags]) => native::pipe(c_int(ctx, *flags)?)?,
            _ => {
                let arity = if name != "pipe2" {
                    "no arguments"
                } else {
                    "exactly one argument"
                };
                return Err(Error::new(
                    "TypeError",
                    format!("posix.{name}() takes {arity} ({} given)", args.len()),
                ));
            }
        };
        for fd in fds {
            result.push(ctx.int(i64::from(fd)));
        }
        return Ok(ctx.alloc(Value::Tuple(result)));
    }
    #[cfg(target_os = "linux")]
    if matches!(name, "preadv" | "pwritev") {
        let [fd, buffers, offset, rest @ ..] = args else {
            return Err(Error::new(
                "TypeError",
                format!("{name} expected at least 3 arguments, got {}", args.len()),
            ));
        };
        if rest.len() > 1 {
            return Err(Error::new(
                "TypeError",
                format!("{name} expected at most 4 arguments, got {}", args.len()),
            ));
        }
        let fd = c_int(ctx, *fd)?;
        let offset = file_offset(ctx, *offset)?;
        let flags = rest
            .first()
            .map(|id| c_int(ctx, *id))
            .transpose()?
            .unwrap_or(0);
        return vectored::transfer(ctx, fd, *buffers, name, Some((offset, flags)));
    }
    if matches!(name, "pread" | "pwrite") {
        let [fd, data, offset] = args else {
            return Err(Error::new(
                "TypeError",
                format!("{name} expected 3 arguments, got {}", args.len()),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        if name == "pwrite" {
            return write_fd(ctx, fd, *data, Some(*offset));
        }
        let length = ssize_t(ctx, *data)?;
        let offset = file_offset(ctx, *offset)?;
        let bytes = native::read(fd, length, Some(offset), || ctx.dispatch_signals())?;
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    if matches!(name, "read" | "readinto" | "write")
        || (cfg!(target_os = "linux") && matches!(name, "readv" | "writev"))
    {
        let [fd, data] = args else {
            return Err(Error::new(
                "TypeError",
                format!("{name} expected 2 arguments, got {}", args.len()),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        #[cfg(target_os = "linux")]
        if matches!(name, "readv" | "writev") {
            return vectored::transfer(ctx, fd, *data, name, None);
        }
        return if name == "read" {
            let length = ssize_t(ctx, *data)?;
            let bytes = native::read(fd, length, None, || ctx.dispatch_signals())?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        } else if name == "readinto" {
            readinto(ctx, fd, *data)
        } else {
            write_fd(ctx, fd, *data, None)
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
    #[cfg(target_os = "linux")]
    if name == "lockf" {
        let [fd, command, length] = args else {
            return Err(Error::new(
                "TypeError",
                format!("lockf expected 3 arguments, got {}", args.len()),
            ));
        };
        let fd = c_int(ctx, *fd)?;
        let command = c_int(ctx, *command)?;
        let length = file_offset(ctx, *length)?;
        let audit_fd = ctx.int(i64::from(fd));
        let audit_command = ctx.int(i64::from(command));
        let audit_length = ctx.int(length);
        super::audit::event(ctx, "os.lockf", &[audit_fd, audit_command, audit_length])?;
        if let Err(error) = native::lockf(fd, command, length) {
            if error.kind() == std::io::ErrorKind::Interrupted {
                // A raising signal handler takes precedence over EINTR. Do not retry the lock.
                ctx.dispatch_signals()?;
            }
            return Err(super::os_error(error));
        }
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
        return truncate::by_fd(ctx, fd, length);
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
