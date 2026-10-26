//! Buffered read/write windows on anonymous seekable binary files.

use std::fs::File;
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::ops::Range;

use super::super::exceptions::Result;
use super::{Bytes, Error, close_file, errno_error};

pub(super) struct Random {
    file: File,
    buffer: Vec<u8>,
    // Kernel and Python positions relative to the same window. The descriptor
    // can also move through fileno(), so an absolute starting offset can go stale.
    raw_pos: usize,
    pos: usize,
    read_end: usize,
    dirty: Option<Range<usize>>,
}

impl Random {
    pub(super) fn new(file: File, capacity: usize) -> Result<Self> {
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
        })
    }

    pub(super) fn tell(&mut self) -> io::Result<u64> {
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
    pub(super) fn flush(&mut self) -> io::Result<()> {
        self.write_back()?;
        if self.raw_pos != self.pos {
            self.file
                .seek(SeekFrom::Current(self.pos as i64 - self.raw_pos as i64))?;
        }
        self.reset();
        Ok(())
    }

    pub(super) fn close(mut self) -> io::Result<()> {
        let flushed = self.flush();
        close_file(self.file).and(flushed)
    }

    pub(super) fn seek(&mut self, at: SeekFrom) -> io::Result<u64> {
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

    pub(super) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
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

    pub(super) fn read_bytes(&mut self, limit: Option<usize>) -> Result<Bytes> {
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

    pub(super) fn readline(&mut self) -> Result<Bytes> {
        let mut result = Vec::new();
        self.read_until(b'\n', &mut result).map_err(errno_error)?;
        Ok(result)
    }
}

impl BufRead for Random {
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

impl Read for Random {
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
