//! Buffer a selected FileIO descriptor without changing its identity or ownership.
use std::cell::{Cell, RefCell};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::fd::RawFd;
use std::rc::Rc;

use super::super::{builtins, files, os_error};
use super::file_io::{self, RawFile};
use super::{Context, Error, Id, MemoryStream, NONE, Result, TRUE, Value};
use crate::objects::{BoxedPipe, Random};
use crate::{BinaryOutput, BlockedWrite};

pub(super) fn backing(ctx: &Context, receiver: Id) -> Option<Id> {
    match ctx.heap.get(receiver) {
        Value::File(_) => Some(receiver),
        Value::Instance {
            memory_stream: Some(MemoryStream::BufferedFile(buffer)),
            ..
        } => Some(*buffer),
        _ => None,
    }
}

/// Only a writer initialized with a caller's FileIO delegates finalization to its wrapper.
fn writer_fallback(ctx: &Context, owner: Id) -> Option<(Id, &BufferedFd, &Cell<bool>)> {
    let Value::Instance {
        class,
        memory_stream: Some(MemoryStream::BufferedFile(buffer)),
        ..
    } = ctx.heap.get(owner)
    else {
        return None;
    };
    let Value::File(file) = ctx.heap.get(*buffer) else {
        return None;
    };
    let files::BinaryFile::Descriptor(fd) = file.as_ref() else {
        return None;
    };
    Some((*class, fd, fd.drop_fallback.as_ref()?))
}

/// A Python finalizer can decline close, raise, or resurrect the writer.
pub(in crate::dynamic) fn note_writer_finalizer(ctx: &Context, owner: Id) {
    if let Some((_, _, armed)) = writer_fallback(ctx, owner) {
        armed.set(false);
    }
}

/// A live module global can reach Rust Drop without running its Python finalizer.
/// Permit the native flush only while it still implements the writer's close.
pub(in crate::dynamic) fn prepare_writer_shutdown(ctx: &Context) {
    for &owner in ctx.heap.pending_finalizers() {
        let Some((class, fd, armed)) = writer_fallback(ctx, owner) else {
            continue;
        };
        if !armed.get() || fd.closed() {
            continue;
        }
        let base = ctx.find_in_mro(class, |candidate| {
            (ctx.heap.get(candidate).io_kind() == Some("BufferedWriter")).then_some(candidate)
        });
        let native = base.is_some_and(|base| {
            ["__del__", "__getattribute__", "closed", "close", "flush"]
                .iter()
                .all(|name| ctx.class_attr(class, name) == ctx.class_attr(base, name))
        }) && ["close", "flush"]
            .iter()
            .all(|name| ctx.raw_instance_field(owner, name).is_none())
            && fd.check_raw(ctx, owner, "close").is_ok()
            && fd.check_raw(ctx, owner, "flush").is_ok();
        armed.set(native);
    }
}

fn native_raw(ctx: &Context, raw: Id, methods: &[&str], owner: &str) -> Result<Rc<RawFile>> {
    // Going straight to the descriptor is valid only while the Python raw methods
    // are native. The instance dictionary can change after the buffer is created.
    if ctx
        .heap
        .get(raw)
        .instance_class()
        .and_then(|class| ctx.heap.get(class).io_kind())
        == Some("FileIO")
        && !methods
            .iter()
            .any(|method| ctx.raw_instance_field(raw, method).is_some())
        && let Some(handle) = file_io::state(ctx, raw)
    {
        return Ok(handle);
    }
    Err(Error::new(
        "NotImplementedError",
        format!("{owner} with Python raw methods is unsupported"),
    ))
}

pub(super) fn ssize(ctx: &mut Context, value: Id) -> Result<i64> {
    ctx.index_integer(value)?.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

/// Attach one buffered stream to a caller-owned native FileIO, without reopening its fd.
pub(super) fn initialize(
    ctx: &mut Context,
    receiver: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
    reading: bool,
) -> Result<Id> {
    let (owner, capability, methods): (_, _, &[&str]) = if reading {
        (
            "BufferedReader",
            "readable",
            &[
                "close", "flush", "readable", "seekable", "isatty", "fileno", "tell", "seek",
                "read", "readall", "readinto",
            ],
        )
    } else {
        (
            "BufferedWriter",
            "writable",
            &[
                "close", "flush", "writable", "seekable", "isatty", "fileno", "tell", "seek",
                "write", "truncate",
            ],
        )
    };
    let [raw, capacity] =
        builtins::bind_builtin_arguments(owner, args, kwargs, ["raw", "buffer_size"], 1)?;
    let raw = raw.expect("required argument");
    let capacity = capacity
        .map(|n| ssize(ctx, n))
        .transpose()?
        .unwrap_or(super::DEFAULT_BUFFER_SIZE);
    let method = ctx.lookup_attr(raw, capability)?;
    if ctx.call(method, vec![], Vec::new())? != TRUE {
        return Err(Error::new(
            "UnsupportedOperation",
            format!("File or stream is not {capability}."),
        ));
    }
    if capacity <= 0 {
        return Err(Error::new(
            "ValueError",
            "buffer size must be strictly positive",
        ));
    }
    let handle = native_raw(ctx, raw, methods, owner)?;
    if backing(ctx, receiver).is_some() {
        return Err(Error::new(
            "NotImplementedError",
            format!("{owner} reinitialization is unsupported"),
        ));
    }
    // Both standard Rust buffer constructors allocate infallibly.
    if capacity as u64 > crate::text_output::binary_capacity(i128::MAX) as u64 {
        return Err(Error::new(
            "NotImplementedError",
            format!("explicit {owner} buffers above 8 MiB are unsupported"),
        ));
    }
    let mut fd = BufferedFd::new(raw, handle, reading, capacity as usize);
    if !reading {
        fd.drop_fallback = Some(Cell::new(true));
    }
    // Do not close the caller's file if allocating the backing value fails.
    let pending = fd.buffer.take();
    let value = files::BinaryFile::Descriptor(fd).into_value()?;
    let Value::File(file) = &value else {
        unreachable!()
    };
    let files::BinaryFile::Descriptor(fd) = file.as_ref() else {
        unreachable!()
    };
    fd.buffer.replace(pending);
    let buffer = ctx.alloc(value);
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!("uninitialized buffered instance")
    };
    *memory_stream = Some(MemoryStream::BufferedFile(buffer));
    Ok(NONE)
}

struct Descriptor {
    handle: Rc<RawFile>,
    raw_position: Option<u64>,
    last_read: usize,
}

impl Descriptor {
    fn fd(&self) -> io::Result<RawFd> {
        self.handle
            .live_fd()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))
    }
}

fn count(n: isize) -> io::Result<usize> {
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

impl Read for Descriptor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: the buffer is writable for its length. The kernel validates the fd.
        let n = count(unsafe { libc::read(self.fd()?, buf.as_mut_ptr().cast(), buf.len()) })?;
        self.last_read = n;
        self.raw_position = self.raw_position.and_then(|p| p.checked_add(n as u64));
        Ok(n)
    }
}

impl Write for Descriptor {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // SAFETY: the buffer is readable for its length. The kernel validates the fd.
        count(unsafe { libc::write(self.fd()?, buf.as_ptr().cast(), buf.len()) })
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Descriptor {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(n) => (
                i64::try_from(n).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?,
                libc::SEEK_SET,
            ),
            SeekFrom::Current(n) => (n, libc::SEEK_CUR),
            SeekFrom::End(n) => (n, libc::SEEK_END),
        };
        // SAFETY: lseek takes only integer arguments.
        let n = unsafe { libc::lseek(self.fd()?, offset, whence) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            self.raw_position = Some(n as u64);
            Ok(n as u64)
        }
    }
}

/// Return buffered bytes first, or perform one raw read without reading ahead.
pub(super) fn read_once(reader: &mut BufReader<impl Read>, size: i64) -> Result<Vec<u8>> {
    let size = usize::try_from(size).unwrap_or(reader.capacity());
    let buffered = reader.buffer();
    if !buffered.is_empty() {
        let count = size.min(buffered.len());
        let mut bytes = builtins::zeroed_bytes(count as i64)?;
        bytes.copy_from_slice(&buffered[..count]);
        reader.consume(count);
        return Ok(bytes);
    }
    let mut bytes = builtins::zeroed_bytes(size as i64)?;
    if !bytes.is_empty() {
        loop {
            match reader.get_mut().read(&mut bytes) {
                Ok(count) => bytes.truncate(count),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => bytes.clear(),
                Err(error) => return Err(os_error(error)),
            }
            break;
        }
    }
    Ok(bytes)
}

/// RawFile.tell() exposes read-ahead. Read whole blocks directly and fill one
/// complete buffer for a final partial block, as CPython's BufferedReader does.
fn read_bytes(reader: &mut BufReader<Descriptor>, size: i64) -> Result<Option<Vec<u8>>> {
    let mut data = Vec::new();
    let result = (|| -> io::Result<()> {
        if size < 0 {
            reader.read_to_end(&mut data)?;
            return Ok(());
        }
        while data.len() < size as usize {
            let remaining = size as usize - data.len();
            if reader.buffer().is_empty() && remaining >= reader.capacity() {
                let direct = remaining - remaining % reader.capacity();
                if reader
                    .get_mut()
                    .take(direct as u64)
                    .read_to_end(&mut data)?
                    < direct
                {
                    break;
                }
            } else {
                let buffered = match reader.fill_buf() {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    result => result?,
                };
                let count = remaining.min(buffered.len());
                if count == 0 {
                    break;
                }
                data.try_reserve(count)
                    .map_err(|_| io::ErrorKind::OutOfMemory)?;
                data.extend_from_slice(&buffered[..count]);
                reader.consume(count);
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(Some(data)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            Ok((!data.is_empty()).then_some(data))
        }
        Err(error) if error.kind() == io::ErrorKind::OutOfMemory => {
            Err(Error::new("MemoryError", ""))
        }
        Err(error) => Err(os_error(error)),
    }
}

fn buffer_error(ctx: &mut Context, error: io::Error) -> Error {
    let Some(blocked) = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<BlockedWrite>())
    else {
        return os_error(error);
    };
    let errno = ctx.int(i64::from(blocked.errno));
    let message = ctx.text(BlockedWrite::DESCRIPTION);
    let accepted = ctx.int(blocked.accepted as i64);
    ctx.os_exception("BlockingIOError", None, &[errno, message, accepted], None)
        .unwrap_or_else(|error| error)
}

enum Buffer {
    Reader(BufReader<Descriptor>),
    Writer(BinaryOutput<Descriptor>),
    Random(Random<Descriptor>),
    // The process engine allocated this buffer before launching the child.
    Pipe(BoxedPipe),
}

impl Buffer {
    fn position(&mut self) -> io::Result<u64> {
        // The caller can reposition the descriptor through another handle.
        match self {
            Self::Random(file) => file.tell(),
            Self::Pipe(_) => Err(io::Error::from_raw_os_error(libc::ESPIPE)),
            Self::Reader(r) => Ok(r
                .get_mut()
                .stream_position()?
                .saturating_sub(r.buffer().len() as u64)),
            Self::Writer(w) => Ok(w.inner.stream_position()? + (w.buffer.len() - w.written) as u64),
        }
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let reader = match self {
            Self::Random(file) => return file.seek(position),
            Self::Pipe(_) => return Err(io::Error::from_raw_os_error(libc::ESPIPE)),
            Self::Reader(r) => r,
            Self::Writer(w) => {
                w.flush()?;
                return w.inner.seek(position);
            }
        };
        let unread = reader.buffer().len() as i128;
        if unread > 0 && !matches!(position, SeekFrom::End(_)) {
            // An external lseek moves the descriptor but not buffered bytes.
            // CPython uses the cached raw position here; tell() refreshes it.
            let raw = reader.get_mut();
            let end = match raw.raw_position {
                Some(end) => end,
                None => raw.stream_position()?,
            };
            let logical = i128::from(end) - unread;
            let offset = match position {
                SeekFrom::Start(n) => i128::from(n) - logical,
                SeekFrom::Current(n) => i128::from(n),
                SeekFrom::End(_) => unreachable!(),
            };
            let consumed = raw.last_read as i128 - unread;
            if (-consumed..=unread).contains(&offset) {
                reader.seek_relative(offset as i64)?;
                return Ok((logical + offset).max(0) as u64);
            }
        }
        reader.seek(position)
    }
}

pub(in crate::dynamic) struct BufferedFd {
    pub(in crate::dynamic) raw: Id,
    pub(in crate::dynamic) reading: bool,
    pub(in crate::dynamic) writing: bool,
    drop_fallback: Option<Cell<bool>>,
    handle: Rc<RawFile>,
    buffer: RefCell<Option<Buffer>>,
}

impl BufferedFd {
    pub(super) fn new(raw: Id, handle: Rc<RawFile>, reading: bool, capacity: usize) -> Self {
        let mut descriptor = Descriptor {
            handle: handle.clone(),
            raw_position: None,
            last_read: 0,
        };
        let buffer = if reading {
            let _ = descriptor.stream_position();
            Buffer::Reader(BufReader::with_capacity(capacity, descriptor))
        } else {
            Buffer::Writer(BinaryOutput::with_buffer(
                Vec::with_capacity(capacity),
                descriptor,
            ))
        };
        Self {
            raw,
            reading,
            writing: !reading,
            handle,
            drop_fallback: None,
            buffer: RefCell::new(Some(buffer)),
        }
    }

    pub(super) fn update(raw: Id, handle: Rc<RawFile>, capacity: usize) -> Result<Self> {
        let mut descriptor = Descriptor {
            handle: handle.clone(),
            raw_position: None,
            last_read: 0,
        };
        descriptor
            .stream_position()
            .map_err(|_| Error::new("UnsupportedOperation", "File or stream is not seekable."))?;
        let buffer = Random::new(descriptor, capacity)?;
        Ok(Self {
            raw,
            handle,
            reading: true,
            writing: true,
            drop_fallback: None,
            buffer: RefCell::new(Some(Buffer::Random(buffer))),
        })
    }

    pub(super) fn pipe(raw: Id, handle: Rc<RawFile>, reading: bool, pipe: BoxedPipe) -> Self {
        Self {
            raw,
            handle,
            reading,
            writing: !reading,
            drop_fallback: None,
            buffer: RefCell::new(Some(Buffer::Pipe(pipe))),
        }
    }

    /// Check the live raw after any argument conversion which can invoke Python.
    fn check_raw(&self, ctx: &Context, receiver: Id, name: &str) -> Result<()> {
        if matches!(ctx.heap.get(receiver), Value::File(_)) {
            return Ok(());
        }
        let (owner, methods): (_, &[&str]) = if self.reading {
            (
                "BufferedReader",
                match name {
                    "read" => &["read", "readall", "readinto"],
                    "read1" | "readline" | "readinto" => &["readinto"],
                    "seek" => &["seek", "seekable", "tell"],
                    "close" => &["flush", "close"],
                    _ => std::slice::from_ref(&name),
                },
            )
        } else {
            (
                "BufferedWriter",
                match name {
                    "flush" => &["write"],
                    "seek" => &["write", "seek", "seekable", "tell"],
                    "close" => &["close", "flush"],
                    _ => std::slice::from_ref(&name),
                },
            )
        };
        let current = native_raw(ctx, self.raw, methods, owner)?;
        if !Rc::ptr_eq(&current, &self.handle) {
            return Err(Error::new(
                "NotImplementedError",
                format!("reinitialized {owner} raw FileIO is unsupported"),
            ));
        }
        Ok(())
    }

    pub(in crate::dynamic) fn closed(&self) -> bool {
        self.handle.live_fd().is_none()
    }

    fn close(&self, ctx: Option<&mut Context>, flush: bool) -> Result<()> {
        let pending = match self.buffer.borrow_mut().take() {
            Some(Buffer::Pipe(pipe)) => pipe.close().map_err(Into::into),
            Some(Buffer::Random(mut file)) if flush && !self.closed() => {
                file.flush().map_err(os_error)
            }
            Some(Buffer::Writer(mut writer)) if flush && !self.closed() => {
                writer.flush().map_err(|error| {
                    if let Some(ctx) = ctx {
                        buffer_error(ctx, error)
                    } else {
                        os_error(error)
                    }
                })
            }
            _ => Ok(()),
        };
        self.handle.close().and(pending)
    }

    /// A BufferedWriter subclass has already had its one opportunity to flush.
    pub(super) fn close_after_flush(&self, ctx: &Context, receiver: Id) -> Result<()> {
        self.check_raw(ctx, receiver, "close")?;
        self.close(None, false)
    }

    pub(in crate::dynamic) fn call(
        &self,
        ctx: &mut Context,
        receiver: Id,
        name: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let name = name.strip_prefix("file.").unwrap_or(name);
        if matches!((name, args), ("close", []) | ("__exit__", [_, _, _])) {
            self.check_raw(ctx, receiver, name)?;
            self.close(Some(ctx), true)?;
            return Ok(NONE);
        }
        let closed = || {
            Error::new(
                "ValueError",
                match name {
                    "read" | "read1" => "read of closed file",
                    "readline" => "readline of closed file",
                    "readinto" => "readinto of closed file",
                    "seek" => "seek of closed file",
                    "write" => "write to closed file",
                    "flush" if !self.reading => "flush of closed file",
                    "flush" | "__enter__" => "I/O operation on closed file.",
                    _ => "I/O operation on closed file",
                },
            )
        };
        let size = if let ("read1", [n]) = (name, args) {
            Some(ctx.index_integer(*n)?.to_i64().ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?)
        } else if matches!(name, "read" | "readline") {
            Some(files::read_length(ctx, args.first().copied())?)
        } else {
            None
        };
        if name == "seek" && !self.reading {
            let target = *args
                .first()
                .ok_or_else(|| Error::new("TypeError", "seek requires an offset"))?;
            let whence =
                files::buffered_whence(ctx, super::super::option(args, 1, kwargs, "whence")?)?;
            if self.closed() {
                return Err(closed());
            }
            // A failed seek on a pipe must leave pending bytes in the Python buffer.
            if self.call(ctx, receiver, "seekable", &[], &[])? != TRUE {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "File or stream is not seekable.",
                ));
            }
            let offset = files::buffered_offset(ctx, target)?;
            self.check_raw(ctx, receiver, "seek")?;
            let fd = self.handle.live_fd().ok_or_else(closed)?;
            self.call(ctx, receiver, "flush", &[], &[])?;
            let pos = crate::objects::os::seek(fd, offset, whence)?;
            return Ok(ctx.int(pos));
        }
        let seek = if name == "seek" {
            let offset = ctx
                .index_integer(
                    *args
                        .first()
                        .ok_or_else(|| Error::new("TypeError", "seek requires an offset"))?,
                )?
                .to_i64()
                .ok_or_else(|| Error::new("OverflowError", "offset out of range"))?;
            match super::super::option(args, 1, kwargs, "whence")?
                .map(|v| ctx.index_integer(v))
                .transpose()?
                .map(|v| v.to_i64())
                .unwrap_or(Some(0))
            {
                Some(0) if offset >= 0 => Some(SeekFrom::Start(offset as u64)),
                Some(1) => Some(SeekFrom::Current(offset)),
                Some(2) => Some(SeekFrom::End(offset)),
                _ => return Err(Error::new("ValueError", "invalid seek")),
            }
        } else {
            None
        };
        self.check_raw(ctx, receiver, name)?;
        let fd = self.handle.live_fd().ok_or_else(closed)?;
        let mut slot = self.buffer.borrow_mut();
        let buffer = slot.as_mut().ok_or_else(closed)?;
        match name {
            "__enter__" => Ok(receiver),
            "fileno" => Ok(ctx.int(i64::from(fd))),
            "readable" => Ok(builtins::boolean(self.reading)),
            "writable" => Ok(builtins::boolean(self.writing)),
            // SAFETY: isatty inspects the integer descriptor without retaining a reference.
            "isatty" => Ok(builtins::boolean(unsafe { libc::isatty(fd) } != 0)),
            "seekable" => Ok(builtins::boolean(
                // SAFETY: lseek accepts an integer descriptor and a zero offset.
                unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) } >= 0,
            )),
            "flush" => {
                match buffer {
                    Buffer::Random(file) => file.flush().map_err(os_error)?,
                    Buffer::Writer(w) => w.flush().map_err(|err| buffer_error(ctx, err))?,
                    Buffer::Pipe(pipe) => pipe.flush()?,
                    Buffer::Reader(_) => {}
                }
                Ok(NONE)
            }
            "tell" => Ok(ctx.int(buffer.position().map_err(os_error)? as i64)),
            "seek" => {
                let pos = buffer
                    .seek(seek.expect("validated seek"))
                    .map_err(|err| buffer_error(ctx, err))?;
                Ok(ctx.int(pos as i64))
            }
            "read" | "read1" | "readline" => {
                if !self.reading {
                    return Err(Error::new("UnsupportedOperation", "read"));
                }
                let size = size.unwrap_or(-1);
                if name == "read1" {
                    let bytes = match buffer {
                        Buffer::Reader(r) => read_once(r, size)?,
                        Buffer::Random(file) => {
                            let size = if size < 0 {
                                file.capacity() as i64
                            } else {
                                size
                            };
                            let mut bytes = builtins::zeroed_bytes(size)?;
                            let count = file.read_once(&mut bytes).map_err(os_error)?;
                            bytes.truncate(count);
                            bytes
                        }
                        Buffer::Pipe(pipe) => pipe.read1(usize::try_from(size).ok())?,
                        Buffer::Writer(_) => unreachable!(),
                    };
                    return Ok(ctx.alloc(Value::Bytes(bytes)));
                }
                if name == "read" && size < -1 {
                    return Err(Error::new(
                        "ValueError",
                        "read length must be non-negative or -1",
                    ));
                }
                if name == "read" && size > crate::MAX_BYTES_LEN as i64 {
                    return Err(Error::new("OverflowError", "byte string is too large"));
                }
                if let Buffer::Pipe(pipe) = buffer {
                    let data = pipe.read(usize::try_from(size).ok(), name == "readline")?;
                    return Ok(ctx.alloc(Value::Bytes(data)));
                }
                let r: &mut dyn BufRead = match buffer {
                    Buffer::Reader(r) if name == "read" => {
                        return Ok(
                            read_bytes(r, size)?.map_or(NONE, |data| ctx.alloc(Value::Bytes(data)))
                        );
                    }
                    Buffer::Random(file) if name == "read" => {
                        let data = file.read_bytes(usize::try_from(size).ok())?;
                        return Ok(ctx.alloc(Value::Bytes(data)));
                    }
                    Buffer::Reader(r) => r,
                    Buffer::Random(file) => file,
                    _ => unreachable!(),
                };
                let mut reader = r.take(if size < 0 { u64::MAX } else { size as u64 });
                let mut data = Vec::new();
                reader.read_until(b'\n', &mut data).map_err(os_error)?;
                Ok(ctx.alloc(Value::Bytes(data)))
            }
            "readinto" if args.len() == 1 => {
                if !self.reading {
                    return Err(Error::new("UnsupportedOperation", "readinto"));
                }
                let Value::ByteArray(bytes) = ctx.heap.get_mut(args[0]) else {
                    return Err(Error::new(
                        "TypeError",
                        "readinto requires a writable buffer",
                    ));
                };
                if let Buffer::Pipe(pipe) = buffer {
                    let data = pipe.read(Some(bytes.len()), false)?;
                    bytes[..data.len()].copy_from_slice(&data);
                    return Ok(ctx.int(data.len() as i64));
                }
                let r: &mut dyn Read = match buffer {
                    Buffer::Reader(r) => r,
                    Buffer::Random(file) => file,
                    _ => unreachable!(),
                };
                let mut total = 0;
                while total < bytes.len() {
                    let n = r.read(&mut bytes[total..]).map_err(os_error)?;
                    if n == 0 {
                        break;
                    }
                    total += n;
                }
                Ok(ctx.int(total as i64))
            }
            "write" if args.len() == 1 => {
                if !self.writing {
                    return Err(Error::new("UnsupportedOperation", "write"));
                }
                let data = files::output_bytes(ctx, args[0])?;
                let count = data.len();
                match buffer {
                    Buffer::Pipe(pipe) => pipe.write(data)?,
                    Buffer::Writer(w) => w.write_all(data).map_err(|err| buffer_error(ctx, err))?,
                    Buffer::Random(file) => file.write(data).map_err(os_error)?,
                    Buffer::Reader(_) => unreachable!(),
                }
                Ok(ctx.int(count as i64))
            }
            _ => Err(Error::new(
                "TypeError",
                format!("unsupported buffered descriptor call: {name}"),
            )),
        }
    }
}

impl Drop for BufferedFd {
    fn drop(&mut self) {
        if self.drop_fallback.as_ref().is_none_or(Cell::get) && self.buffer.get_mut().is_some() {
            let _ = self.close(None, true);
        }
    }
}
