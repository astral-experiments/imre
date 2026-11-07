//! Boxed access to the native, seekable in-memory binary file.

use std::cell::Cell;
use std::io::SeekFrom;

use super::{Context, Error, FALSE, Id, MemoryStream, NONE, Result, TRUE, Value};
use crate::objects::Memory;

pub(super) const METHODS: &[&str] = &[
    "__new__",
    "__init__",
    "__getstate__",
    "__setstate__",
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
    "getbuffer",
    "isatty",
    "closed",
    "close",
    "flush",
    "readable",
    "writable",
    "seekable",
];

/// A callback may retain this heap slot ID while the evaluator temporarily yields its borrow.
#[derive(Clone, Copy)]
pub(in crate::dynamic) struct BytesIo {
    pub(in crate::dynamic) storage: Id,
}

/// Cursor, backing bytes and cached Python value belong to the exporting heap slot.
#[derive(Clone)]
pub(in crate::dynamic) struct BytesIoStorage {
    pub(in crate::dynamic) file: Option<Memory>,
    deferred: bool,
    closed_position: u64,
    // Resolving a writable export through a shared heap borrow invalidates the cached bytes.
    pub(in crate::dynamic) snapshot: Cell<Option<Id>>,
}

fn closed() -> Error {
    Error::new("ValueError", "I/O operation on closed file.")
}

impl BytesIoStorage {
    pub(in crate::dynamic) fn file(&self) -> Result<&Memory> {
        self.file.as_ref().ok_or_else(closed)
    }

    pub(in crate::dynamic) fn file_mut(&mut self) -> Result<&mut Memory> {
        self.file.as_mut().ok_or_else(closed)
    }
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
    fn new(ctx: &mut Context) -> Self {
        let data = crate::BytesOwner::new(Vec::new());
        let storage = BytesIoStorage {
            file: Some(Memory::new(data)),
            deferred: false,
            closed_position: 0,
            snapshot: Cell::new(None),
        };
        Self {
            storage: ctx.alloc(Value::BytesIoStorage(Box::new(storage))),
        }
    }

    fn state(self, ctx: &Context) -> &BytesIoStorage {
        let Value::BytesIoStorage(state) = ctx.heap.get(self.storage) else {
            unreachable!("BytesIO owns its storage slot")
        };
        state
    }

    fn state_mut(self, ctx: &mut Context) -> &mut BytesIoStorage {
        let Value::BytesIoStorage(state) = ctx.heap.get_mut(self.storage) else {
            unreachable!("BytesIO owns its storage slot")
        };
        state
    }

    fn file(self, ctx: &Context) -> Result<&Memory> {
        self.state(ctx).file()
    }

    fn file_mut(self, ctx: &mut Context) -> Result<&mut Memory> {
        self.state_mut(ctx).file_mut()
    }

    fn initial(self, ctx: &Context) -> Option<Id> {
        let state = self.state(ctx);
        state
            .deferred
            .then(|| state.snapshot.get().expect("deferred BytesIO input"))
    }

    /// Install private, writable storage without changing the stream position or any existing bytes.
    fn detach(&self, ctx: &mut Context, initial: Id, size: usize) -> Result<()> {
        let position = self.file_mut(ctx)?.seek(SeekFrom::Current(0))?;
        let bytes = copy_bytes(&ctx.bytes_like(initial)?[..size])?;
        let bytes = crate::BytesOwner::try_new(bytes).map_err(|_| crate::Error::allocation())?;
        let mut file = Memory::new(bytes);
        file.seek(SeekFrom::Start(position))?;
        let state = self.state_mut(ctx);
        state.file = Some(file);
        state.deferred = false;
        Ok(())
    }

    fn make_writable(&self, ctx: &mut Context) -> Result<()> {
        if let Some(initial) = self.initial(ctx) {
            let size = ctx.bytes_like(initial)?.len();
            self.detach(ctx, initial, size)?;
        }
        Ok(())
    }

    fn getvalue(&self, ctx: &mut Context) -> Result<Id> {
        self.file(ctx)?;
        let exported = ctx.buffer_is_exported(self.storage)?;
        if !exported && let Some(id) = self.state(ctx).snapshot.get() {
            return Ok(id);
        }
        let data = copy_bytes(self.file(ctx)?.bytes())?;
        let id = ctx.alloc(Value::Bytes(data));
        if !exported {
            self.state(ctx).snapshot.set(Some(id));
        }
        Ok(id)
    }

    fn read(&self, ctx: &mut Context, limit: Option<usize>, line: bool) -> Result<Id> {
        if let Some(initial) = self.initial(ctx) {
            let pos = self.file_mut(ctx)?.seek(SeekFrom::Current(0))? as usize;
            let data = ctx.bytes_like(initial)?;
            let remaining = data.get(pos..).unwrap_or_default();
            let end = limit.unwrap_or(usize::MAX).min(remaining.len());
            let end = if line {
                memchr::memchr(b'\n', &remaining[..end]).map_or(end, |i| i + 1)
            } else {
                end
            };
            let value = if pos == 0 && end == data.len() {
                initial
            } else {
                ctx.alloc(Value::Bytes(copy_bytes(&remaining[..end])?))
            };
            self.file_mut(ctx)?.seek(SeekFrom::Current(end as i64))?;
            return Ok(value);
        }
        if let Some(data) = self.file_mut(ctx)?.read_memory_bytes(limit, line)? {
            return Ok(ctx.alloc(Value::Bytes(data)));
        }
        let value = self.getvalue(ctx)?;
        self.file_mut(ctx)?.seek(SeekFrom::End(0))?;
        Ok(value)
    }

    pub(super) fn write(&self, ctx: &mut Context, data: &[u8]) -> Result<()> {
        self.file(ctx)?;
        ctx.check_bytearray_resize(self.storage)?;
        if !data.is_empty() {
            self.make_writable(ctx)?;
            self.file_mut(ctx)?.write(data)?;
            self.state(ctx).snapshot.set(None);
        }
        Ok(())
    }

    fn write_input(&self, ctx: &mut Context, input: Id) -> Result<usize> {
        let Some(method) = ctx.python_buffer_method(input, true)? else {
            ctx.native_buffer_bytes(input)?;
            return self.write_buffer(ctx, input);
        };
        // PyBUF_CONTIG_RO requests shape without strides. Acquisition may change this stream.
        ctx.with_contiguous_python_buffer(input, method, 0x0008, |ctx, view| {
            self.write_buffer(ctx, view)
        })
    }

    fn write_buffer(&self, ctx: &mut Context, input: Id) -> Result<usize> {
        self.file(ctx)?;
        ctx.with_buffer_export(input, |ctx| {
            ctx.check_bytearray_resize(self.storage)?;
            if ctx.native_buffer_bytes(input)?.is_empty() {
                return Ok(0);
            }
            self.make_writable(ctx)?;
            // Direct byte inputs have distinct slots from this stream. Other inputs
            // can alias this storage through a raw callback window.
            let size = if let Some((
                Value::Bytes(data) | Value::ByteArray(data),
                Value::BytesIoStorage(state),
            )) = ctx
                .heap
                .source_and_target(ctx.scalar_value(input), self.storage)
            {
                state.file_mut()?.write(data)?;
                data.len()
            } else {
                let bytes = copy_bytes(ctx.native_buffer_bytes(input)?)?;
                self.file_mut(ctx)?.write(&bytes)?;
                bytes.len()
            };
            if size != 0 {
                self.state(ctx).snapshot.set(None);
            }
            Ok(size)
        })
    }

    fn readlines(&self, ctx: &mut Context, hint: Option<Id>) -> Result<Id> {
        self.file(ctx)?;
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
            let id = self.read(ctx, None, true)?;
            let line = ctx.bytes_like(id)?;
            if line.is_empty() {
                break;
            }
            size += line.len();
            lines
                .try_reserve(1)
                .map_err(|_| crate::Error::allocation())?;
            lines.push(id);
            if limit > 0 && size >= limit as usize {
                break;
            }
        }
        Ok(ctx.alloc(Value::List(lines)))
    }

    fn setstate(&self, ctx: &mut Context, receiver: Id, state: Id) -> Result<Id> {
        let items = ctx.tuple_items(state)?;
        let Some(&[value, position, dict, ..]) = items.as_deref() else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{}.__setstate__ argument should be 3-tuple, got {}",
                    ctx.type_name(receiver),
                    ctx.type_name(state),
                ),
            ));
        };
        ctx.check_bytearray_resize(self.storage)?;
        self.reset(ctx);
        self.write_input(ctx, value)?;
        let position = ctx
            .as_int(position)
            .map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "second item of state must be an integer, not {}",
                        ctx.type_name(position)
                    ),
                )
            })?
            .to_i64()
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?;
        let position = u64::try_from(position)
            .map_err(|_| Error::new("ValueError", "position value cannot be negative"))?;
        self.set_position_after_write(ctx, position)?;
        super::restore_stream_dict(ctx, receiver, dict, "third")
    }

    fn set_position_after_write(&self, ctx: &mut Context, position: u64) -> Result<()> {
        // A Python exporter can close the stream when releasing its view after a successful write.
        if let Ok(file) = self.file_mut(ctx) {
            file.seek(SeekFrom::Start(position))?;
        } else {
            self.state_mut(ctx).closed_position = position;
        }
        Ok(())
    }

    fn reset(&self, ctx: &mut Context) {
        let state = self.state_mut(ctx);
        if let Some(file) = &mut state.file {
            file.reset_position_and_size();
        } else {
            state.closed_position = 0;
        }
        state.deferred = false;
        state.snapshot.set(None);
    }

    fn init(
        &self,
        ctx: &mut Context,
        args: &[Id],
        kwargs: &[(String, Id)],
        fresh: bool,
    ) -> Result<Id> {
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
        self.reset(ctx);
        ctx.check_bytearray_resize(self.storage)?;
        if initial != NONE {
            if matches!(ctx.heap.get(initial), Value::Bytes(_)) {
                if fresh {
                    let state = self.state_mut(ctx);
                    state.snapshot.set(Some(initial));
                    state.deferred = true;
                    return Ok(NONE);
                }
                let bytes = crate::BytesOwner::try_new(copy_bytes(ctx.bytes_like(initial)?)?)
                    .map_err(|_| crate::Error::allocation())?;
                let state = self.state_mut(ctx);
                state.file = Some(Memory::new(bytes));
                state.snapshot.set(Some(initial));
            } else {
                self.write_input(ctx, initial)?;
                self.set_position_after_write(ctx, 0)?;
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
    let (stream, fresh) = match ctx.heap.get(receiver) {
        Value::Instance {
            memory_stream: Some(MemoryStream::Bytes(stream)),
            ..
        } => (*stream, false),
        Value::Instance {
            memory_stream: None,
            ..
        } => {
            let stream = BytesIo::new(ctx);
            let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
                unreachable!()
            };
            *memory_stream = Some(MemoryStream::Bytes(stream));
            (stream, true)
        }
        _ => return Err(Error::new("TypeError", "incompatible memory stream layout")),
    };
    if name == "__init__" {
        return stream.init(ctx, args, kwargs, fresh);
    }
    if name == "getbuffer" && !(args.is_empty() && kwargs.is_empty()) {
        return Err(Error::new("TypeError", "getbuffer() takes no arguments"));
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
            stream.file(ctx)?;
            ctx.check_bytearray_resize(stream.storage)?;
            let size = match args.first().copied() {
                None | Some(NONE) => None,
                Some(id) => Some(ctx.index_integer(id)?.to_i64().ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C long")
                })?),
            };
            // A size hook may have acquired a new export after the initial guard.
            let retain_storage = ctx.buffer_is_exported(stream.storage)?;
            if let Some(initial) = stream.initial(ctx) {
                let requested = match size {
                    None => stream.file_mut(ctx)?.seek(SeekFrom::Current(0))?,
                    Some(size) => u64::try_from(size).map_err(|_| {
                        Error::new("ValueError", format!("negative size value {size}"))
                    })?,
                };
                if requested < ctx.bytes_like(initial)?.len() as u64 {
                    stream.detach(ctx, initial, requested as usize)?;
                    stream.state(ctx).snapshot.set(None);
                }
                return Ok(ctx.int(requested as i64));
            }
            let (size, changed) = stream
                .file_mut(ctx)?
                .truncate_memory(size, retain_storage)?;
            if changed {
                stream.state(ctx).snapshot.set(None);
            }
            Ok(ctx.int(size as i64))
        }
        "write" => {
            let n = stream.write_input(ctx, args[0])?;
            Ok(ctx.int(n as i64))
        }
        "readlines" => stream.readlines(ctx, args.first().copied()),
        "writelines" => {
            stream.file(ctx)?;
            let iterator = ctx.iterator(args[0])?;
            let mark = ctx.heap.temporary_mark();
            while let Some(line) = ctx.next_item(iterator)? {
                stream.write_input(ctx, line)?;
                // Drop this line and its buffer before asking the iterator for the next one.
                ctx.heap.release_temporaries_since(mark);
            }
            Ok(NONE)
        }
        "readinto" => {
            let len = super::super::files::readinto_bytes(ctx, args[0])?.len();
            if let Some(initial) = stream.initial(ctx) {
                let pos = stream.file_mut(ctx)?.seek(SeekFrom::Current(0))? as usize;
                let (target, range) = ctx
                    .writable_byte_span(args[0])?
                    .expect("validated destination");
                let Some((Value::Bytes(data), target)) =
                    ctx.heap.source_and_target(initial, target)
                else {
                    unreachable!("immutable source and writable destination are distinct")
                };
                let src = data.get(pos..).unwrap_or_default();
                let count = src.len().min(len);
                Context::mutable_storage_bytes(target)?[range.start..range.start + count]
                    .copy_from_slice(&src[..count]);
                stream
                    .file_mut(ctx)?
                    .seek(SeekFrom::Current(count as i64))?;
                return Ok(ctx.int(count as i64));
            }
            let data = stream.file_mut(ctx)?.read(Some(len))?;
            let count = data.len();
            super::super::files::readinto_bytes(ctx, args[0])?[..count].copy_from_slice(&data);
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
            let pos = if whence == 2
                && let Some(initial) = stream.initial(ctx)
            {
                let end = (ctx.bytes_like(initial)?.len() as u64).saturating_add_signed(offset);
                stream.file_mut(ctx)?.seek(SeekFrom::Start(end))?
            } else {
                stream.file_mut(ctx)?.seek_memory(offset, whence)?
            };
            Ok(ctx.int(pos as i64))
        }
        "tell" => Ok(ctx.int(stream.file(ctx)?.position() as i64)),
        "__getstate__" => {
            let value = stream.getvalue(ctx)?;
            ctx.heap.keep(value);
            let dict = super::copy_stream_dict(ctx, receiver)?;
            let state = stream.state(ctx);
            let pos = state
                .file
                .as_ref()
                .map_or(state.closed_position, Memory::position);
            let pos = ctx.int(pos as i64);
            Ok(ctx.alloc(Value::Tuple(vec![value, pos, dict])))
        }
        "__setstate__" => stream.setstate(ctx, receiver, args[0]),
        "getvalue" => stream.getvalue(ctx),
        "getbuffer" => {
            stream.file(ctx)?;
            stream.make_writable(ctx)?;
            let owner = ctx.alloc(Value::BytesIoBuffer {
                stream: receiver,
                storage: stream.storage,
            });
            ctx.new_memoryview(owner)
        }
        "closed" => Ok(if stream.file(ctx).is_err() {
            TRUE
        } else {
            FALSE
        }),
        "close" => {
            ctx.check_bytearray_resize(stream.storage)?;
            let state = stream.state_mut(ctx);
            if let Some(file) = state.file.take() {
                state.closed_position = file.position();
            }
            state.deferred = false;
            state.snapshot.set(None);
            Ok(NONE)
        }
        "__exit__" => {
            let close = ctx.lookup_attr(receiver, "close")?;
            ctx.call(close, Vec::new(), Vec::new())
        }
        "flush" | "__enter__" | "isatty" | "readable" | "writable" | "seekable" => {
            stream.file(ctx)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_storage_can_lend_its_cursor_to_a_worker() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let io = ctx.module("_io").unwrap();
        let class = ctx.lookup_attr(io, "BytesIO").unwrap();
        let bytes = ctx.alloc(Value::Bytes(b"abcdef".to_vec()));
        let stream = ctx.call(class, vec![bytes], Vec::new()).unwrap();
        let getbuffer = ctx.lookup_attr(stream, "getbuffer").unwrap();
        let view = ctx.call(getbuffer, Vec::new(), Vec::new()).unwrap();
        let Value::MemoryView {
            owner: Some(exporter),
            ..
        } = *ctx.heap.get(view)
        else {
            panic!("expected a view of the stream");
        };
        let Value::BytesIoBuffer {
            stream: original,
            storage,
        } = *ctx.heap.get(exporter)
        else {
            panic!("expected the original exporter");
        };
        assert_eq!(original, stream);
        let Value::BytesIoStorage(state) = ctx.heap.get_mut(storage) else {
            panic!("expected the stream's storage");
        };
        let address = state.file().unwrap().bytes().as_ptr().addr();
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    let file = state.file_mut().unwrap();
                    assert_eq!(file.bytes().as_ptr().addr(), address);
                    assert_eq!(file.seek(SeekFrom::Start(2)).ok(), Some(2));
                    assert_eq!(file.read(Some(1)).ok().as_deref(), Some(b"c".as_slice()));
                })
                .join()
                .unwrap();
        });
        assert_eq!(ctx.owner_bytes(exporter).unwrap().as_ptr().addr(), address);
        assert!(ctx.buffer_is_exported(storage).unwrap());
        let read = ctx.lookup_attr(stream, "read").unwrap();
        let suffix = ctx.call(read, Vec::new(), Vec::new()).unwrap();
        assert_eq!(ctx.bytes_like(suffix).unwrap(), b"def");
    }
}
