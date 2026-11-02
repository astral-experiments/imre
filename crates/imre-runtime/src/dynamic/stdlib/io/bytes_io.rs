//! Boxed access to the native, seekable in-memory binary file.

use std::cell::{Cell, RefCell};
use std::io::SeekFrom;
use std::rc::Rc;

use super::super::files;
use super::{Context, Error, FALSE, Id, MemoryStream, NONE, Result, TRUE, Value};
use crate::objects::FileState;

pub(super) const METHODS: &[&str] = &[
    "__init__",
    "__iter__",
    "__next__",
    "read",
    "read1",
    "readline",
    "readlines",
    "readinto",
    "truncate",
    "write",
    "writelines",
    "seek",
    "tell",
    "getvalue",
    "isatty",
    "closed",
    "close",
    "flush",
    "readable",
    "writable",
    "seekable",
];

pub(in crate::dynamic) struct BytesIo {
    file: RefCell<FileState>,
    // The heap traces the boxed immutable snapshot until a write replaces its contents.
    pub(in crate::dynamic) snapshot: Cell<Option<Id>>,
}

pub(super) fn copy_bytes(data: &[u8]) -> Result<Vec<u8>> {
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

    fn read(&self, ctx: &mut Context, limit: Option<usize>, line: bool) -> Result<Id> {
        let data = self.file.borrow_mut().read_memory_bytes(limit, line)?;
        if let Some(bytes) = data {
            return Ok(ctx.alloc(Value::Bytes(bytes)));
        }
        let value = self.getvalue(ctx)?;
        self.file.borrow_mut().seek(SeekFrom::End(0))?;
        Ok(value)
    }

    pub(super) fn write(&self, data: &[u8]) -> Result<()> {
        self.file.borrow_mut().write_bytes(data)?;
        if !data.is_empty() {
            self.snapshot.set(None);
        }
        Ok(())
    }

    fn readlines(&self, ctx: &mut Context, hint: Option<Id>) -> Result<Id> {
        let mut file = self.file.borrow_mut();
        file.flush()?;
        let limit = match hint {
            None | Some(NONE) => 0,
            Some(id) => ctx
                .as_int(id)
                .map_err(|_| {
                    Error::new(
                        "TypeError",
                        format!("integer argument expected, got '{}'", ctx.type_name(id)),
                    )
                })?
                .to_i64()
                .ok_or_else(|| {
                    Error::new(
                        "OverflowError",
                        "Python int too large to convert to C ssize_t",
                    )
                })?,
        };
        let mut lines = Vec::new();
        let mut size = 0;
        loop {
            let line = file.read_line_bytes()?;
            if line.is_empty() {
                break;
            }
            size += line.len();
            lines
                .try_reserve(1)
                .map_err(|_| crate::Error::allocation())?;
            lines.push(ctx.alloc(Value::Bytes(line)));
            if limit > 0 && size >= limit as usize {
                break;
            }
        }
        Ok(ctx.alloc(Value::List(lines)))
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
            let data = files::output_bytes(ctx, initial)?;
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
    let owner = if matches!(name, "__enter__" | "__exit__") {
        "_IOBase"
    } else {
        "BytesIO"
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{owner}.{name}() takes no keyword arguments"),
        ));
    }
    super::argument_count(owner, name, args.len())?;
    match name {
        "read" | "read1" | "readline" => {
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
            let limit = size.and_then(|n| usize::try_from(n).ok());
            stream.read(ctx, limit, name == "readline")
        }
        "__iter__" => Ok(receiver),
        "__next__" => {
            let line = stream.read(ctx, None, true)?;
            if ctx.bytes_like(line)?.is_empty() {
                return Err(Error::new("StopIteration", ""));
            }
            Ok(line)
        }
        "truncate" => {
            // Unlike read, truncate checks the stream before it invokes the size's __index__.
            stream.file.borrow_mut().flush()?;
            let size = match args.first().copied() {
                None | Some(NONE) => None,
                Some(id) => Some(ctx.index_integer(id)?.to_i64().ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C long")
                })?),
            };
            let (size, changed) = stream.file.borrow_mut().truncate_memory(size)?;
            if changed {
                stream.snapshot.set(None);
            }
            Ok(ctx.int(size as i64))
        }
        "write" => {
            let input = files::output_bytes(ctx, args[0])?;
            stream.write(input)?;
            Ok(ctx.int(input.len() as i64))
        }
        "readlines" => stream.readlines(ctx, args.first().copied()),
        "writelines" => {
            stream.file.borrow_mut().flush()?;
            let iterator = ctx.iterator(args[0])?;
            while let Some(line) = ctx.next_item(iterator)? {
                stream.write(files::output_bytes(ctx, line)?)?;
            }
            Ok(NONE)
        }
        "readinto" => {
            let buffer = super::super::files::readinto_bytes(ctx, args[0])?;
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
