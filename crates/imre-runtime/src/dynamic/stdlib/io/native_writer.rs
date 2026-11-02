//! BufferedWriter retains the original FileIO and writes through its shared descriptor.

use std::cell::Cell;

use super::super::{builtins, files};
use super::{
    Context, Error, FALSE, Id, MemoryStream, NONE, Result, Value, descriptor_buffer, file_io,
};
use files::BinaryFile;

/// A raw callback must not switch the backing under a native operation already using it.
#[derive(Default)]
pub(super) struct ActiveCall(Cell<bool>);

impl ActiveCall {
    pub(super) fn check_idle(&self) -> Result<()> {
        if self.0.get() {
            return Err(Error::new(
                "NotImplementedError",
                "reinitializing a buffered writer during an active I/O operation is unsupported",
            ));
        }
        Ok(())
    }

    fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        let previous = self.0.replace(true);
        let result = f();
        self.0.set(previous);
        result
    }
}

pub(super) const METHODS: &[&str] = &[
    "__init__",
    "__repr__",
    "_dealloc_warn",
    "raw",
    "closed",
    "name",
    "mode",
    "writable",
    "seekable",
    "isatty",
    "fileno",
    "flush",
    "close",
    "tell",
    "seek",
    "write",
    "truncate",
    "detach",
    "__getstate__",
];

/// Path and standard writers own their fd internally and have no Python FileIO to delegate to.
fn truncate_file(ctx: &mut Context, buffer: Id, size: Option<Id>) -> Result<Id> {
    files::call(ctx, "file.flush", &[buffer], &[])?;
    let size = match size.filter(|&value| value != NONE) {
        Some(value) => ctx.index_integer(value)?.to_i64().ok_or_else(|| {
            Error::new("OverflowError", "Python int too large to convert to C long")
        })?,
        None => {
            let position = files::call(ctx, "file.tell", &[buffer], &[])?;
            ctx.as_integer(position)?
        }
    };
    // The position conversion can run Python and close or replace the underlying descriptor.
    let fd = files::call(ctx, "file.fileno", &[buffer], &[])?;
    file_io::truncate_fd(ctx.as_integer(fd)? as i32, size)?;
    Ok(ctx.int(size))
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
    if name == "truncate" {
        let count = args.len() + kwargs.len();
        if count > 1 {
            return Err(Error::new(
                "TypeError",
                format!("truncate() takes at most 1 argument ({count} given)"),
            ));
        }
        if let Some((key, _)) = kwargs.first() {
            return Err(Error::new(
                "TypeError",
                format!("truncate() got an unexpected keyword argument '{key}'"),
            ));
        }
    } else if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{owner}.{name}() takes no keyword arguments"),
        ));
    }
    if name == "_dealloc_warn" {
        let [source] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{owner}.{name}() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        super::buffered_io::dealloc_warn(ctx, receiver, *source);
        return Ok(NONE);
    }
    super::argument_count(owner, name, args.len())?;
    if name == "__repr__" {
        return super::buffered_io::repr(ctx, receiver);
    }
    if name == "__getstate__" {
        return super::buffered_io::getstate(ctx, receiver);
    }
    if name == "write" {
        files::output_bytes(ctx, args[0])?;
    }
    if name == "readable" && owner == "BufferedWriter" {
        return Ok(FALSE);
    }
    if name == "__exit__" {
        let close = ctx.lookup_attr(receiver, "close")?;
        return ctx.call(close, vec![], Vec::new());
    }
    if let Some(writer) = super::bytes_writer::state(ctx, receiver) {
        return writer.active.run(|| {
            if name == "write" {
                ctx.with_buffer_export(args[0], |ctx| writer.call(ctx, receiver, name, args))
            } else {
                writer.call(ctx, receiver, name, args)
            }
        });
    }
    let Some(buffer) = descriptor_buffer::backing(ctx, receiver) else {
        return if name == "raw" {
            let raw = match ctx.heap.get(receiver) {
                Value::Instance {
                    memory_stream: Some(MemoryStream::UninitializedWriter(raw)),
                    ..
                } => *raw,
                _ => None,
            };
            Ok(raw.unwrap_or(NONE))
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
        unreachable!("BufferedWriter backing must be a binary file")
    };
    let handle = handle.clone();
    if name == "closed" {
        return Ok(builtins::boolean(handle.closed()));
    }
    if name == "truncate" && handle.closed() {
        return Err(Error::new("ValueError", "truncate of closed file"));
    }
    if let BinaryFile::Descriptor(fd) = handle.as_ref() {
        return fd.active.run(|| match name {
            "raw" => Ok(fd.raw),
            "name" | "mode" => ctx.lookup_attr(fd.raw, name),
            "truncate" if owner == "BufferedRandom" => fd.call(ctx, receiver, name, args, kwargs),
            "truncate" => {
                fd.call(ctx, receiver, "flush", &[], &[])?;
                let truncate = ctx.lookup_attr(fd.raw, "truncate")?;
                ctx.call(truncate, args.to_vec(), Vec::new())
            }
            "close" if buffer != receiver && !fd.closed() => {
                if owner == "BufferedWriter" {
                    fd.warn_on_close(ctx, receiver)?;
                }
                let flushed = ctx
                    .lookup_attr(receiver, "flush")
                    .and_then(|flush| ctx.call(flush, vec![], Vec::new()));
                if let Err(error) = &flushed {
                    for id in error.roots() {
                        ctx.heap.keep(id);
                    }
                }
                match (flushed, fd.close_after_flush(ctx, receiver)) {
                    (Err(flush), Err(mut close)) => {
                        close.context = ctx.raised_instance(flush);
                        Err(close)
                    }
                    (Err(flush), _) => Err(flush),
                    (_, closed) => closed.map(|()| NONE),
                }
            }
            _ => fd.call(ctx, receiver, name, args, kwargs),
        });
    }
    let (path, mode) = handle
        .path_metadata()
        .expect("native writer has path metadata");
    match name {
        "name" => return Ok(path),
        "mode" => return Ok(ctx.text(mode)),
        "raw" => {
            return Err(Error::new(
                "AttributeError",
                format!(
                    "'{}' object has no attribute 'raw'",
                    ctx.type_name(receiver)
                ),
            ));
        }
        "truncate" => return truncate_file(ctx, buffer, args.first().copied()),
        _ => {}
    }
    let mut operands = vec![buffer];
    operands.extend_from_slice(args);
    let value = files::call(ctx, &format!("file.{name}"), &operands, kwargs)?;
    Ok(if name == "__enter__" { receiver } else { value })
}
