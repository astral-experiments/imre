//! Boxed gzip access to the lazy path reader and incremental DEFLATE encoder.

use std::cell::RefCell;

use super::{Context, Error, Id, NONE, Result, Value, os};
use crate::objects::FileState;

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let arity = if matches!(name, "read" | "read1" | "compress" | "flush_writer")
        || (name == "readline" && args.len() == 2)
    {
        2
    } else {
        1
    };
    if !kwargs.is_empty() || args.len() != arity {
        return Err(Error::new(
            "TypeError",
            format!("_imre_gzip.{name}() requires {arity} positional arguments"),
        ));
    }
    if matches!(name, "writer" | "compress" | "flush_writer") {
        return super::zlib::gzip_writer(ctx, name, args);
    }
    let receiver = args[0];
    if name == "open" {
        let receiver = super::posixpath::fspath(ctx, receiver)?;
        let path = os::encoded_path(ctx, receiver)?;
        let file = match FileState::gzip_reader(&path) {
            Ok(file) => file,
            Err(error) => return Err(ctx.with_errno_filename(error.into(), receiver)?),
        };
        let handle =
            crate::Shared::try_new(RefCell::new(file)).map_err(|_| crate::Error::allocation())?;
        return Ok(ctx.alloc(Value::GzipReader(handle)));
    }
    let Value::GzipReader(handle) = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected a gzip reader"));
    };
    let handle = handle.clone();
    match name {
        "read" => {
            if handle.borrow().closed() {
                return Err(Error::new("ValueError", "I/O operation on closed file"));
            }
            let size = ctx.index_integer(args[1])?;
            let bytes = handle.borrow_mut().read_size(&size)?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "read1" => {
            let size = os::ssize_t(ctx, args[1])?;
            let bytes = handle.borrow_mut().read_once(usize::try_from(size).ok())?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "readline" => {
            let size = super::files::read_length(ctx, args.get(1).copied())?;
            let mut file = handle.borrow_mut();
            if file.closed() {
                return Err(Error::new("ValueError", "readline of closed file"));
            }
            let bytes = if size == 0 {
                Vec::new()
            } else {
                file.read_line_limited(usize::try_from(size).ok())?
            };
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "tell" => Ok(ctx.alloc(Value::Int(crate::Int::new(
            handle.borrow_mut().tell()?.into(),
        )))),
        "fileno" => Ok(ctx.int(i64::from(handle.borrow().fileno()?))),
        "close" => {
            handle.borrow_mut().close().map_err(super::os_error)?;
            Ok(NONE)
        }
        _ => unreachable!("registered gzip primitive"),
    }
}
