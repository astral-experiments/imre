//! Boxed access to the native, seekable in-memory binary file.

use std::cell::{Cell, RefCell};
use std::io::SeekFrom;
use std::rc::Rc;

use super::{Context, Error, FALSE, Id, MemoryStream, NONE, Result, TRUE, Value};
use crate::objects::FileState;

pub(super) const METHODS: &[&str] = &[
    "__init__", "read", "read1", "readline", "readinto", "truncate", "write", "seek", "tell",
    "getvalue", "isatty",
];

pub(in crate::dynamic) struct BytesIo {
    file: RefCell<FileState>,
    // The heap traces the boxed immutable snapshot until a write replaces its contents.
    pub(in crate::dynamic) snapshot: Cell<Option<Id>>,
}

fn copy_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(data.len())
        .map_err(|_| crate::Error::allocation())?;
    result.extend_from_slice(data);
    Ok(result)
}

impl BytesIo {
    fn empty() -> Self {
        Self {
            file: RefCell::new(FileState::bytes_io(crate::BytesOwner::new(Vec::new()))),
            snapshot: Cell::new(None),
        }
    }

    fn getvalue(&self, ctx: &mut Context) -> Result<Id> {
        let bytes = self.file.borrow_mut().memory_value()?;
        if let Some(id) = self.snapshot.get() {
            return Ok(id);
        }
        let id = ctx.alloc(Value::Bytes(copy_bytes(&bytes)?));
        self.snapshot.set(Some(id));
        Ok(id)
    }

    fn init(&self, ctx: &Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let count = args.len() + kwargs.len();
        if count > 1 {
            return Err(Error::new(
                "TypeError",
                format!("BytesIO() takes at most 1 argument ({count} given)"),
            ));
        }
        let initial = if let Some((key, id)) = kwargs.first() {
            if key != "initial_bytes" {
                return Err(Error::new(
                    "TypeError",
                    format!("BytesIO() got an unexpected keyword argument '{key}'"),
                ));
            }
            *id
        } else {
            args.first().copied().unwrap_or(NONE)
        };
        let mut file = self.file.borrow_mut();
        // CPython resets an open stream even if the new buffer is invalid.
        if !file.closed() {
            *file = Self::empty().file.into_inner();
        }
        self.snapshot.set(None);
        if initial != NONE {
            let data = ctx.bytes_like(initial)?;
            if !matches!(ctx.heap.get(initial), Value::Bytes(_)) && file.closed() {
                file.flush()?;
            }
            let data = crate::BytesOwner::try_new(copy_bytes(data)?)
                .map_err(|_| crate::Error::allocation())?;
            *file = FileState::bytes_io(data);
            if matches!(ctx.heap.get(initial), Value::Bytes(_)) {
                self.snapshot.set(Some(initial));
            }
        }
        Ok(NONE)
    }
}

fn argument_count(name: &str, n: usize) -> Result<()> {
    let message = match name {
        "read" | "read1" | "readline" | "truncate" if n > 1 => {
            format!("{name} expected at most 1 argument, got {n}")
        }
        "seek" if n == 0 => "seek expected at least 1 argument, got 0".into(),
        "seek" if n > 2 => format!("seek expected at most 2 arguments, got {n}"),
        "write" | "readinto" if n != 1 => {
            format!("BytesIO.{name}() takes exactly one argument ({n} given)")
        }
        "read" | "read1" | "readline" | "seek" | "write" | "readinto" | "truncate" | "__exit__" => {
            return Ok(());
        }
        _ if n != 0 => format!("BytesIO.{name}() takes no arguments ({n} given)"),
        _ => return Ok(()),
    };
    Err(Error::new("TypeError", message))
}

/// Coerce Python arguments before borrowing the native state, so index hooks can reenter.
pub(super) fn call(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    let MemoryStream::Bytes(stream) =
        memory_stream.get_or_insert_with(|| MemoryStream::Bytes(Rc::new(BytesIo::empty())))
    else {
        return Err(Error::new("TypeError", "incompatible memory stream layout"));
    };
    let stream = Rc::clone(stream);
    if name == "__init__" {
        return stream.init(ctx, args, kwargs);
    }
    if !kwargs.is_empty() {
        let prefix = if name == "__exit__" { "" } else { "BytesIO." };
        return Err(Error::new(
            "TypeError",
            format!("{prefix}{name}() takes no keyword arguments"),
        ));
    }
    argument_count(name, args.len())?;
    match name {
        "read" | "read1" | "readline" | "truncate" => {
            let size = match args.first().copied() {
                None | Some(NONE) => None,
                Some(id) => {
                    let n = ctx.index_value(id)?.ok_or_else(|| {
                        Error::new(
                            "TypeError",
                            format!(
                                "argument should be integer or None, not '{}'",
                                ctx.type_name(id)
                            ),
                        )
                    })?;
                    let n = n.to_i64().ok_or_else(|| {
                        Error::new(
                            "OverflowError",
                            "cannot fit 'int' into an index-sized integer",
                        )
                    })?;
                    Some(n)
                }
            };
            if name == "truncate" {
                let (size, changed) = stream.file.borrow_mut().truncate_memory(size)?;
                if changed {
                    stream.snapshot.set(None);
                }
                return Ok(ctx.int(size as i64));
            }
            let limit = size.and_then(|n| usize::try_from(n).ok());
            let bytes = stream
                .file
                .borrow_mut()
                .read_memory_bytes(limit, name == "readline")?;
            match bytes {
                Some(bytes) => Ok(ctx.alloc(Value::Bytes(bytes))),
                None => {
                    let value = stream.getvalue(ctx)?;
                    stream.file.borrow_mut().seek(SeekFrom::End(0))?;
                    Ok(value)
                }
            }
        }
        "write" => {
            let input = ctx.bytes_like(args[0])?;
            stream.file.borrow_mut().write_bytes(input)?;
            if !input.is_empty() {
                stream.snapshot.set(None);
            }
            Ok(ctx.int(input.len() as i64))
        }
        "readinto" => {
            let id = args[0];
            let Value::ByteArray(buffer) = ctx.heap.get_mut(id) else {
                let kind = if id == NONE {
                    "None"
                } else {
                    ctx.type_name(id)
                };
                return Err(Error::new(
                    "TypeError",
                    format!("{name}() argument must be read-write bytes-like object, not {kind}"),
                ));
            };
            let count = stream.file.borrow_mut().read_into(buffer)?;
            Ok(ctx.int(count as i64))
        }
        "seek" => {
            let offset = ctx.index_integer(args[0])?.to_i64().ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?;
            let whence = if let Some(&id) = args.get(1) {
                ctx.index_integer(id)?
                    .to_i64()
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or_else(|| {
                        Error::new("OverflowError", "Python int too large to convert to C int")
                    })?
            } else {
                0
            };
            let pos = stream.file.borrow_mut().seek_memory(offset, whence)?;
            Ok(ctx.int(pos as i64))
        }
        "tell" => Ok(ctx.int(stream.file.borrow_mut().seek(SeekFrom::Current(0))? as i64)),
        "getvalue" => stream.getvalue(ctx),
        "closed" => Ok(if stream.file.borrow().closed() {
            TRUE
        } else {
            FALSE
        }),
        "close" => {
            stream
                .file
                .borrow_mut()
                .close()
                .map_err(crate::objects::errno_error)?;
            stream.snapshot.set(None);
            Ok(NONE)
        }
        "__exit__" => {
            let close = ctx.lookup_attr(receiver, "close")?;
            ctx.call(close, Vec::new(), Vec::new())
        }
        "flush" | "__enter__" | "isatty" | "readable" | "writable" | "seekable" => {
            stream.file.borrow_mut().flush()?;
            Ok(match name {
                "flush" => NONE,
                "__enter__" => receiver,
                "isatty" => FALSE,
                _ => TRUE,
            })
        }
        _ => unreachable!("registered BytesIO method"),
    }
}
