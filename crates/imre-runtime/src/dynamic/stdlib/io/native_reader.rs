//! BufferedReader over a native FileIO retains that raw object and its descriptor.

use super::super::{builtins, files, os_error};
use super::{BufferedFd, Context, Error, FALSE, Id, MemoryStream, NONE, Result, TRUE, Value};
use super::{descriptor_buffer, file_io};
use files::{BinaryFile, ReadSource};

pub(super) const METHODS: &[&str] = &[
    "__init__", "raw", "closed", "name", "mode", "readable", "seekable", "isatty", "fileno",
    "flush", "close", "tell", "seek", "read", "read1", "readinto", "readline",
];

fn backing(ctx: &Context, receiver: Id) -> Option<Id> {
    match ctx.heap.get(receiver) {
        Value::File(_) => Some(receiver),
        Value::Instance {
            memory_stream: Some(MemoryStream::BufferedReader(buffer)),
            ..
        } => Some(*buffer),
        _ => None,
    }
}

fn native_raw(ctx: &Context, raw: Id, methods: &[&str]) -> Result<std::rc::Rc<file_io::RawFile>> {
    // Going straight to the descriptor is valid only while the Python raw methods
    // are native. The instance dictionary can change after the buffer is created.
    if ctx
        .heap
        .get(raw)
        .instance_class()
        .and_then(|class| ctx.heap.get(class).io_kind())
        == Some("FileIO")
        && !methods
            .iter()
            .any(|method| ctx.raw_instance_field(raw, method).is_some())
        && let Some(handle) = file_io::state(ctx, raw)
    {
        return Ok(handle);
    }
    Err(Error::new(
        "NotImplementedError",
        "BufferedReader with Python raw methods is unsupported",
    ))
}

/// Verify the native fast path after any argument conversion that can run Python.
pub(super) fn check_raw(ctx: &Context, receiver: Id, fd: &BufferedFd, name: &str) -> Result<()> {
    if matches!(ctx.heap.get(receiver), Value::File(_)) {
        return Ok(());
    }
    let methods: &[&str] = match name {
        "read" => &["read", "readall", "readinto"],
        "read1" | "readline" | "readinto" => &["readinto"],
        "seek" => &["seek", "seekable", "tell"],
        "close" => &["flush", "close"],
        _ => std::slice::from_ref(&name),
    };
    if fd.uses_raw(&native_raw(ctx, fd.raw, methods)?) {
        Ok(())
    } else {
        Err(Error::new(
            "NotImplementedError",
            "reinitialized BufferedReader raw FileIO is unsupported",
        ))
    }
}

fn ssize(ctx: &mut Context, value: Id) -> Result<i64> {
    ctx.index_integer(value)?.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

fn initialize(ctx: &mut Context, receiver: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [raw, capacity] = builtins::bind_builtin_arguments(
        "BufferedReader",
        args,
        kwargs,
        ["raw", "buffer_size"],
        1,
    )?;
    let raw = raw.expect("required argument");
    let capacity = capacity
        .map(|n| ssize(ctx, n))
        .transpose()?
        .unwrap_or(super::DEFAULT_BUFFER_SIZE);

    let readable = ctx.lookup_attr(raw, "readable")?;
    if ctx.call(readable, vec![], Vec::new())? != TRUE {
        return Err(Error::new(
            "UnsupportedOperation",
            "File or stream is not readable.",
        ));
    }
    if capacity <= 0 {
        return Err(Error::new(
            "ValueError",
            "buffer size must be strictly positive",
        ));
    }
    let raw_handle = native_raw(
        ctx,
        raw,
        &[
            "close", "flush", "readable", "seekable", "isatty", "fileno", "tell", "seek", "read",
            "readall", "readinto",
        ],
    )?;
    if backing(ctx, receiver).is_some() {
        return Err(Error::new(
            "NotImplementedError",
            "BufferedReader reinitialization is unsupported",
        ));
    }
    // BufReader::with_capacity allocates infallibly. Bound user-controlled sizes
    // to the same maximum that open() passes to the native buffer.
    if capacity as u64 > crate::text_output::binary_capacity(i128::MAX) as u64 {
        return Err(Error::new(
            "NotImplementedError",
            "explicit BufferedReader buffers above 8 MiB are unsupported",
        ));
    }
    let buffer = BufferedFd::new(raw, raw_handle, true, capacity as usize);
    let buffer = ctx.alloc(BinaryFile::Descriptor(buffer).into_value()?);
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!("uninitialized BufferedReader instance")
    };
    *memory_stream = Some(MemoryStream::BufferedReader(buffer));
    Ok(NONE)
}

fn validate(name: &str, count: usize, kwargs: &[(String, Id)]) -> Result<()> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("BufferedReader.{name}() takes no keyword arguments"),
        ));
    }
    let message = match name {
        "read" | "read1" | "readline" if count > 1 => {
            format!("{name} expected at most 1 argument, got {count}")
        }
        "readinto" if count != 1 => {
            format!("BufferedReader.{name}() takes exactly one argument ({count} given)")
        }
        "seek" if count == 0 => "seek expected at least 1 argument, got 0".into(),
        "seek" if count > 2 => format!("seek expected at most 2 arguments, got {count}"),
        "read" | "read1" | "readline" | "readinto" | "seek" | "__exit__" => return Ok(()),
        _ if count != 0 => format!("BufferedReader.{name}() takes no arguments ({count} given)"),
        _ => return Ok(()),
    };
    Err(Error::new("TypeError", message))
}

pub(super) fn call(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "__init__" {
        return initialize(ctx, receiver, args, kwargs);
    }
    validate(name, args.len(), kwargs)?;
    let target = if name == "readinto" {
        Some(files::readinto_target(ctx, args[0])?)
    } else {
        None
    };
    let args = target.as_ref().map_or(args, std::slice::from_ref);
    if name == "writable" {
        return Ok(FALSE);
    }
    if name == "__exit__" {
        let close = ctx.lookup_attr(receiver, "close")?;
        return ctx.call(close, Vec::new(), Vec::new());
    }
    let Some(buffer) = backing(ctx, receiver) else {
        return if name == "raw" {
            Ok(NONE)
        } else {
            Err(Error::new(
                "ValueError",
                "I/O operation on uninitialized object",
            ))
        };
    };
    let Value::File(handle) = ctx.heap.get(buffer) else {
        unreachable!("BufferedReader backing must be a binary file")
    };
    let handle = handle.clone();
    if name == "closed" {
        return Ok(builtins::boolean(handle.closed()));
    }
    if let BinaryFile::Descriptor(fd) = handle.as_ref() {
        if name == "raw" {
            return Ok(fd.raw);
        }
        if matches!(name, "name" | "mode") {
            return ctx.lookup_attr(fd.raw, name);
        }
        if receiver != buffer && name == "close" && !fd.closed() {
            let flushed = ctx
                .lookup_attr(receiver, "flush")
                .and_then(|flush| ctx.call(flush, vec![], Vec::new()));
            fd.call(ctx, receiver, name, args, kwargs)?;
            return flushed.map(|_| NONE);
        }
        return fd.call(ctx, receiver, name, args, kwargs);
    }
    if matches!(name, "name" | "mode" | "raw") {
        if let Some((path, mode)) = handle.path_metadata() {
            match name {
                "name" => return Ok(path),
                "mode" => return Ok(ctx.text(mode)),
                _ => {}
            }
        } else if name == "name"
            && let Some(fd) = handle.socket_reader_name()
        {
            return Ok(ctx.int(i64::from(fd)));
        }
        return Err(Error::new(
            "AttributeError",
            format!(
                "'{}' object has no attribute '{name}'",
                ctx.type_name(receiver)
            ),
        ));
    }
    if name == "read1" {
        let size = args
            .first()
            .copied()
            .map(|n| ssize(ctx, n))
            .transpose()?
            .filter(|&n| n >= 0)
            .unwrap_or(super::DEFAULT_BUFFER_SIZE);
        let BinaryFile::Reader { slot, .. } = handle.as_ref() else {
            unreachable!("readable receiver")
        };
        let mut slot = slot.borrow_mut();
        let source = slot
            .as_mut()
            .ok_or_else(|| Error::new("ValueError", "read of closed file"))?;
        let data = match source {
            ReadSource::File(reader) => descriptor_buffer::read_once(reader, size)?,
            ReadSource::Socket(socket) => {
                let mut data = builtins::zeroed_bytes(size)?;
                let n = socket.read1(&mut data).map_err(os_error)?;
                data.truncate(n);
                data
            }
        };
        return Ok(ctx.alloc(Value::Bytes(data)));
    }
    let mut operands = vec![buffer];
    operands.extend_from_slice(args);
    let result = files::call(ctx, &format!("file.{name}"), &operands, kwargs)?;
    Ok(if name == "__enter__" {
        receiver
    } else {
        result
    })
}
