use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value, argument, os_error};

/// Binary handles retain their access mode after close.
pub(in crate::dynamic) enum BinaryFile {
    Descriptor(super::io::BufferedFd),
    // The original sys stream owns its encoder and the shared process binary buffer.
    Standard {
        file: crate::objects::TextFile,
        path: Id,
    },
    Reader {
        slot: RefCell<Option<BufReader<std::fs::File>>>,
        path: Id,
    },
    Writer {
        file: RefCell<crate::objects::FileState>,
        path: Id,
        mode: &'static str,
    },
}

impl BinaryFile {
    /// Wrap the descriptor acquired by open() without performing a second path lookup.
    pub(in crate::dynamic) fn from_open(
        path: &str,
        name: Id,
        access: u32,
        mode: &'static str,
        writer_capacity: Option<usize>,
        fd: std::os::fd::OwnedFd,
    ) -> Result<Self> {
        let opened = crate::objects::prepare_file(fd, access, |error| {
            crate::objects::errno_with_filename(error, Some(path))
        })?;
        if access == 0 {
            let capacity = crate::text_output::binary_capacity(opened.1.blksize().into());
            return Ok(Self::Reader {
                path: name,
                slot: RefCell::new(Some(BufReader::with_capacity(capacity, opened.0))),
            });
        }
        Ok(Self::Writer {
            file: RefCell::new(crate::objects::FileState::binary_writer(
                path,
                access,
                writer_capacity,
                opened,
            )?),
            path: name,
            mode,
        })
    }

    /// Filename objects and binary access modes remain observable after close.
    pub(in crate::dynamic) fn path_metadata(&self) -> Option<(Id, &'static str)> {
        match self {
            Self::Reader { path, .. } => Some((*path, "rb")),
            Self::Writer { path, mode, .. } => Some((*path, mode)),
            Self::Standard { path, .. } => Some((*path, "wb")),
            _ => None,
        }
    }

    pub(in crate::dynamic) fn into_value(self) -> Result<Value> {
        crate::shared::Shared::try_new(self)
            .map(|file| Value::File(super::super::value::BoxedFile::new(file)))
            .map_err(|_| Error::new("MemoryError", ""))
    }

    /// All aliases observe closure through the same buffered stream.
    /// A callback may check frame roots while a file operation holds its state.
    pub(in crate::dynamic) fn owns_open_descriptor(&self) -> bool {
        match self {
            Self::Reader { slot, .. } => !slot.try_borrow().is_ok_and(|slot| slot.is_none()),
            Self::Writer { file, .. } => !file.try_borrow().is_ok_and(|file| file.closed()),
            _ => !self.closed(),
        }
    }

    pub(in crate::dynamic) fn closed(&self) -> bool {
        match self {
            Self::Descriptor(fd) => fd.closed(),
            Self::Standard { file, .. } => file.closed(),
            Self::Reader { slot, .. } => slot.borrow().is_none(),
            Self::Writer { file, .. } => file.borrow().closed(),
        }
    }
}

/// Borrow a binary writer's input, including the native bytes of an array.
pub(super) fn output_bytes(ctx: &Context, value: Id) -> Result<&[u8]> {
    match ctx.heap.get(value) {
        Value::Array(array) => Ok(array.as_bytes()),
        _ => ctx.bytes_like(value),
    }
}

pub(super) fn read_length(ctx: &mut Context, size: Option<Id>) -> Result<i64> {
    let Some(id) = size.filter(|&id| id != NONE) else {
        return Ok(-1);
    };
    let integer = ctx.index_value(id)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "argument should be integer or None, not '{}'",
                ctx.type_name(id)
            ),
        )
    })?;
    integer.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            format!(
                "cannot fit '{}' into an index-sized integer",
                ctx.type_name(id)
            ),
        )
    })
}

/// Validate a writable native destination before the file consumes any input.
pub(super) fn readinto_target(ctx: &Context, object: Id) -> Result<Id> {
    if let Some((storage, _)) = ctx.writable_byte_span(object)? {
        // Some paths validate before delegating. Keep a callback's byte window intact.
        return Ok(
            if matches!(
                ctx.heap.get(object),
                Value::MemoryView { .. } | Value::BytesIoBuffer { .. }
            ) {
                object
            } else {
                storage
            },
        );
    }
    let kind = if object == NONE {
        "None"
    } else {
        ctx.type_name(object)
    };
    Err(Error::new(
        "TypeError",
        format!("readinto() argument must be read-write bytes-like object, not {kind}"),
    ))
}

pub(super) fn readinto_bytes(ctx: &mut Context, object: Id) -> Result<&mut [u8]> {
    let object = readinto_target(ctx, object)?;
    let (target, range) = ctx
        .writable_byte_span(object)?
        .expect("validated writable buffer");
    let bytes = ctx.writable_storage_bytes(target)?;
    Ok(&mut bytes[range])
}

/// A buffered read encountering EAGAIN returns its copied prefix count, or None if still empty.
pub(super) fn readinto_error(ctx: &mut Context, error: std::io::Error, count: usize) -> Result<Id> {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Ok(if count == 0 {
            NONE
        } else {
            ctx.int(count as i64)
        })
    } else {
        Err(os_error(error))
    }
}

pub(super) fn binary_method(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
) -> Result<Id> {
    let Value::File(handle) = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected a file"));
    };
    let handle = handle.clone();
    let slot = match handle.as_ref() {
        BinaryFile::Descriptor(file) => return file.call(ctx, receiver, name, args),
        BinaryFile::Standard { file, .. } => {
            return standard_buffer_method(ctx, file, receiver, name, args);
        }
        BinaryFile::Reader { slot, .. } => slot,
        BinaryFile::Writer { file, .. } => {
            if name == "seek" {
                let whence = whence_argument(ctx, args.get(1).copied())?;
                let offset = ctx.index_integer(argument(args, 0)?)?;
                let pos = file.borrow_mut().seek_disk(&offset, i64::from(whence))?;
                return Ok(ctx.int(pos as i64));
            }
            if matches!((name, args), ("close", []) | ("__exit__", [_, _, _])) {
                let closed = file.borrow().closed();
                if let Some(result) =
                    super::io::overridden_close(ctx, receiver, name, closed, |_| {
                        file.borrow_mut()
                            .close_binary_after_flush()
                            .map_err(crate::objects::errno_error)?;
                        Ok(NONE)
                    })
                {
                    return result;
                }
                file.borrow_mut()
                    .close()
                    .map_err(crate::objects::errno_error)?;
                return Ok(NONE);
            }
            let mut file = file.borrow_mut();
            return match (name, args) {
                ("write", [value]) => {
                    let bytes = output_bytes(ctx, *value)?;
                    file.write_bytes(bytes)?;
                    Ok(ctx.int(bytes.len() as i64))
                }
                ("flush", []) => {
                    file.flush()?;
                    Ok(NONE)
                }
                ("__enter__", []) if file.closed() => {
                    Err(Error::new("ValueError", "I/O operation on closed file."))
                }
                ("__enter__", []) => Ok(receiver),
                ("fileno", []) => Ok(ctx.int(i64::from(file.fileno()?))),
                ("tell", []) => Ok(ctx.int(file.tell()? as i64)),
                ("writable", []) => {
                    file.fileno()?;
                    Ok(TRUE)
                }
                ("seekable", []) => {
                    file.fileno()?;
                    Ok(crate::dynamic::builtins::boolean(file.tell().is_ok()))
                }
                ("isatty", []) => Ok(crate::dynamic::builtins::boolean(
                    crate::objects::os::isatty(file.fileno()?),
                )),
                _ => Err(Error::new(
                    "TypeError",
                    format!("invalid arguments to file.{name}"),
                )),
            };
        }
    };
    // A Python size conversion may read from or close this same handle.
    let length = match name {
        "read" | "readline" => Some(read_length(ctx, args.first().copied())?),
        _ => None,
    };
    if name == "seek" {
        let target = argument(args, 0)?;
        let whence = buffered_whence(ctx, args.get(1).copied())?;
        {
            let mut slot = slot.borrow_mut();
            let reader = slot
                .as_mut()
                .ok_or_else(|| Error::new("ValueError", "seek of closed file"))?;
            if reader.get_mut().stream_position().is_err() {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "File or stream is not seekable.",
                ));
            }
        }
        // An offset conversion may advance or close this same stream.
        let offset = buffered_offset(ctx, target)?;
        let mut slot = slot.borrow_mut();
        let reader = slot
            .as_mut()
            .ok_or_else(|| Error::new("ValueError", "I/O operation on closed file"))?;
        if whence == libc::SEEK_SET && offset < 0 {
            // A seekable device may accept a negative absolute offset. Ask the
            // underlying file without discarding unread buffered data on failure.
            // SAFETY: the borrowed file keeps its descriptor open for the call.
            let position = unsafe {
                libc::lseek(
                    reader.get_ref().as_raw_fd(),
                    offset as libc::off_t,
                    libc::SEEK_SET,
                )
            };
            if position < 0 {
                return Err(os_error(std::io::Error::last_os_error()));
            }
            let remaining = reader.buffer().len();
            reader.consume(remaining);
            return Ok(ctx.int(position as i64));
        }
        let pos = match whence {
            0 => SeekFrom::Start(offset as u64),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return Err(Error::new("ValueError", "invalid seek whence")),
        };
        return Ok(ctx.int(reader.seek(pos).map_err(os_error)? as i64));
    }
    let mut slot = slot.borrow_mut();
    if matches!(name, "__exit__" | "close") {
        slot.take();
        return Ok(NONE);
    }
    let reader = slot.as_mut().ok_or_else(|| {
        let message = match name {
            "read" => "read of closed file",
            "readline" => "readline of closed file",
            "readinto" => "readinto of closed file",
            "__enter__" | "flush" => "I/O operation on closed file.",
            _ => "I/O operation on closed file",
        };
        Error::new("ValueError", message)
    });
    if let Some(length) = length {
        if name == "read" && length < -1 {
            return Err(Error::new(
                "ValueError",
                "read length must be non-negative or -1",
            ));
        }
        let mut bytes = Vec::new();
        let mut input = reader?.take(u64::try_from(length).unwrap_or(u64::MAX));
        let requested = if name == "read" {
            usize::try_from(length).ok()
        } else {
            None
        };
        if let Some(size) = requested {
            if size > crate::MAX_BYTES_LEN {
                return Err(Error::new("OverflowError", "byte string is too large"));
            }
            bytes
                .try_reserve_exact(size)
                .map_err(|_| Error::new("MemoryError", ""))?;
        }
        if name == "readline" {
            input.read_until(b'\n', &mut bytes)
        } else {
            input.read_to_end(&mut bytes)
        }
        .map_err(os_error)?;
        if requested.is_some_and(|size| bytes.len() < size) {
            crate::compact_bytes(&mut bytes);
        }
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    let f = reader?;
    match name {
        "__enter__" => Ok(receiver),
        "readable" => Ok(TRUE),
        "seekable" => Ok(crate::dynamic::builtins::boolean(
            f.get_mut().stream_position().is_ok(),
        )),
        "isatty" => {
            use std::io::IsTerminal;
            Ok(if f.get_ref().is_terminal() {
                TRUE
            } else {
                FALSE
            })
        }
        "flush" => Ok(NONE),
        "fileno" => Ok(ctx.int(i64::from(f.get_ref().as_raw_fd()))),
        "tell" => Ok(ctx.int(f.stream_position().map_err(os_error)? as i64)),
        "readinto" => {
            let buffer = readinto_bytes(ctx, argument(args, 0)?)?;
            let mut count = 0;
            while count < buffer.len() {
                match f.read(&mut buffer[count..]) {
                    Ok(0) => break,
                    Ok(n) => count += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return readinto_error(ctx, e, count),
                }
            }
            Ok(ctx.int(count as i64))
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("unknown native file method file.{name}"),
        )),
    }
}

pub(super) fn whence_argument(ctx: &mut Context, value: Option<Id>) -> Result<i32> {
    match value {
        None => Ok(0),
        Some(value) => ctx
            .index_integer(value)?
            .to_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int")),
    }
}

pub(super) fn check_buffered_whence(whence: i32) -> Result<i32> {
    if !matches!(
        whence,
        libc::SEEK_SET | libc::SEEK_CUR | libc::SEEK_END | libc::SEEK_DATA | libc::SEEK_HOLE
    ) {
        return Err(Error::new(
            "ValueError",
            format!("whence value {whence} unsupported"),
        ));
    }
    Ok(whence)
}

pub(super) fn buffered_whence(ctx: &mut Context, value: Option<Id>) -> Result<i32> {
    check_buffered_whence(whence_argument(ctx, value)?)
}

pub(super) fn buffered_offset(ctx: &mut Context, target: Id) -> Result<i64> {
    ctx.index_integer(target)?.to_i64().ok_or_else(|| {
        Error::new(
            "ValueError",
            format!(
                "cannot fit '{}' into an offset-sized integer",
                ctx.type_name(target)
            ),
        )
    })
}

fn standard_buffer_method(
    ctx: &mut Context,
    file: &crate::objects::TextFile,
    receiver: Id,
    name: &str,
    args: &[Id],
) -> Result<Id> {
    if matches!((name, args), ("close", []) | ("__exit__", [_, _, _])) {
        if !file.closed() {
            if file.standard_writer()? {
                ctx.check_thread_reporter_mutable()?;
            }
            file.close_standard_buffer()?;
        }
        return Ok(NONE);
    }
    match (name, args) {
        ("write", [value]) => {
            let data = output_bytes(ctx, *value)?;
            if file.closed() {
                return Err(Error::new("ValueError", "write to closed file"));
            }
            crate::sys::write_standard_buffer(file.standard_writer()?, data)?;
            Ok(ctx.int(data.len() as i64))
        }
        ("flush", []) => {
            if file.closed() {
                return Err(Error::new("ValueError", "flush of closed file"));
            }
            crate::sys::flush_standard_buffer(file.standard_writer()?).map_err(os_error)?;
            Ok(NONE)
        }
        ("tell" | "seekable", []) => {
            let position = crate::sys::standard_buffer_position(file.standard_writer()?, None);
            if name == "seekable" {
                Ok(crate::dynamic::builtins::boolean(position.is_ok()))
            } else {
                Ok(ctx.int(position?))
            }
        }
        ("seek", [target] | [target, _]) => {
            let whence = buffered_whence(ctx, args.get(1).copied())?;
            if file.closed() {
                return Err(Error::new("ValueError", "seek of closed file"));
            }
            if crate::sys::standard_buffer_position(file.standard_writer()?, None).is_err() {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "File or stream is not seekable.",
                ));
            }
            let offset = buffered_offset(ctx, *target)?;
            let pos = crate::sys::standard_buffer_position(
                file.standard_writer()?,
                Some((offset, whence)),
            )?;
            Ok(ctx.int(pos))
        }
        ("fileno", []) => Ok(ctx.int(if file.standard_writer()? { 2 } else { 1 })),
        ("writable", []) => {
            file.standard_writer()?;
            Ok(TRUE)
        }
        ("isatty", []) => Ok(crate::dynamic::builtins::boolean(
            crate::objects::os::isatty(file.fileno()?),
        )),
        ("__enter__", []) => {
            file.standard_writer()?;
            Ok(receiver)
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to file.{name}"),
        )),
    }
}
