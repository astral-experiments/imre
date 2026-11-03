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

/// An explicit buffered wrapper retains the caller's FileIO and controls its close.
fn wrapper_fallback(ctx: &Context, owner: Id) -> Option<(Id, &BufferedFd, &Cell<bool>)> {
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

/// The raw object of a reader constructed from a caller's descriptor.
pub(super) fn explicit_reader_raw(ctx: &Context, receiver: Id) -> Option<Id> {
    let (_, fd, _) = wrapper_fallback(ctx, receiver)?;
    (fd.reading && !fd.writing).then_some(fd.raw)
}

/// Disarm native drop before giving up the writer's current backing.
fn retire_writer(ctx: &Context, receiver: Id) -> Result<Option<Id>> {
    let raw = match ctx.heap.get(receiver) {
        Value::Instance {
            memory_stream: Some(MemoryStream::BufferedBytes(writer)),
            ..
        } => {
            writer.active.check_idle()?;
            writer.raw.get()
        }
        Value::Instance {
            memory_stream: Some(MemoryStream::UninitializedWriter(raw)),
            ..
        } => *raw,
        Value::Instance {
            memory_stream: Some(MemoryStream::DetachedWriter) | None,
            ..
        } => None,
        _ => {
            let Some((_, fd, fallback)) =
                wrapper_fallback(ctx, receiver).filter(|(_, fd, _)| fd.writing && !fd.reading)
            else {
                return Err(Error::new(
                    "NotImplementedError",
                    "reinitializing a directly owned file writer is unsupported",
                ));
            };
            fd.active.check_idle()?;
            // Retiring this backing must neither flush pending writes nor close the caller's FileIO.
            fallback.set(false);
            Some(fd.raw)
        }
    };
    Ok(raw)
}

/// Argument conversion precedes invalidation. During writable() raw still names
/// the outgoing stream; after it succeeds raw names the replacement even on error.
fn invalidate_writer(ctx: &mut Context, receiver: Id, replacement: Option<Id>) -> Result<()> {
    let previous = retire_writer(ctx, receiver)?;
    let raw = replacement.or(previous);
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    *memory_stream = Some(MemoryStream::UninitializedWriter(raw));
    if let Some(old) = previous
        && raw != previous
    {
        ctx.heap.weak_edge_removed(old);
    }
    Ok(())
}

/// A flush override may reinitialize or detach this writer before returning.
pub(super) fn detach_writer(ctx: &mut Context, receiver: Id) -> Result<Id> {
    if !wrapper_fallback(ctx, receiver).is_some_and(|(_, fd, _)| fd.writing && !fd.reading) {
        return Err(Error::new("UnsupportedOperation", "detach"));
    }
    let flush = ctx.lookup_attr(receiver, "flush")?;
    ctx.call(flush, Vec::new(), Vec::new())?;
    let raw = retire_writer(ctx, receiver)?;
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    *memory_stream = Some(MemoryStream::DetachedWriter);
    let raw = raw.ok_or_else(|| {
        Error::new(
            "SystemError",
            "detach returned NULL without setting an exception",
        )
    })?;
    ctx.heap.weak_edge_removed(raw);
    Ok(raw)
}

/// A successful nested __init__ in tell() supplies the final raw and buffer.
/// CPython resets that buffer after the outer position probe returns.
pub(super) fn finish_writer_probe(ctx: &Context, receiver: Id) -> Result<()> {
    if let Some(writer) = super::bytes_writer::state(ctx, receiver) {
        return writer.finish_probe();
    }
    if let Some((_, fd, _)) = wrapper_fallback(ctx, receiver)
        && let Some(Buffer::Writer(output)) = fd.buffer.borrow_mut().as_mut()
    {
        output.buffer.clear();
        output.written = 0;
        return Ok(());
    }
    Err(Error::new(
        "NotImplementedError",
        "position probe left the buffered writer uninitialized",
    ))
}

/// A descriptor installed inside a Random raw.tell() keeps its physical position.
pub(super) fn finish_random_probe(ctx: &Context, receiver: Id) -> bool {
    let Some((_, fd, _)) = wrapper_fallback(ctx, receiver) else {
        return false;
    };
    if let Some(Buffer::Random(file)) = fd.buffer.borrow_mut().as_mut() {
        file.reset();
    }
    true
}

/// A Python finalizer can decline close, raise, or resurrect the writer.
pub(in crate::dynamic) fn note_writer_finalizer(ctx: &Context, owner: Id) {
    if let Some((_, _, armed)) = wrapper_fallback(ctx, owner) {
        armed.set(false);
    }
}

/// A live module global can reach Rust Drop without running its Python finalizer.
/// Permit the native flush only while it still implements the writer's close.
pub(in crate::dynamic) fn prepare_writer_shutdown(ctx: &Context) {
    for &owner in ctx.heap.pending_finalizers() {
        let Some((class, fd, armed)) = wrapper_fallback(ctx, owner) else {
            continue;
        };
        if !armed.get() || fd.closed() {
            continue;
        }
        let kind = if fd.reading {
            "BufferedRandom"
        } else {
            "BufferedWriter"
        };
        let base = ctx.find_in_mro(class, |candidate| {
            (ctx.heap.get(candidate).io_kind() == Some(kind)).then_some(candidate)
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

const RANDOM_RAW_METHODS: &[&str] = &[
    "close", "flush", "readable", "writable", "seekable", "isatty", "fileno", "tell", "seek",
    "read", "readall", "readinto", "write",
];

fn native_handle(ctx: &Context, raw: Id, methods: &[&str]) -> Option<Rc<RawFile>> {
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
    {
        file_io::state(ctx, raw)
    } else {
        None
    }
}

fn native_raw(ctx: &Context, raw: Id, methods: &[&str], owner: &str) -> Result<Rc<RawFile>> {
    native_handle(ctx, raw, methods).ok_or_else(|| {
        Error::new(
            "NotImplementedError",
            format!("{owner} with Python raw methods is unsupported"),
        )
    })
}

pub(super) fn ssize(ctx: &mut Context, value: Id) -> Result<i64> {
    ctx.index_integer(value)?.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

fn random_initialized(ctx: &Context, receiver: Id) -> bool {
    backing(ctx, receiver).is_some()
        || matches!(
            ctx.heap.get(receiver),
            Value::Instance {
                memory_stream: Some(MemoryStream::Reader(_)),
                ..
            }
        )
}

pub(super) fn check_capabilities(ctx: &mut Context, raw: Id, capabilities: &[&str]) -> Result<()> {
    for &capability in capabilities {
        let method = ctx.lookup_attr(raw, capability)?;
        if ctx.call(method, vec![], Vec::new())? != TRUE {
            return Err(Error::new(
                "UnsupportedOperation",
                format!("File or stream is not {capability}."),
            ));
        }
    }
    Ok(())
}

/// Attach one buffered stream to a caller-owned native FileIO, without reopening its fd.
pub(super) fn initialize(
    ctx: &mut Context,
    receiver: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
    owner: &str,
) -> Result<Id> {
    let (capabilities, methods): (&[&str], &[&str]) = match owner {
        "BufferedReader" => (
            &["readable"],
            &[
                "close", "flush", "readable", "seekable", "isatty", "fileno", "tell", "seek",
                "read", "readall", "readinto",
            ],
        ),
        "BufferedWriter" => (
            &["writable"],
            &[
                "close", "flush", "writable", "seekable", "isatty", "fileno", "tell", "seek",
                "write", "truncate",
            ],
        ),
        "BufferedRandom" => (&["seekable", "readable", "writable"], RANDOM_RAW_METHODS),
        _ => unreachable!("sealed buffered class"),
    };
    let [raw, capacity] =
        builtins::bind_builtin_arguments(owner, args, kwargs, ["raw", "buffer_size"], 1)?;
    let raw = raw.expect("required argument");
    let capacity = capacity
        .map(|n| ssize(ctx, n))
        .transpose()?
        .unwrap_or(super::DEFAULT_BUFFER_SIZE);
    // A FileIO subclass still owns a native fd, but its Python methods must run.
    if owner == "BufferedReader"
        && native_handle(ctx, raw, methods).is_none()
        && (backing(ctx, receiver).is_none() || explicit_reader_raw(ctx, receiver).is_some())
    {
        return super::buffered_reader::initialize(ctx, receiver, raw, capacity);
    }
    if owner == "BufferedRandom"
        && super::buffered_reader::reinitialize_update(ctx, receiver, raw, capacity, capabilities)?
    {
        return Ok(NONE);
    }
    if owner == "BufferedWriter" {
        invalidate_writer(ctx, receiver, None)?;
    }
    check_capabilities(ctx, raw, capabilities)?;
    let mut revived = false;
    if owner == "BufferedWriter" {
        // A successful nested initialization in writable() can make methods usable during tell().
        revived = super::bytes_writer::state(ctx, receiver).is_some_and(|w| w.ready.get())
            || backing(ctx, receiver).is_some();
        invalidate_writer(ctx, receiver, Some(raw))?;
    }
    if owner == "BufferedRandom" && !random_initialized(ctx, receiver) {
        // The public raw slot is visible during size validation and raw.tell().
        super::buffered_reader::start_init(ctx, receiver, Some(raw))?;
    }
    if capacity <= 0 {
        return Err(Error::new(
            "ValueError",
            "buffer size must be strictly positive",
        ));
    }
    let handle = match native_raw(ctx, raw, methods, owner) {
        Ok(handle) => handle,
        Err(_) if owner == "BufferedRandom" => {
            if random_initialized(ctx, receiver) {
                return Err(Error::new(
                    "NotImplementedError",
                    "BufferedRandom reinitialization is unsupported",
                ));
            }
            return super::buffered_reader::install(ctx, receiver, raw, capacity, true);
        }
        Err(_) if owner == "BufferedWriter" => {
            return super::bytes_writer::initialize(ctx, receiver, raw, capacity as usize, revived);
        }
        Err(error) => return Err(error),
    };
    if backing(ctx, receiver).is_some()
        || super::bytes_writer::state(ctx, receiver).is_some()
        || owner == "BufferedRandom" && random_initialized(ctx, receiver)
    {
        return Err(Error::new(
            "NotImplementedError",
            format!("{owner} reinitialization is unsupported"),
        ));
    }
    let mut fd = if owner == "BufferedRandom" {
        BufferedFd::update(raw, handle, capacity as usize)?
    } else {
        BufferedFd::new(raw, handle, owner == "BufferedReader", capacity as usize)?
    };
    // A reader has no output to flush at shutdown. Only its Python close or
    // the last owner of the raw stream should close the caller's descriptor.
    fd.drop_fallback = Some(Cell::new(owner != "BufferedReader"));
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

    fn seek_raw(&mut self, offset: i64, whence: i32) -> io::Result<u64> {
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
        self.seek_raw(offset, whence)
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
    blocking_error(ctx, blocked.errno, blocked.accepted)
}

pub(super) fn blocking_error(ctx: &mut Context, errno: i32, accepted: usize) -> Error {
    let errno = ctx.int(i64::from(errno));
    let message = ctx.text(BlockedWrite::DESCRIPTION);
    let accepted = ctx.int(accepted as i64);
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

    fn seek(&mut self, offset: i64, whence: i32) -> io::Result<u64> {
        let position = match whence {
            libc::SEEK_SET if offset >= 0 => Some(SeekFrom::Start(offset as u64)),
            libc::SEEK_CUR => Some(SeekFrom::Current(offset)),
            libc::SEEK_END => Some(SeekFrom::End(offset)),
            _ => None,
        };
        let reader = match self {
            Self::Random(file) => {
                return match position {
                    Some(at) => file.seek(at),
                    None => file.seek_raw(|raw, _| raw.seek_raw(offset, whence)),
                };
            }
            Self::Pipe(_) => return Err(io::Error::from_raw_os_error(libc::ESPIPE)),
            Self::Reader(r) => r,
            Self::Writer(w) => {
                w.flush()?;
                return w.inner.seek_raw(offset, whence);
            }
        };
        let Some(position) = position else {
            let pos = reader.get_mut().seek_raw(offset, whence)?;
            let buffered = reader.buffer().len();
            reader.consume(buffered);
            return Ok(pos);
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
    callback_active: Cell<bool>,
    pub(super) active: super::native_writer::ActiveCall,
    buffer: RefCell<Option<Buffer>>,
}

impl BufferedFd {
    pub(super) fn new(
        raw: Id,
        handle: Rc<RawFile>,
        reading: bool,
        capacity: usize,
    ) -> Result<Self> {
        let mut descriptor = Descriptor {
            handle: handle.clone(),
            raw_position: None,
            last_read: 0,
        };
        let buffer = if reading {
            // std::io::BufReader cannot adopt fallibly reserved storage.
            if capacity > crate::text_output::binary_capacity(i128::MAX) {
                return Err(Error::new(
                    "NotImplementedError",
                    "explicit BufferedReader buffers above 8 MiB are unsupported",
                ));
            }
            let _ = descriptor.stream_position();
            Buffer::Reader(BufReader::with_capacity(capacity, descriptor))
        } else {
            let mut pending = Vec::new();
            pending
                .try_reserve_exact(capacity)
                .map_err(|_| Error::new("MemoryError", ""))?;
            Buffer::Writer(BinaryOutput::with_buffer(pending, descriptor))
        };
        Ok(Self {
            raw,
            reading,
            writing: !reading,
            handle,
            drop_fallback: None,
            callback_active: Cell::new(false),
            active: super::native_writer::ActiveCall::default(),
            buffer: RefCell::new(Some(buffer)),
        })
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
            callback_active: Cell::new(false),
            active: super::native_writer::ActiveCall::default(),
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
            callback_active: Cell::new(false),
            active: super::native_writer::ActiveCall::default(),
            buffer: RefCell::new(Some(Buffer::Pipe(pipe))),
        }
    }

    /// Check the live raw after any argument conversion which can invoke Python.
    fn check_raw(&self, ctx: &Context, receiver: Id, name: &str) -> Result<()> {
        if name != "seek" && matches!(ctx.heap.get(receiver), Value::File(_)) {
            return Ok(());
        }
        let (owner, methods): (_, &[&str]) = if self.reading && self.writing {
            let methods: &[&str] = match name {
                // These operations synchronize the cursor. truncate and its subsequent
                // tell are looked up on the live raw object after synchronization.
                "seek" => &["write", "seek", "tell"],
                "truncate" | "flush" => &["write", "seek"],
                "close" => &["write", "seek", "flush", "close"],
                _ => RANDOM_RAW_METHODS,
            };
            ("BufferedRandom", methods)
        } else if self.reading {
            (
                "BufferedReader",
                match name {
                    "read" => &["read", "readall", "readinto"],
                    "read1" | "readline" | "readinto" | "peek" => &["readinto"],
                    "seek" => &["seek", "tell"],
                    "close" => &["flush", "close"],
                    _ => std::slice::from_ref(&name),
                },
            )
        } else {
            (
                "BufferedWriter",
                match name {
                    "flush" => &["write"],
                    "seek" => &["write", "seek"],
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

    pub(super) fn warn_on_close(&self, ctx: &mut Context, receiver: Id) -> Result<()> {
        self.check_reentry(ctx)?;
        if ctx.raw_instance_field(receiver, "_finalizing") == Some(TRUE) {
            self.callback_active.set(true);
            super::buffered_io::dealloc_warn(ctx, receiver, receiver);
            self.callback_active.set(false);
        }
        Ok(())
    }

    /// A raw method or Python signal handler can call back into its own buffer.
    fn check_reentry(&self, ctx: &Context) -> Result<()> {
        if self.callback_active.get() {
            let name = ctx.raw_instance_field(self.raw, "name");
            let kind = match (self.reading, self.writing) {
                (true, true) => "BufferedRandom",
                (_, true) => "BufferedWriter",
                _ => "BufferedReader",
            };
            let value = name.map_or_else(
                || format!("<_io.{kind}>"),
                |id| format!("<_io.{kind} name={}>", ctx.repr(id)),
            );
            return Err(Error::new(
                "RuntimeError",
                format!("reentrant call inside {value}"),
            ));
        }
        Ok(())
    }

    /// Realign the raw offset first and let the live FileIO truncate implement its size protocol.
    fn truncate(&self, ctx: &mut Context, size: Id) -> Result<Id> {
        self.check_reentry(ctx)?;
        // The native flush writes and seeks on the descriptor without calling Python.
        native_raw(ctx, self.raw, &["write", "seek"], "BufferedRandom")?;
        {
            let mut slot = self.buffer.borrow_mut();
            let Some(Buffer::Random(file)) = slot.as_mut() else {
                return Err(Error::new("ValueError", "truncate of closed file"));
            };
            file.flush().map_err(os_error)?;
        }
        self.callback_active.set(true);
        let result = ctx
            .lookup_attr(self.raw, "truncate")
            .and_then(|truncate| ctx.call(truncate, vec![size], Vec::new()));
        if let Ok(id) = result {
            ctx.heap.keep(id);
            // CPython refreshes the raw position after success and ignores a failed tell.
            let _ = ctx
                .lookup_attr(self.raw, "tell")
                .and_then(|tell| ctx.call(tell, Vec::new(), Vec::new()));
        }
        self.callback_active.set(false);
        result
    }

    fn close(&self, mut ctx: Option<&mut Context>, flush: bool) -> Result<()> {
        let pending = match self.buffer.borrow_mut().take() {
            Some(Buffer::Pipe(pipe)) => pipe.close().map_err(Into::into),
            Some(Buffer::Random(mut file)) if flush && !self.closed() => {
                file.flush().map_err(os_error)
            }
            Some(Buffer::Writer(mut writer)) if flush && !self.closed() => {
                writer.flush().map_err(|error| {
                    if let Some(ctx) = ctx.as_deref_mut() {
                        buffer_error(ctx, error)
                    } else {
                        os_error(error)
                    }
                })
            }
            _ => Ok(()),
        };
        match (pending, self.handle.close(), ctx) {
            (Err(mut flush), Err(mut close), Some(ctx)) => {
                if flush.context == NONE {
                    flush.context = ctx.heap.active_exception();
                }
                close.context = ctx.raised_instance(flush);
                Err(close)
            }
            (pending, closed, _) => closed.and(pending),
        }
    }

    /// The buffered wrapper has already called its public flush method.
    pub(super) fn close_after_flush(&self, ctx: &Context, receiver: Id) -> Result<()> {
        self.check_reentry(ctx)?;
        self.check_raw(ctx, receiver, "close")?;
        self.close(None, false)
    }

    pub(in crate::dynamic) fn call(
        &self,
        ctx: &mut Context,
        receiver: Id,
        name: &str,
        args: &[Id],
    ) -> Result<Id> {
        if matches!((name, args), ("close", []) | ("__exit__", [_, _, _])) {
            self.check_reentry(ctx)?;
            self.check_raw(ctx, receiver, name)?;
            self.close(Some(ctx), true)?;
            return Ok(NONE);
        }
        if let ("readinto", [target]) = (name, args) {
            files::readinto_target(ctx, *target)?;
            if !self.reading {
                return Err(Error::new("UnsupportedOperation", "read"));
            }
        }
        let closed = || {
            Error::new(
                "ValueError",
                match name {
                    "read" | "read1" => "read of closed file",
                    "readline" => "readline of closed file",
                    "peek" => "peek of closed file",
                    "readinto" => "readinto of closed file",
                    "truncate" => "truncate of closed file",
                    "seek" => "seek of closed file",
                    "write" => "write to closed file",
                    "flush" if self.writing => "flush of closed file",
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
        if name == "seek" {
            let target = args[0];
            let whence = files::buffered_whence(ctx, args.get(1).copied())?;
            if self.closed() {
                return Err(closed());
            }
            let seekable = ctx.lookup_attr(self.raw, "seekable")?;
            if ctx.call(seekable, vec![], Vec::new())? != TRUE {
                return Err(Error::new(
                    "UnsupportedOperation",
                    "File or stream is not seekable.",
                ));
            }
            // Conversion can move or close this stream, or replace methods on its raw.
            let offset = files::buffered_offset(ctx, target)?;
            if self.closed() {
                return Err(Error::new("ValueError", "I/O operation on closed file"));
            }
            self.check_raw(ctx, receiver, name)?;
            self.check_reentry(ctx)?;
            let mut slot = self.buffer.borrow_mut();
            let position = slot
                .as_mut()
                .ok_or_else(closed)?
                .seek(offset, whence)
                .map_err(|err| buffer_error(ctx, err))?;
            return Ok(ctx.int(position as i64));
        }
        self.check_raw(ctx, receiver, name)?;
        let fd = self.handle.live_fd().ok_or_else(closed)?;
        if name == "truncate" {
            return self.truncate(ctx, args.first().copied().unwrap_or(NONE));
        }
        if name == "read" && size == Some(-1) && self.reading {
            self.check_reentry(ctx)?;
            // The public BufferedReader and Popen share this pipe's read-ahead. Consume
            // it once and release the file and buffer borrows before dispatching Python.
            let prefix = match self.buffer.borrow().as_ref() {
                Some(Buffer::Pipe(pipe)) => Some(pipe.take_read_ahead()?),
                _ => None,
            };
            if let Some(bytes) = prefix {
                self.callback_active.set(true);
                let result = file_io::read(ctx, &self.handle, -1, false, bytes);
                self.callback_active.set(false);
                return result;
            }
        }
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
                // A reader forwards flush to the raw without entering the buffer lock.
                if self.writing {
                    self.check_reentry(ctx)?;
                }
                match buffer {
                    Buffer::Random(file) => file.flush().map_err(os_error)?,
                    Buffer::Writer(w) => w.flush().map_err(|err| buffer_error(ctx, err))?,
                    Buffer::Pipe(pipe) => pipe.flush()?,
                    Buffer::Reader(_) => {}
                }
                Ok(NONE)
            }
            "tell" => Ok(ctx.int(buffer.position().map_err(os_error)? as i64)),
            "peek" => {
                self.check_reentry(ctx)?;
                let data = match buffer {
                    Buffer::Reader(reader) => crate::objects::peek_bytes(reader)?,
                    Buffer::Random(reader) => reader.peek()?,
                    Buffer::Pipe(pipe) => pipe.peek()?,
                    Buffer::Writer(_) => unreachable!("peek requires a buffered reader"),
                };
                Ok(ctx.alloc(Value::Bytes(data)))
            }
            "read" | "read1" | "readline" => {
                if !self.reading {
                    return Err(Error::new("UnsupportedOperation", "read"));
                }
                let size = size.unwrap_or(-1);
                if name == "read" && size < -1 {
                    return Err(Error::new(
                        "ValueError",
                        "read length must be non-negative or -1",
                    ));
                }
                if size != 0 {
                    self.check_reentry(ctx)?;
                }
                if name == "read1" {
                    let bytes = match buffer {
                        Buffer::Reader(r) => read_once(r, size)?,
                        Buffer::Random(file) => {
                            let size = file.read_once_size(usize::try_from(size).ok());
                            let mut bytes = builtins::zeroed_bytes(size as i64)?;
                            let count = file.read_once(&mut bytes).map_err(os_error)?;
                            bytes.truncate(count);
                            bytes
                        }
                        Buffer::Pipe(pipe) => pipe.read1(usize::try_from(size).ok())?,
                        Buffer::Writer(_) => unreachable!(),
                    };
                    return Ok(ctx.alloc(Value::Bytes(bytes)));
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
                if self.callback_active.get() {
                    files::readinto_target(ctx, args[0])?;
                    self.check_reentry(ctx)?;
                }
                let bytes = files::readinto_bytes(ctx, args[0])?;
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
                    let n = match r.read(&mut bytes[total..]) {
                        Ok(n) => n,
                        Err(e) => return files::readinto_error(ctx, e, total),
                    };
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
                self.check_reentry(ctx)?;
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
