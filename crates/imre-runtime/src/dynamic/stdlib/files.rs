use super::{Context, Error, Id, NONE, Result, TRUE, Value, os_error};
use crate::sys::StandardBuffer;

#[derive(Clone)]
pub(in crate::dynamic) enum BinaryFile {
    Descriptor(crate::Shared<super::io::BufferedFd>),
    Standard {
        file: triomphe::Arc<StandardBuffer>,
        path: Id,
    },
}

impl BinaryFile {
    pub(in crate::dynamic) fn path_metadata(&self) -> Option<(Id, &'static str)> {
        match self {
            Self::Standard { path, .. } => Some((*path, "wb")),
            _ => None,
        }
    }

    pub(super) fn descriptor(fd: super::io::BufferedFd) -> Result<Value> {
        let fd = crate::Shared::try_new(fd).map_err(|_| Error::new("MemoryError", ""))?;
        Ok(Value::File(super::super::value::BoxedFile::new(
            Self::Descriptor(fd),
        )))
    }

    pub(super) fn standard(stderr: bool, path: Id) -> Result<Value> {
        let file = triomphe::Arc::try_new(StandardBuffer::new(stderr))
            .map_err(|_| Error::new("MemoryError", ""))?;
        Ok(Value::File(super::super::value::BoxedFile::new(
            Self::Standard { file, path },
        )))
    }

    pub(in crate::dynamic) fn closed(&self) -> bool {
        match self {
            Self::Descriptor(fd) => fd.closed(),
            Self::Standard { file, .. } => file.closed(),
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
    match handle.as_ref() {
        BinaryFile::Descriptor(file) => file.call(ctx, receiver, name, args),
        BinaryFile::Standard { file, .. } => {
            standard_buffer_method(ctx, file, receiver, name, args)
        }
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
    file: &StandardBuffer,
    receiver: Id,
    name: &str,
    args: &[Id],
) -> Result<Id> {
    const CLOSED: &str = "I/O operation on closed file.";
    match (name, args) {
        ("close", []) | ("__exit__", [_, _, _]) => {
            if file.is_stderr() && !file.closed() {
                ctx.check_thread_reporter_mutable()?;
            }
            file.close()?;
            Ok(NONE)
        }
        ("write", [value]) => {
            let data = output_bytes(ctx, *value)?;
            file.write(data)?;
            Ok(ctx.int(data.len() as i64))
        }
        ("flush", []) => {
            file.flush()?;
            Ok(NONE)
        }
        ("tell", []) => Ok(ctx.int(file.position(None)?)),
        ("seekable", []) => Ok(crate::dynamic::builtins::boolean(file.seekable(CLOSED)?)),
        ("seek", [target] | [target, _]) => {
            let whence = buffered_whence(ctx, args.get(1).copied())?;
            if !file.seekable("seek of closed file")? {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "File or stream is not seekable.",
                ));
            }
            let offset = buffered_offset(ctx, *target)?;
            Ok(ctx.int(file.position(Some((offset, whence)))?))
        }
        ("fileno", []) => Ok(ctx.int(i64::from(file.fileno(CLOSED)?))),
        ("writable", []) => {
            file.check_open(CLOSED)?;
            Ok(TRUE)
        }
        ("isatty", []) => Ok(crate::dynamic::builtins::boolean(
            crate::objects::os::isatty(file.fileno("I/O operation on closed file")?),
        )),
        ("__enter__", []) => {
            file.check_open(CLOSED)?;
            Ok(receiver)
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to file.{name}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_the_native_standard_buffer_is_visible_through_its_boxed_alias() {
        let Value::File(buffer) = BinaryFile::standard(true, NONE).unwrap() else {
            unreachable!()
        };
        let BinaryFile::Standard { file, .. } = buffer.as_ref() else {
            unreachable!()
        };
        let peer = file.clone();
        assert!(!buffer.closed());
        std::thread::spawn(move || {
            assert_eq!(peer.fileno("closed").ok(), Some(libc::STDERR_FILENO));
            assert!(peer.close().is_ok());
        })
        .join()
        .unwrap();
        assert!(buffer.closed());
        assert!(file.close().is_ok());
    }
}
