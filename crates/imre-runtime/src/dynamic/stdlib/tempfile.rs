//! Runtime-imported binary update files backed by the native anonymous file owner.

use std::cell::RefCell;

use super::{Context, Error, Id, NONE, Result, Value, argument, option};
use crate::objects::{FileState, errno_error};

pub(in crate::dynamic) const METHODS: &[&str] = &[
    "read",
    "readline",
    "write",
    "seek",
    "tell",
    "fileno",
    "flush",
    "close",
    "__enter__",
    "__exit__",
];

/// Opens the binary update modes supported by the native TemporaryFile constructor.
pub(super) fn open(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if args.len() > 1
        || kwargs
            .iter()
            .any(|(key, _)| !matches!(key.as_str(), "mode" | "dir"))
    {
        return Err(Error::new(
            "TypeError",
            "unsupported arguments to TemporaryFile",
        ));
    }
    if let Some(mode) = option(args, 0, kwargs, "mode")?
        && !matches!(ctx.str_value(mode)?, "w+b" | "wb+")
    {
        return Err(Error::new(
            "NotImplementedError",
            "TemporaryFile supports binary update mode",
        ));
    }
    let directory = option(args, 1, kwargs, "dir")?.filter(|&id| id != NONE);
    let file = FileState::temporary(directory.map(|id| ctx.str_value(id)).transpose()?)?;
    let owner =
        crate::Shared::try_new(RefCell::new(file)).map_err(|_| crate::Error::allocation())?;
    Ok(ctx.alloc(Value::BufferedRandom(owner)))
}

/// Applies a boxed operation to the same buffered stream used by the native file ABI.
pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let receiver = argument(args, 0)?;
    let Value::BufferedRandom(owner) = ctx.heap.get(receiver).clone() else {
        return Err(Error::new("TypeError", "expected a buffered random file"));
    };
    let args = &args[1..];
    let arity = match name {
        "read" => 0..=1,
        "write" => 1..=1,
        "seek" => 1..=2,
        "__exit__" => 3..=3,
        _ => 0..=0,
    };
    if !arity.contains(&args.len()) || !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("unsupported arguments to BufferedRandom.{name}"),
        ));
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
        "readline" => {
            let bytes = owner.borrow_mut().read_line_bytes()?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        "write" => {
            let bytes = ctx.bytes_like(args[0])?;
            owner.borrow_mut().write_bytes(bytes)?;
            let length = crate::Int::from_usize(bytes.len());
            Ok(ctx.alloc(Value::Int(length)))
        }
        "seek" => {
            let offset = ctx.index_integer(args[0])?;
            let whence = if let Some(&value) = args.get(1) {
                ctx.index_integer(value)?
                    .to_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| {
                        Error::new("OverflowError", "Python int too large to convert to C int")
                    })?
            } else {
                0
            };
            let position = owner.borrow_mut().seek_disk(&offset, i64::from(whence))?;
            Ok(ctx.int(position as i64))
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
