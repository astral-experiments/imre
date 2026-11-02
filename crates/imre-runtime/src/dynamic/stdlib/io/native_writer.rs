//! BufferedWriter retains the original FileIO and writes through its shared descriptor.

use super::super::{builtins, files};
use super::{Context, Error, FALSE, Id, NONE, Result, Value, descriptor_buffer, file_io};
use files::BinaryFile;

pub(super) const METHODS: &[&str] = &[
    "__init__", "raw", "closed", "name", "mode", "writable", "seekable", "isatty", "fileno",
    "flush", "close", "tell", "seek", "write", "truncate", "detach",
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
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "__init__" {
        return descriptor_buffer::initialize(ctx, receiver, args, kwargs, false);
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
            format!("BufferedWriter.{name}() takes no keyword arguments"),
        ));
    }
    super::argument_count("BufferedWriter", name, args.len())?;
    if name == "write" {
        files::output_bytes(ctx, args[0])?;
    }
    if name == "readable" {
        return Ok(FALSE);
    }
    if name == "__exit__" {
        let close = ctx.lookup_attr(receiver, "close")?;
        return ctx.call(close, vec![], Vec::new());
    }
    if let Some(writer) = super::bytes_writer::state(ctx, receiver) {
        return writer.call(ctx, receiver, name, args);
    }
    if name == "detach" {
        return Err(Error::new("UnsupportedOperation", "detach"));
    }
    let Some(buffer) = descriptor_buffer::backing(ctx, receiver) else {
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
        return match name {
            "raw" => Ok(fd.raw),
            "name" | "mode" => ctx.lookup_attr(fd.raw, name),
            "truncate" => {
                fd.call(ctx, receiver, "flush", &[], &[])?;
                let truncate = ctx.lookup_attr(fd.raw, "truncate")?;
                ctx.call(truncate, args.to_vec(), Vec::new())
            }
            "close" if buffer != receiver && !fd.closed() => {
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
        };
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
