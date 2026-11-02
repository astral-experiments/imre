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
    /// Whether truncate is invoking the size object's index protocol on this stream.
    pub(crate) fn random_truncating(&self) -> bool {
        matches!(&self.stream, Some(Stream::Random(file)) if file.truncating_name.is_some())
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

    fn reset(&mut self) {
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
        let current = self.tell()?;
        let target = match at {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(n) => Some(
                current
                    .checked_add_signed(n)
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?,
            ),
            SeekFrom::End(_) => None,
        };
        // A move within read-ahead leaves the kernel offset and any pending writes alone.
        if self.pos < self.read_end
            && let Some(target) = target
        {
            let offset = self.pos as i128 + i128::from(target) - i128::from(current);
            if (0..=self.read_end as i128).contains(&offset) {
                self.pos = offset as usize;
                return Ok(target);
            }
        }
        self.write_back()?;
        let position = self.file.seek(target.map_or(at, SeekFrom::Start))?;
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

    pub(crate) fn capacity(&self) -> usize {
        self.buffer.len()
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
