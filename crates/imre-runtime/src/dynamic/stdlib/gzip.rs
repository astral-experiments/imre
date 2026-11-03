//! Boxed access to the same lazy, path-backed gzip decoder as typed reads.

use std::cell::RefCell;

use super::{Context, Error, Id, NONE, Result, Value, os};
use crate::objects::FileState;

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let arity = if name == "read" { 2 } else { 1 };
    if !kwargs.is_empty() || args.len() != arity {
        return Err(Error::new(
            "TypeError",
            format!("_imre_gzip.{name}() requires {arity} positional arguments"),
        ));
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
        "readline" => {
            let bytes = handle.borrow_mut().read_line_bytes()?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "fileno" => Ok(ctx.int(i64::from(handle.borrow().fileno()?))),
        "close" => {
            handle.borrow_mut().close().map_err(super::os_error)?;
            Ok(NONE)
        }
        _ => unreachable!("registered gzip primitive"),
    }
}
