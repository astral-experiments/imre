//! BufferedReader over a native FileIO retains that raw object and its descriptor.

use super::super::{builtins, files, os_error};
use super::descriptor_buffer::{self, backing, ssize};
use super::{Context, Error, FALSE, Id, NONE, Result, Value};
use files::{BinaryFile, ReadSource};

pub(super) const METHODS: &[&str] = &[
    "__init__",
    "__repr__",
    "_dealloc_warn",
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
        "readinto" | "readinto1" | "_dealloc_warn" if count != 1 => {
            format!("{owner}.{name}() takes exactly one argument ({count} given)")
        }
        "seek" if count == 0 => "seek expected at least 1 argument, got 0".into(),
        "seek" if count > 2 => format!("seek expected at most 2 arguments, got {count}"),
        "read" | "read1" | "readline" | "peek" | "readinto" | "readinto1" | "truncate" | "seek"
        | "__exit__" | "_dealloc_warn" => {
            return Ok(());
        }
        _ if count != 0 => format!("{owner}.{name}() takes no arguments ({count} given)"),
        _ => return Ok(()),
    };
    Err(Error::new("TypeError", message))
}

fn repr(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let name = match ctx.lookup_attr(receiver, "name") {
        Ok(name) => Some(name),
        Err(error)
            if ctx.is_builtin_error(&error, "AttributeError")
                || ctx.is_builtin_error(&error, "ValueError") =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    let class = ctx.exact_type(receiver)?;
    let class_name = if ctx.heap.get(class).io_kind() == Some("BufferedReader") {
        "_io.BufferedReader"
    } else {
        ctx.class_name(class)
    }
    .to_owned();
    let Some(name) = name else {
        return Ok(ctx.text(format!("<{class_name}>")));
    };
    if ctx.repr_ancestors.contains(&receiver) {
        return Err(Error::new(
            "RuntimeError",
            format!("reentrant call inside {class_name}.__repr__"),
        ));
    }
    ctx.repr_ancestors
        .try_reserve(1)
        .map_err(|_| crate::Error::allocation())?;
    ctx.repr_ancestors.push(receiver);
    let repr = ctx.builtin("repr");
    let shown = ctx.call(repr, vec![name], Vec::new());
    ctx.repr_ancestors.pop();
    let text = format!("<{class_name} name={}>", ctx.str_value(shown?)?);
    Ok(ctx.text(text))
}

/// A missing or failing raw warning hook must not interrupt flush or close.
pub(super) fn dealloc_warn(ctx: &mut Context, receiver: Id, source: Id) {
    let raw = if let Some(buffer) = backing(ctx, receiver)
        && let Value::File(handle) = ctx.heap.get(buffer)
        && let BinaryFile::Descriptor(fd) = handle.as_ref()
    {
        Some(fd.raw)
    } else {
        super::buffered_reader::initialized_raw(ctx, receiver)
    };
    if let Some(raw) = raw {
        let _ = super::text_wrapper::invoke(ctx, raw, "_dealloc_warn", vec![source]);
    }
}

/// A warning or flush override can replace the raw, even across backing types.
pub(super) fn flush_and_close(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let flush = super::text_wrapper::invoke(ctx, receiver, "flush", vec![]);
    if let Err(error) = &flush {
        for root in error.roots() {
            ctx.heap.keep(root);
        }
    }
    let close = if let Some(buffer) = backing(ctx, receiver)
        && let Value::File(handle) = ctx.heap.get(buffer)
        && let BinaryFile::Descriptor(fd) = handle.as_ref()
    {
        fd.close_after_flush(ctx, receiver).map(|_| NONE)
    } else {
        super::buffered_reader::close_raw(ctx, receiver)
    };
    match (flush, close) {
        (Err(flush), Err(mut close)) => {
            close.context = ctx.raised_instance(flush);
            Err(close)
        }
        (Err(flush), _) => Err(flush),
        (_, result) => result,
    }
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
    if name == "__repr__" {
        return repr(ctx, receiver);
    }
    if name == "peek"
        && let Some(&size) = args.first()
    {
        ssize(ctx, size)?;
    }
    if name == "writable" {
        return Ok(FALSE);
    }
    if name == "_dealloc_warn" {
        dealloc_warn(ctx, receiver, args[0]);
        return Ok(NONE);
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
            fd.warn_on_reader_close(ctx, receiver)?;
            return flush_and_close(ctx, receiver);
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
