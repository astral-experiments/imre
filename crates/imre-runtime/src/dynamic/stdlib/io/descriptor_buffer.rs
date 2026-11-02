//! Buffer a selected FileIO descriptor without changing its identity or ownership.
use std::cell::RefCell;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::fd::RawFd;
use std::rc::Rc;

use super::super::{builtins, files, os_error};
use super::file_io::RawFile;
use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::BoxedPipe;

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

enum Buffer {
    Reader(BufReader<Descriptor>),
    Writer(BufWriter<Descriptor>),
    // The process engine allocated this buffer before launching the child.
    Pipe(BoxedPipe),
}

impl Buffer {
    fn position(&mut self) -> io::Result<u64> {
        // The caller can reposition the descriptor through another handle.
        match self {
            Self::Pipe(_) => Err(io::Error::from_raw_os_error(libc::ESPIPE)),
            Self::Reader(r) => Ok(r
                .get_mut()
                .stream_position()?
                .saturating_sub(r.buffer().len() as u64)),
            Self::Writer(w) => Ok(w.get_mut().stream_position()? + w.buffer().len() as u64),
        }
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let reader = match self {
            Self::Pipe(_) => return Err(io::Error::from_raw_os_error(libc::ESPIPE)),
            Self::Reader(r) => r,
            Self::Writer(w) => return w.seek(position),
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
            Buffer::Writer(BufWriter::with_capacity(capacity, descriptor))
        };
        Self {
            raw,
            reading,
            handle,
            buffer: RefCell::new(Some(buffer)),
        }
    }

    pub(super) fn pipe(raw: Id, handle: Rc<RawFile>, reading: bool, pipe: BoxedPipe) -> Self {
        Self {
            raw,
            handle,
            reading,
            buffer: RefCell::new(Some(Buffer::Pipe(pipe))),
        }
    }

    pub(super) fn uses_raw(&self, raw: &Rc<RawFile>) -> bool {
        Rc::ptr_eq(raw, &self.handle)
    }

    pub(in crate::dynamic) fn closed(&self) -> bool {
        self.handle.live_fd().is_none()
    }

    fn close(&self) -> Result<()> {
        let pending = match self.buffer.borrow_mut().take() {
            Some(Buffer::Pipe(pipe)) => pipe.close().map_err(Into::into),
            Some(Buffer::Writer(mut writer)) => {
                let flushed = if self.closed() {
                    Ok(())
                } else {
                    writer.flush().map_err(os_error)
                };
                let _ = writer.into_parts(); // A failed flush must not be retried by BufWriter::drop.
                flushed
            }
            _ => Ok(()),
        };
        self.handle.close().and(pending)
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
            super::native_reader::check_raw(ctx, receiver, self, name)?;
            self.close()?;
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
        super::native_reader::check_raw(ctx, receiver, self, name)?;
        let fd = self.handle.live_fd().ok_or_else(closed)?;
        let mut slot = self.buffer.borrow_mut();
        let buffer = slot.as_mut().ok_or_else(closed)?;
        match name {
            "__enter__" => Ok(receiver),
            "fileno" => Ok(ctx.int(i64::from(fd))),
            "readable" => Ok(builtins::boolean(self.reading)),
            "writable" => Ok(builtins::boolean(!self.reading)),
            // SAFETY: isatty inspects the integer descriptor without retaining a reference.
            "isatty" => Ok(builtins::boolean(unsafe { libc::isatty(fd) } != 0)),
            "seekable" => Ok(builtins::boolean(
                // SAFETY: lseek accepts an integer descriptor and a zero offset.
                unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) } >= 0,
            )),
            "flush" => {
                match buffer {
                    Buffer::Writer(w) => w.flush().map_err(os_error)?,
                    Buffer::Pipe(pipe) => pipe.flush()?,
                    Buffer::Reader(_) => {}
                }
                Ok(NONE)
            }
            "tell" => Ok(ctx.int(buffer.position().map_err(os_error)? as i64)),
            "seek" => Ok(ctx.int(
                buffer
                    .seek(seek.expect("validated seek"))
                    .map_err(os_error)? as i64,
            )),
            "read" | "read1" | "readline" => {
                if !self.reading {
                    return Err(Error::new("UnsupportedOperation", "read"));
                }
                let size = size.unwrap_or(-1);
                if name == "read1" {
                    let bytes = match buffer {
                        Buffer::Reader(r) => read_once(r, size)?,
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
                let Buffer::Reader(r) = buffer else {
                    unreachable!()
                };
                if name == "read" {
                    return Ok(
                        read_bytes(r, size)?.map_or(NONE, |data| ctx.alloc(Value::Bytes(data)))
                    );
                }
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
                let Buffer::Reader(r) = buffer else {
                    unreachable!()
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
                if self.reading {
                    return Err(Error::new("UnsupportedOperation", "write"));
                }
                let data = ctx.bytes_like(args[0])?;
                match buffer {
                    Buffer::Pipe(pipe) => pipe.write(data)?,
                    Buffer::Writer(w) => w.write_all(data).map_err(os_error)?,
                    Buffer::Reader(_) => unreachable!(),
                }
                Ok(ctx.int(data.len() as i64))
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
        let _ = self.close();
    }
}
