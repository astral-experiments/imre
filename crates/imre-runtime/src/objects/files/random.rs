//! Buffered read/write windows on seekable binary files.

use std::fs::File;
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::ops::Range;

use super::super::exceptions::Result;
use super::{Bytes, Error, FileState, Kind, Stream, close_file, errno_error};

pub(crate) struct Random<F = File> {
    file: F,
    buffer: Vec<u8>,
    // Kernel and Python positions relative to the same window. The descriptor
    // can also move through fileno(), so an absolute starting offset can go stale.
    raw_pos: usize,
    pos: usize,
    read_end: usize,
    dirty: Option<Range<usize>>,
    truncating_name: Option<String>,
}

impl FileState {
    pub(crate) fn random_peek(&mut self) -> Result<Bytes> {
        self.check_random_reentry()?;
        let Some(Stream::Random(file)) = &mut self.stream else {
            return Err(Error::new(Kind::ValueError, "peek of closed file"));
        };
        file.peek()
    }

    /// Return buffered bytes or make one read through the retained anonymous file.
    pub(crate) fn random_read_once(&mut self, size: Option<usize>) -> Result<Bytes> {
        self.check_random_reentry()?;
        let Some(Stream::Random(file)) = &mut self.stream else {
            return Err(Error::new(Kind::ValueError, "read of closed file"));
        };
        let size = file.read_once_size(size);
        let mut data = Vec::new();
        data.try_reserve_exact(size)
            .map_err(|_| Error::allocation())?;
        data.resize(size, 0);
        let count = file.read_once(&mut data).map_err(errno_error)?;
        data.truncate(count);
        Ok(data)
    }

    /// A size object's index method can call back into the same buffered stream.
    pub(crate) fn check_random_reentry(&self) -> Result<()> {
        if let Some(Stream::Random(file)) = &self.stream
            && let Some(name) = &file.truncating_name
        {
            return Err(Error::new(
                Kind::RuntimeError,
                format!("reentrant call inside <_io.BufferedRandom name={name}>"),
            ));
        }
        Ok(())
    }

    /// Flush and rewind to the logical cursor before converting the optional Python size.
    pub(crate) fn begin_random_truncate(&mut self, name: Option<String>) -> Result<()> {
        let Some(Stream::Random(file)) = &mut self.stream else {
            return Err(Error::new(Kind::ValueError, "truncate of closed file"));
        };
        file.flush().map_err(errno_error)?;
        file.truncating_name = name;
        Ok(())
    }

    /// Release the reentrancy guard even if size conversion failed. A resize preserves the cursor.
    pub(crate) fn finish_random_truncate(&mut self, size: Option<i64>) -> Result<()> {
        let Some(Stream::Random(file)) = &mut self.stream else {
            return Err(Error::new(Kind::ValueError, "truncate of closed file"));
        };
        file.truncating_name = None;
        if let Some(size) = size {
            let size = u64::try_from(size)
                .map_err(|_| errno_error(io::Error::from_raw_os_error(libc::EINVAL)))?;
            file.file.set_len(size).map_err(errno_error)?;
        }
        Ok(())
    }
}

impl<F: Read + Write + Seek> Random<F> {
    pub(crate) fn new(file: F, capacity: usize) -> Result<Self> {
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(capacity)
            .map_err(|_| Error::allocation())?;
        buffer.resize(capacity, 0);
        Ok(Self {
            file,
            buffer,
            raw_pos: 0,
            pos: 0,
            read_end: 0,
            dirty: None,
            truncating_name: None,
        })
    }

    pub(crate) fn tell(&mut self) -> io::Result<u64> {
        let raw = self.file.stream_position()?;
        Ok(raw.saturating_add_signed(self.pos as i64 - self.raw_pos as i64))
    }

    pub(crate) fn reset(&mut self) {
        self.raw_pos = 0;
        self.pos = 0;
        self.read_end = 0;
        self.dirty = None;
    }

    fn write_back(&mut self) -> io::Result<()> {
        if let Some(span) = &mut self.dirty {
            if self.raw_pos != span.start {
                self.file
                    .seek(SeekFrom::Current(span.start as i64 - self.raw_pos as i64))?;
                self.raw_pos = span.start;
            }
            while span.start < span.end {
                match self.file.write(&self.buffer[span.clone()]) {
                    Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                    Ok(count) => {
                        span.start += count;
                        self.raw_pos += count;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            self.dirty = None;
        }
        Ok(())
    }

    /// Flush pending edits and leave the descriptor at the caller's logical position.
    pub(crate) fn flush(&mut self) -> io::Result<()> {
        self.write_back()?;
        if self.raw_pos != self.pos {
            self.file
                .seek(SeekFrom::Current(self.pos as i64 - self.raw_pos as i64))?;
        }
        self.reset();
        Ok(())
    }

    pub(crate) fn seek(&mut self, at: SeekFrom) -> io::Result<u64> {
        // A move within read-ahead leaves the descriptor and pending writes alone.
        if self.pos < self.read_end {
            let raw = self.file.stream_position()?;
            let start = i128::from(raw) - self.raw_pos as i128;
            let pos = match at {
                SeekFrom::Start(n) => Some(i128::from(n) - start),
                SeekFrom::Current(n) => Some(self.pos as i128 + i128::from(n)),
                SeekFrom::End(_) => None,
            };
            if let Some(pos) = pos.filter(|pos| (0..=self.read_end as i128).contains(pos)) {
                self.pos = pos as usize;
                return Ok(raw.saturating_add_signed(self.pos as i64 - self.raw_pos as i64));
            }
        }
        self.seek_raw(|raw, adjustment| {
            let at = match at {
                SeekFrom::Current(n) => SeekFrom::Current(
                    n.checked_add(adjustment)
                        .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?,
                ),
                other => other,
            };
            raw.seek(at)
        })
    }

    /// A failed raw seek leaves unread bytes available at the logical cursor.
    pub(crate) fn seek_raw(
        &mut self,
        seek: impl FnOnce(&mut F, i64) -> io::Result<u64>,
    ) -> io::Result<u64> {
        self.write_back()?;
        // Appending may have moved the descriptor while writing the dirty bytes.
        let adjustment = self.pos as i64 - self.raw_pos as i64;
        let position = seek(&mut self.file, adjustment)?;
        self.reset();
        Ok(position)
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > self.buffer.len() - self.pos {
            self.flush()?;
            if bytes.len() > self.buffer.len() {
                self.file.write_all(bytes)?;
                return Ok(());
            }
        }
        let end = self.pos + bytes.len();
        self.buffer[self.pos..end].copy_from_slice(bytes);
        let span = self.dirty.get_or_insert(self.pos..end);
        span.start = span.start.min(self.pos);
        span.end = span.end.max(end);
        self.pos = end;
        if self.read_end != 0 {
            self.read_end = self.read_end.max(end);
        }
        Ok(())
    }

    pub(crate) fn read_bytes(&mut self, limit: Option<usize>) -> Result<Bytes> {
        let mut result = Vec::new();
        match limit {
            Some(n) => self.take(n as u64).read_to_end(&mut result),
            None => self.read_to_end(&mut result),
        }
        .map_err(errno_error)?;
        Ok(result)
    }

    pub(super) fn read_into(&mut self, dest: &mut [u8]) -> Result<usize> {
        let mut offset = 0;
        while offset < dest.len() {
            let n = self.read(&mut dest[offset..]).map_err(errno_error)?;
            if n == 0 {
                break;
            }
            offset += n;
        }
        Ok(offset)
    }

    /// BufferedRandom synchronizes pending writes and refills from its logical cursor.
    pub(crate) fn peek(&mut self) -> Result<Bytes> {
        self.flush().map_err(errno_error)?;
        super::peek_bytes(self)
    }

    /// A read1 with read-ahead returns only the buffered prefix, regardless of the limit.
    pub(crate) fn read_once_size(&self, limit: Option<usize>) -> usize {
        let size = limit.unwrap_or(self.buffer.len());
        if self.pos < self.read_end {
            size.min(self.read_end - self.pos)
        } else {
            size
        }
    }

    /// Read buffered data or issue a single raw read without reading ahead.
    pub(crate) fn read_once(&mut self, dest: &mut [u8]) -> io::Result<usize> {
        if dest.is_empty() {
            return Ok(0);
        }
        if self.pos < self.read_end {
            let count = dest.len().min(self.read_end - self.pos);
            dest[..count].copy_from_slice(&self.buffer[self.pos..self.pos + count]);
            self.pos += count;
            Ok(count)
        } else {
            self.flush()?;
            self.file.read(dest)
        }
    }

    pub(super) fn readline(&mut self) -> Result<Bytes> {
        let mut result = Vec::new();
        self.read_until(b'\n', &mut result).map_err(errno_error)?;
        Ok(result)
    }
}

impl Random<File> {
    pub(super) fn close(mut self) -> io::Result<()> {
        let flushed = self.flush();
        close_file(self.file).and(flushed)
    }
}

impl<F: Read + Write + Seek> BufRead for Random<F> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos >= self.read_end {
            self.flush()?;
            self.read_end = self.file.read(&mut self.buffer)?;
            self.raw_pos = self.read_end;
        }
        Ok(&self.buffer[self.pos..self.read_end])
    }

    fn consume(&mut self, count: usize) {
        self.pos = (self.pos + count).min(self.read_end);
    }
}

impl<F: Read + Write + Seek> Read for Random<F> {
    fn read(&mut self, dest: &mut [u8]) -> io::Result<usize> {
        if dest.is_empty() {
            return Ok(0);
        }
        if self.pos >= self.read_end && dest.len() >= self.buffer.len() {
            self.flush()?;
            let direct = dest.len() / self.buffer.len() * self.buffer.len();
            return self.file.read(&mut dest[..direct]);
        }
        let available = self.fill_buf()?;
        let count = dest.len().min(available.len());
        dest[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}
