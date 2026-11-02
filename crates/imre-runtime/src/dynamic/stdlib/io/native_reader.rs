//! BufferedReader over a native FileIO retains that raw object and its descriptor.

use super::super::{builtins, files, os_error};
use super::descriptor_buffer::{self, backing, ssize};
use super::{Context, Error, FALSE, Id, NONE, Result, Value};
use files::{BinaryFile, ReadSource};

pub(super) const METHODS: &[&str] = &[
    "__init__",
    "raw",
    "closed",
    "name",
    "mode",
    "readable",
    "seekable",
    "isatty",
    "fileno",
    "flush",
    "close",
    "tell",
    "seek",
    "read",
    "read1",
    "readinto",
    "readinto1",
    "truncate",
    "detach",
    "readline",
    "peek",
];

fn validate(owner: &str, name: &str, count: usize, kwargs: &[(String, Id)]) -> Result<()> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{owner}.{name}() takes no keyword arguments"),
        ));
    }
    let message = match name {
        "read" | "read1" | "readline" | "peek" | "truncate" if count > 1 => {
            format!("{name} expected at most 1 argument, got {count}")
        }
        "readinto" | "readinto1" if count != 1 => {
            format!("{owner}.{name}() takes exactly one argument ({count} given)")
        }
        "seek" if count == 0 => "seek expected at least 1 argument, got 0".into(),
        "seek" if count > 2 => format!("seek expected at most 2 arguments, got {count}"),
        "read" | "read1" | "readline" | "peek" | "readinto" | "readinto1" | "truncate" | "seek"
        | "__exit__" => {
            return Ok(());
        }
        _ if count != 0 => format!("{owner}.{name}() takes no arguments ({count} given)"),
        _ => return Ok(()),
    };
    Err(Error::new("TypeError", message))
}

pub(super) fn call(
    ctx: &mut Context,
    owner: &str,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "__init__" {
        return descriptor_buffer::initialize(ctx, receiver, args, kwargs, owner);
    }
    validate(owner, name, args.len(), kwargs)?;
    if name == "peek"
        && let Some(&size) = args.first()
    {
        ssize(ctx, size)?;
    }
    if name == "writable" {
        return Ok(FALSE);
    }
    if super::buffered_reader::has_state(ctx, receiver) {
        return super::buffered_reader::call(ctx, receiver, name, args);
    }
    if name == "readinto1" {
        return super::buffered_io::call(ctx, receiver, name, args, kwargs);
    }
    let target = if name == "readinto" {
        Some(files::readinto_target(ctx, args[0])?)
    } else {
        None
    };
    let args = target.as_ref().map_or(args, std::slice::from_ref);
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
    if name == "detach" {
        return Err(Error::new("UnsupportedOperation", "detach"));
    }
    let Value::File(handle) = ctx.heap.get(buffer) else {
        unreachable!("BufferedReader backing must be a binary file")
    };
    let handle = handle.clone();
    if name == "closed" {
        return Ok(builtins::boolean(handle.closed()));
    }
    if name == "truncate" {
        return Err(if handle.closed() {
            Error::new("ValueError", "truncate of closed file")
        } else {
            Error::new("UnsupportedOperation", "truncate")
        });
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
    if name == "peek" {
        let BinaryFile::Reader { slot, .. } = handle.as_ref() else {
            unreachable!("readable receiver")
        };
        let mut slot = slot.borrow_mut();
        let input = slot
            .as_mut()
            .ok_or_else(|| Error::new("ValueError", "peek of closed file"))?;
        return Ok(ctx.alloc(Value::Bytes(crate::objects::peek_bytes(input)?)));
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
