//! Positioned binary I/O on ordinary files, in-memory files, and ZIP members.

use super::super::{c_int, zipfile::Member};
use super::*;
use num_traits::ToPrimitive;

pub(super) struct Memory {
    data: crate::BytesOwner,
    position: u64,
}

fn copied(data: &[u8]) -> Result<Bytes> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(data.len())
        .map_err(|_| Error::allocation())?;
    bytes.extend_from_slice(data);
    Ok(bytes)
}

impl Memory {
    fn remaining(&self) -> &[u8] {
        self.data.get(self.position as usize..).unwrap_or(&[])
    }

    fn read(&mut self, size: Option<usize>) -> Result<Bytes> {
        let data = self.remaining();
        let end = size.unwrap_or(usize::MAX).min(data.len());
        let result = copied(&data[..end])?;
        self.position += end as u64;
        Ok(result)
    }

    pub(super) fn write(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let start = self.position as usize;
        let end = start
            .checked_add(bytes.len())
            .ok_or_else(Error::allocation)?;
        if crate::BytesOwner::get_mut(&mut self.data).is_none() {
            self.data = crate::BytesOwner::new(copied(&self.data)?);
        }
        let data = crate::BytesOwner::get_mut(&mut self.data).unwrap();
        if end > data.len() {
            data.try_reserve(end - data.len())
                .map_err(|_| Error::allocation())?;
            data.resize(end, 0);
        }
        data[start..end].copy_from_slice(bytes);
        self.position = end as u64;
        Ok(())
    }

    fn seek(&mut self, at: SeekFrom) -> Result<u64> {
        let position = match at {
            SeekFrom::Start(n) => n,
            SeekFrom::Current(n) => self.position.saturating_add_signed(n),
            SeekFrom::End(n) => (self.data.len() as u64).saturating_add_signed(n),
        };
        if position > isize::MAX as u64 {
            return Err(Error::new(Kind::OverflowError, "new position too large"));
        }
        self.position = position;
        Ok(position)
    }
}

impl FileState {
    pub(in crate::objects) fn memory(data: crate::BytesOwner) -> crate::ObjectOwner {
        Self {
            fd: None,
            raw: false,
            name: None,
            kind: FileKind::Memory,
            stream: Some(Stream::Memory(Memory { data, position: 0 })),
        }
        .into_object()
    }

    pub(in crate::objects) fn member(member: Member) -> crate::ObjectOwner {
        Self {
            fd: None,
            raw: false,
            name: Some(member.name().clone()),
            kind: FileKind::Member,
            stream: Some(Stream::Member(Box::new(member))),
        }
        .into_object()
    }

    pub(super) fn into_object(self) -> crate::ObjectOwner {
        crate::ObjectOwner::new(Object(StorageCell::new(Storage::File(Box::new(self)))))
    }

    pub(super) fn check_member(&mut self, message: &'static str) -> Result<()> {
        if self.kind == FileKind::Member && self.stream.is_none() {
            Err(Error::new(Kind::ValueError, message))
        } else {
            Ok(())
        }
    }

    pub(in crate::objects) fn read_bytes(&mut self, limit: Option<usize>) -> Result<Bytes> {
        match self.kind {
            FileKind::Memory => match self.stream()? {
                Stream::Memory(m) => m.read(limit),
                _ => unreachable!(),
            },
            FileKind::Member => {
                self.check_member("read from closed file.")?;
                match self.stream()? {
                    Stream::Member(m) => m.read(limit),
                    _ => unreachable!(),
                }
            }
            _ => self.input()?.read_bytes(limit),
        }
    }

    pub(super) fn read_line_bytes(&mut self) -> Result<Bytes> {
        match self.kind {
            FileKind::Memory => match self.stream()? {
                Stream::Memory(m) => m.read(memchr::memchr(b'\n', m.remaining()).map(|i| i + 1)),
                _ => unreachable!(),
            },
            FileKind::Member => {
                self.check_member("read from closed file.")?;
                match self.stream()? {
                    Stream::Member(m) => m.readline(),
                    _ => unreachable!(),
                }
            }
            _ => self.input()?.binary_line(),
        }
    }

    /// Convert a signed absolute seek using this stream's Python rules.
    pub(in crate::objects) fn absolute_offset(&mut self, offset: i128) -> Result<u64> {
        match self.kind {
            FileKind::Memory => {
                let signed = i64::try_from(offset).map_err(|_| {
                    Error::new(
                        Kind::OverflowError,
                        "Python int too large to convert to C ssize_t",
                    )
                })?;
                self.stream()?;
                u64::try_from(signed).map_err(|_| {
                    Error::new(Kind::ValueError, format!("negative seek value {offset}"))
                })
            }
            FileKind::Member => {
                self.check_member("seek on closed file.")?;
                Ok(offset.clamp(0, i128::from(i64::MAX)) as u64)
            }
            _ => {
                self.stream()?;
                disk_offset(offset)
            }
        }
    }

    pub(in crate::objects) fn seek(&mut self, at: SeekFrom) -> Result<u64> {
        if self.kind == FileKind::BinaryRead && self.stream.is_none() {
            return Err(Error::new(Kind::ValueError, "seek of closed file"));
        }
        match self.stream()? {
            Stream::Read(input) => input.seek(at),
            Stream::Memory(m) => m.seek(at),
            Stream::Member(m) => m.seek(at),
            Stream::UnreadGzip { .. } | Stream::Write(_) | Stream::TextMemory(_) => {
                Err(Error::new(Kind::UnsupportedOperation, "seek"))
            }
        }
    }

    pub(in crate::objects) fn read_into(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self.stream()? {
            Stream::Read(input) => input.read_into(buf),
            Stream::UnreadGzip { .. } if buf.is_empty() => Ok(0),
            Stream::UnreadGzip { .. } => self.input()?.read_into(buf),
            Stream::Memory(m) => {
                let n = m.remaining().len().min(buf.len());
                buf[..n].copy_from_slice(&m.remaining()[..n]);
                m.position += n as u64;
                Ok(n)
            }
            Stream::Member(m) => {
                let bytes = m.read(Some(buf.len()))?;
                buf[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
            Stream::Write(_) | Stream::TextMemory(_) => {
                Err(Error::new(Kind::UnsupportedOperation, "read"))
            }
        }
    }
}

/// Convert a Python file offset into the position used by POSIX file operations.
pub(in crate::objects) fn disk_offset(offset: i128) -> Result<u64> {
    let signed = i64::try_from(offset).map_err(|_| {
        Error::new(
            Kind::ValueError,
            "cannot fit 'int' into an offset-sized integer",
        )
    })?;
    u64::try_from(signed)
        .map_err(|_| super::errno_error(io::Error::from_raw_os_error(libc::EINVAL)))
}

/// Construct a BytesIO, sharing its immutable initial bytes until the first write.
///
/// # Safety
/// The optional initial buffer is null or a rooted bytes allocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_io(initial: *const Bytes) -> *const Object {
    let data = if initial.is_null() {
        crate::BytesOwner::new(Vec::new())
    } else {
        // SAFETY: the caller keeps the bytes owner live throughout construction.
        unsafe { crate::acquire_bytes(initial as usize) }
    };
    crate::ObjectOwner::into_raw(FileState::memory(data))
}

/// Reposition the cursor using Python's signed offset and integer whence.
///
/// # Safety
/// The file and canonical integer arguments must remain rooted until return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binary_seek(
    file: *const Object,
    offset: usize,
    whence: usize,
) -> usize {
    // SAFETY: no callbacks can invalidate any of the three borrowed values.
    let result = unsafe {
        let offset = Int::borrow(offset);
        object(file).0.with_mut(|s| {
            let f = s.file_mut();
            f.check_member("seek on closed file.")?;
            if let Some(Stream::Member(member)) = &mut f.stream {
                return member.seek_offset(&offset, Int::borrow(whence).as_small().unwrap_or(-1));
            }
            if f.kind == FileKind::GzipBinaryRead {
                f.stream()?;
            }
            let whence = i64::from(c_int(whence)?);
            let off = match f.kind {
                FileKind::Memory => {
                    let off = offset
                        .as_small()
                        .or_else(|| offset.big().to_i64())
                        .ok_or_else(|| {
                            Error::new(
                                Kind::OverflowError,
                                "Python int too large to convert to C ssize_t",
                            )
                        })?;
                    f.stream()?;
                    off
                }
                _ => {
                    if !(0..=2).contains(&whence) {
                        return Err(Error::new(
                            Kind::ValueError,
                            format!("whence value {whence} unsupported"),
                        ));
                    }
                    if f.stream.is_none() {
                        return Err(Error::new(Kind::ValueError, "seek of closed file"));
                    }
                    offset
                        .as_small()
                        .or_else(|| offset.big().to_i64())
                        .ok_or_else(|| {
                            Error::new(
                                Kind::ValueError,
                                "cannot fit 'int' into an offset-sized integer",
                            )
                        })?
                }
            };
            let at = match whence {
                0 => SeekFrom::Start(f.absolute_offset(i128::from(off))?),
                1 => SeekFrom::Current(off),
                2 => SeekFrom::End(off),
                _ if f.kind == FileKind::Memory => {
                    return Err(Error::new(
                        Kind::ValueError,
                        format!("invalid whence ({whence}, should be 0, 1 or 2)"),
                    ));
                }
                _ => return Err(Error::new(Kind::ValueError, "whence value unsupported")),
            };
            f.seek(at)
        })
    };
    finish(result.map(|n| Int::from_usize(n as usize).into_raw()), 0)
}

/// Returns the logical byte position, accounting for any buffered bytes.
///
/// # Safety
/// The file is a rooted binary stream with no pending error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binary_tell(file: *const Object) -> usize {
    // SAFETY: the position query cannot invoke Python code.
    let result = unsafe {
        object(file).0.with_mut(|s| {
            let f = s.file_mut();
            f.check_member("tell on closed file.")?;
            if f.kind == FileKind::BinaryRead && f.stream.is_none() {
                return Err(Error::new(Kind::ValueError, "I/O operation on closed file"));
            }
            f.seek(SeekFrom::Current(0))
        })
    };
    finish(result.map(|n| Int::from_usize(n as usize).into_raw()), 0)
}

/// Override a ZipExtFile's minimum decompression/read-ahead block.
///
/// # Safety
/// The stream is an exact rooted ZipExtFile; size is a canonical integer word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_min_read_size(file: *const Object, size: usize) {
    // SAFETY: the integer and object remain live for the assignment.
    let result = unsafe {
        object(file).0.with_mut(|s| {
            let f = s.file_mut();
            let size = Int::borrow(size);
            if let Some(Stream::Member(m)) = &mut f.stream {
                m.min_read = super::super::bytes::native_size(&size)?.max(1) as usize;
            }
            Ok(())
        })
    };
    finish(result, ());
}
