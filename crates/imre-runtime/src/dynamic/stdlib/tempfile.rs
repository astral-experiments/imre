//! Filesystem primitives for the runtime-loaded tempfile module.

use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::{Context, Error, Id, NONE, Result, Value, argument};
use crate::objects::{FileState, errno_error};

pub(in crate::dynamic) const METHODS: &[&str] = &[
    "read",
    "readline",
    "readable",
    "readinto",
    "writable",
    "seekable",
    "isatty",
    "write",
    "seek",
    "tell",
    "truncate",
    "fileno",
    "flush",
    "close",
    "__enter__",
    "__exit__",
];

/// Decode a POSIX spelling as a Python filesystem string, including surrogate escapes.
fn filesystem_text(ctx: &mut Context, path: PathBuf) -> Id {
    match path.into_os_string().into_string() {
        Ok(text) => ctx.text(text),
        Err(path) => {
            let points = crate::objects::interpreter_stdlib::fsdecode(path.as_bytes()).collect();
            ctx.text_codepoints(points)
        }
    }
}

/// The wrapper and any saved wrapper methods own cleanup independently of the raw stream.
pub(in crate::dynamic) struct NamedCloser {
    file: crate::Shared<RefCell<FileState>>,
    unlink: Option<PathBuf>,
    on_close: bool,
}

impl NamedCloser {
    pub(in crate::dynamic) fn needs_cleanup(&self) -> bool {
        self.unlink.is_some() || !self.file.borrow().closed()
    }

    fn unlink(&mut self) -> std::io::Result<()> {
        if let Some(path) = self.unlink.take() {
            match std::fs::remove_file(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                result => return result,
            }
        }
        Ok(())
    }
}

impl Drop for NamedCloser {
    fn drop(&mut self) {
        if !self.file.borrow().random_truncating() {
            let _ = self.file.borrow_mut().close();
        }
        let _ = self.unlink();
    }
}

fn open_named(ctx: &mut Context, name: Id, on_close: Id, write_only: bool) -> Result<Id> {
    let bytes = match ctx.heap.get(ctx.scalar_value(name)) {
        Value::Bytes(bytes) => bytes.clone(),
        _ => ctx.encode_filesystem_string(name)?,
    };
    if bytes.contains(&0) {
        return Err(Error::new("ValueError", "embedded null byte"));
    }
    let path = PathBuf::from(OsString::from_vec(bytes));
    let on_close = !write_only && ctx.truth(on_close)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path);
    let file = match file {
        Ok(file) => file,
        Err(error) => return Err(ctx.with_errno_filename(errno_error(error).into(), name)?),
    };
    if write_only {
        let writer = ctx.temporary_writer(file, name);
        if writer.is_err() {
            let _ = std::fs::remove_file(path);
        }
        return writer;
    }
    let owners = (|| -> Result<_> {
        let file = FileState::buffered_random(file)?;
        let owner =
            crate::Shared::try_new(RefCell::new(file)).map_err(|_| crate::Error::allocation())?;
        let closer = NamedCloser {
            file: owner.clone(),
            unlink: None,
            on_close,
        };
        let closer =
            crate::Shared::try_new(RefCell::new(closer)).map_err(|_| crate::Error::allocation())?;
        Ok((owner, closer))
    })();
    let (owner, closer) = match owners {
        Ok(owners) => owners,
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
    };
    let guard = match ctx
        .heap
        .alloc_file_writer(Value::NamedFileCloser(closer.clone()))
    {
        Ok(guard) => guard,
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
    };
    closer.borrow_mut().unlink = Some(path);
    let file = ctx.alloc(Value::BufferedRandom(owner, name));
    Ok(ctx.alloc(Value::Tuple(vec![file, guard])))
}

/// Keep native path bytes intact when the caller requests a bytes filename.
pub(super) fn path_operation(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    use crate::objects::tempdirs;
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name} takes no keyword arguments"),
        ));
    }
    match (name, args) {
        ("check_text_options", &[encoding, errors, newline]) => {
            super::io::check_text_options(ctx, Some(encoding), Some(errors), Some(newline))?;
            Ok(NONE)
        }
        ("open_named", &[name, on_close]) => open_named(ctx, name, on_close, false),
        ("open_named", &[name, on_close, writer]) => {
            let write_only = ctx.truth(writer)?;
            open_named(ctx, name, on_close, write_only)
        }
        ("keep_named", &[guard]) => {
            let Value::NamedFileCloser(closer) = ctx.heap.get(guard) else {
                return Err(Error::new(
                    "TypeError",
                    "expected a named temporary file closer",
                ));
            };
            closer.borrow_mut().unlink = None;
            Ok(NONE)
        }
        ("finish_named", &[guard, name, exiting]) => {
            let exiting = ctx.truth(exiting)?;
            let Value::NamedFileCloser(closer) = ctx.heap.get(guard).clone() else {
                return Err(Error::new(
                    "TypeError",
                    "expected a named temporary file closer",
                ));
            };
            let mut closer = closer.borrow_mut();
            let closed = {
                let mut file = closer.file.borrow_mut();
                file.check_random_reentry()
                    .and_then(|()| file.close().map_err(errno_error))
            };
            if (exiting || closer.on_close)
                && let Err(error) = closer.unlink()
            {
                drop(closer);
                return Err(ctx.with_errno_filename(errno_error(error).into(), name)?);
            }
            closed?;
            Ok(NONE)
        }
        ("random_name", []) => Ok(ctx.text(tempdirs::random_name().map_err(errno_error)?)),
        ("default_directory", []) => Ok(filesystem_text(ctx, tempdirs::find_default_directory()?)),
        ("abspath" | "mkdir" | "cleanup_directory", &[arg]) => {
            let arg = super::posixpath::fspath(ctx, arg)?;
            let value = ctx.heap.get(ctx.scalar_value(arg));
            let is_bytes = matches!(value, Value::Bytes(_));
            let encoded;
            let path = match value {
                Value::Str(text) => Path::new(text),
                Value::SurrogateStr(_) => {
                    encoded = ctx.encode_filesystem_string(arg)?;
                    Path::new(OsStr::from_bytes(&encoded))
                }
                Value::Bytes(bytes) => Path::new(OsStr::from_bytes(bytes)),
                _ => return Err(Error::new("UnicodeError", "cannot encode filesystem path")),
            };
            if name == "cleanup_directory" {
                if !path.is_absolute() || path.file_name().is_none() {
                    return Err(Error::new(
                        "ValueError",
                        "expected a named temporary directory",
                    ));
                }
                let mut entry = path.to_owned();
                return match tempdirs::remove_tree(&mut entry) {
                    Ok(()) => Ok(NONE),
                    Err(error) => {
                        let filename = if is_bytes {
                            ctx.alloc(Value::Bytes(entry.into_os_string().into_vec()))
                        } else {
                            filesystem_text(ctx, entry)
                        };
                        Err(ctx.with_errno_filename(errno_error(error).into(), filename)?)
                    }
                };
            }
            if name == "mkdir" {
                if path.as_os_str().as_bytes().contains(&0) {
                    return Err(Error::new(
                        "ValueError",
                        "mkdir: embedded null character in path",
                    ));
                }
                return match tempdirs::create_private_directory(path) {
                    Ok(()) => Ok(NONE),
                    Err(error) => Err(ctx.with_errno_filename(errno_error(error).into(), arg)?),
                };
            }
            let path = tempdirs::absolute(path).map_err(errno_error)?;
            if is_bytes {
                Ok(ctx.alloc(Value::Bytes(path.into_os_string().into_vec())))
            } else {
                Ok(filesystem_text(ctx, path))
            }
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        )),
    }
}

/// Adopt an unnamed descriptor into the buffering and text layers requested by its mode.
pub(super) fn open(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if args.len() > 1 {
        return Err(Error::new(
            "TypeError",
            "unsupported arguments to TemporaryFile",
        ));
    }
    let [mode, directory, prefix, suffix, encoding, errors, newline] = super::builtins::bind(
        "TemporaryFile",
        args,
        kwargs,
        [
            "mode", "dir", "prefix", "suffix", "encoding", "errors", "newline",
        ],
    )?;
    if let Some(mode) = mode {
        super::io::check_open_string(ctx, mode, "mode", "str")?;
    }
    let [encoding, errors, newline] =
        [encoding, errors, newline].map(|value| value.filter(|&id| id != NONE));
    super::io::check_text_options(ctx, encoding, errors, newline)?;
    let mode_text = mode
        .map(|id| ctx.str_value(id))
        .transpose()?
        .unwrap_or("w+b");
    super::io::validate_open_mode(mode_text)?;
    let binary = mode_text.contains('b');
    if binary {
        super::io::check_binary_options(encoding, errors, newline)?;
    }
    let (_, raw_mode) = super::io::fileio_mode(&mode_text.replace('t', ""))?;
    if !matches!(raw_mode, "wb" | "rb+") {
        return Err(Error::new(
            "NotImplementedError",
            "TemporaryFile supports write and update modes",
        ));
    }
    if !binary {
        ctx.module("io")?;
    }
    let directory = directory.filter(|&id| id != NONE);
    let file = if let (Some(dir), Some(prefix), Some(suffix)) = (directory, prefix, suffix) {
        let path = |ctx: &mut Context, value| {
            let value = super::posixpath::fspath(ctx, value)?;
            super::os::c_path(ctx, value, "embedded null byte")
        };
        let directory = path(ctx, dir)?;
        let prefix = path(ctx, prefix)?;
        let suffix = path(ctx, suffix)?;
        crate::objects::tempdirs::anonymous_file_in(
            Path::new(OsStr::from_bytes(directory.as_bytes())),
            OsStr::from_bytes(prefix.as_bytes()),
            OsStr::from_bytes(suffix.as_bytes()),
        )?
    } else {
        crate::objects::tempdirs::anonymous_file(
            directory.map(|id| ctx.str_value(id)).transpose()?,
        )?
    };
    let name = ctx.int(i64::from(file.as_raw_fd()));
    let buffer = if raw_mode == "wb" {
        ctx.temporary_writer(file, name)?
    } else {
        let owner = crate::Shared::try_new(RefCell::new(FileState::buffered_random(file)?))
            .map_err(|_| crate::Error::allocation())?;
        ctx.alloc(Value::BufferedRandom(owner, name))
    };
    if binary {
        return Ok(buffer);
    }
    let wrapper = ctx.builtins["_io.TextIOWrapper"];
    let result = ctx
        .call(
            wrapper,
            vec![
                buffer,
                encoding.unwrap_or(NONE),
                errors.unwrap_or(NONE),
                newline.unwrap_or(NONE),
            ],
            Vec::new(),
        )
        .and_then(|stream| {
            ctx.set_attr(stream, "mode", ctx.scalar_value(mode.unwrap()))?;
            Ok(stream)
        });
    if result.is_err() {
        let _ = ctx
            .lookup_attr(buffer, "close")
            .and_then(|close| ctx.call(close, Vec::new(), Vec::new()));
    }
    result
}

/// Applies a boxed operation to the same buffered stream used by the native file ABI.
pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let receiver = argument(args, 0)?;
    let Value::BufferedRandom(owner, file_name) = ctx.heap.get(receiver).clone() else {
        return Err(Error::new("TypeError", "expected a buffered random file"));
    };
    let args = &args[1..];
    let arity = match name {
        "read" | "truncate" => 0..=1,
        "write" | "readinto" => 1..=1,
        "seek" => 1..=2,
        "__exit__" => 3..=3,
        _ => 0..=0,
    };
    if !arity.contains(&args.len()) || !kwargs.is_empty() {
        let count = args.len();
        let message = match name {
            "readinto" if !kwargs.is_empty() => {
                format!("BufferedRandom.{name}() takes no keyword arguments")
            }
            "readinto" => {
                format!("BufferedRandom.readinto() takes exactly one argument ({count} given)")
            }
            _ => format!("unsupported arguments to BufferedRandom.{name}"),
        };
        return Err(Error::new("TypeError", message));
    }
    if matches!(
        name,
        "readline" | "truncate" | "flush" | "close" | "__exit__"
    ) {
        owner.borrow().check_random_reentry()?;
    }
    match name {
        "read" => {
            let size = match args.first() {
                None | Some(&NONE) => crate::Int::from(-1),
                Some(&size) => ctx.index_integer(size)?,
            };
            let bytes = owner.borrow_mut().read_size(&size)?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "readinto" => {
            let target = args[0];
            let storage = ctx.scalar_value(target);
            let Value::ByteArray(bytes) = ctx.heap.get_mut(storage) else {
                let kind = if target == NONE {
                    "None"
                } else {
                    ctx.type_name(target)
                };
                return Err(Error::new(
                    "TypeError",
                    format!("readinto() argument must be read-write bytes-like object, not {kind}"),
                ));
            };
            let mut file = owner.borrow_mut();
            if file.closed() {
                return Err(Error::new("ValueError", "readinto of closed file"));
            }
            file.check_random_reentry()?;
            let count = file.read_into(bytes)?;
            Ok(ctx.int(count as i64))
        }
        "readline" => {
            let bytes = owner.borrow_mut().read_line_bytes()?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "write" => {
            let bytes = ctx.bytes_like(args[0])?;
            let mut file = owner.borrow_mut();
            file.check_random_reentry()?;
            file.write_bytes(bytes)?;
            let length = crate::Int::from_usize(bytes.len());
            Ok(ctx.alloc(Value::Int(length)))
        }
        "seek" => {
            let whence = super::files::buffered_whence(ctx, args.get(1).copied())?;
            let offset = ctx.index_integer(args[0])?;
            let position = owner.borrow_mut().seek_disk(&offset, i64::from(whence))?;
            Ok(ctx.int(position as i64))
        }
        "readable" | "writable" | "seekable" | "isatty" => {
            owner.borrow().fileno()?;
            Ok(crate::dynamic::builtins::boolean(name != "isatty"))
        }
        "truncate" => {
            let supplied = args.first().copied().filter(|&id| id != NONE);
            let busy = supplied
                .filter(|&id| !matches!(ctx.heap.get(id), Value::Int(_) | Value::Bool(_)))
                .map(|_| ctx.repr(file_name));
            owner.borrow_mut().begin_random_truncate(busy)?;
            let requested: Result<(i64, Id)> = (|| {
                if let Some(id) = supplied {
                    let size = ctx.index_integer(id)?.to_i64().ok_or_else(|| {
                        Error::new("OverflowError", "Python int too large to convert to C long")
                    })?;
                    Ok((size, id))
                } else {
                    let size = owner.borrow_mut().tell()? as i64;
                    Ok((size, ctx.int(size)))
                }
            })();
            owner
                .borrow_mut()
                .finish_random_truncate(requested.as_ref().ok().map(|&(size, _)| size))?;
            requested.map(|(_, original)| original)
        }
        "tell" => Ok(ctx.int(owner.borrow_mut().tell()? as i64)),
        "fileno" => Ok(ctx.int(i64::from(owner.borrow().fileno()?))),
        "flush" => {
            owner.borrow_mut().flush()?;
            Ok(NONE)
        }
        "close" | "__exit__" => {
            owner.borrow_mut().close().map_err(errno_error)?;
            Ok(NONE)
        }
        "__enter__" if owner.borrow().closed() => {
            Err(Error::new("ValueError", "I/O operation on closed file."))
        }
        "__enter__" => Ok(receiver),
        _ => unreachable!("only buffered random file methods are bound"),
    }
}
