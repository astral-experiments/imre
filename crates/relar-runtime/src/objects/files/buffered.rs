//! Buffered binary and UTF-8 streams with Python's read and write boundaries.

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, IsTerminal, Read, Write};

use smallvec::SmallVec;

use super::gzip::{GzipError, GzipReader};
use super::{Bytes, Error, Kind, Result, Str, decode_utf8_text, os_error};

pub(super) struct Output {
    raw: BufWriter<File>,
    text: Option<Vec<u8>>,
    line_buffering: bool,
}

impl Output {
    pub(super) fn new(file: File, capacity: usize, binary: bool) -> Self {
        Self {
            line_buffering: !binary && file.is_terminal(),
            raw: BufWriter::with_capacity(capacity, file),
            text: (!binary).then(Vec::new),
        }
    }

    pub(super) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let Some(pending) = &mut self.text else {
            return self.raw.write_all(bytes);
        };
        let flush_line = self.line_buffering && memchr::memchr2(b'\r', b'\n', bytes).is_some();
        // TextIOWrapper groups small encoded writes, but forwards a large write
        // separately after draining any pending text.
        const CHUNK_SIZE: usize = 8192;
        if bytes.len() >= CHUNK_SIZE {
            Self::drain_text(&mut self.raw, pending)?;
            self.raw.write_all(bytes)?;
        } else {
            pending.extend_from_slice(bytes);
            if pending.len() >= CHUNK_SIZE || flush_line {
                Self::drain_text(&mut self.raw, pending)?;
            }
        }
        if flush_line {
            self.raw.flush()?;
        }
        Ok(())
    }

    fn drain_text(raw: &mut BufWriter<File>, pending: &mut Vec<u8>) -> io::Result<()> {
        let result = raw.write_all(pending);
        // TextIOWrapper removes the pending text even when the binary write fails.
        pending.clear();
        result
    }

    pub(super) fn flush(&mut self) -> io::Result<()> {
        if let Some(pending) = &mut self.text {
            Self::drain_text(&mut self.raw, pending)?;
        }
        self.raw.flush()
    }

    pub(super) fn finish(mut self) -> (File, io::Result<()>) {
        let flushed = self.flush();
        let (file, _) = self.raw.into_parts();
        (file, flushed)
    }
}

enum Source {
    Plain(File),
    Gzip(Box<GzipReader>),
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(file) => file.read(buf),
            Self::Gzip(gzip) => gzip.read(buf).map_err(|err| {
                if err.kind() == io::ErrorKind::UnexpectedEof {
                    io::Error::other(GzipError(
                        Kind::EOFError,
                        "Compressed file ended before the end-of-stream marker was reached",
                    ))
                } else {
                    err
                }
            }),
        }
    }
}

pub(super) struct Input {
    raw: BufReader<Source>,
    text: TextBuffer,
}

impl Input {
    pub(super) fn new(file: File, gzip: bool, binary: bool) -> Self {
        let source = if gzip {
            Source::Gzip(Box::new(GzipReader::new(file)))
        } else {
            Source::Plain(file)
        };
        // TextIOWrapper reads 8192 bytes per decode. CPython 3.14's binary
        // BufferedReader reads 128 KiB ahead, exposing errors in that chunk.
        Self {
            raw: BufReader::with_capacity(if binary { 128 * 1024 } else { 8192 }, source),
            text: TextBuffer::default(),
        }
    }

    pub(super) fn into_file(self) -> File {
        match self.raw.into_inner() {
            Source::Plain(file) => file,
            Source::Gzip(reader) => reader.into_file(),
        }
    }

    pub(super) fn read_bytes(&mut self, size: Option<usize>) -> Result<Bytes> {
        let mut bytes = Vec::new();
        if let Some(limit) = size {
            // A positive buffered read reserves its requested bytes object.
            // The 64-bit CPython bytes header and terminator require 33 bytes.
            if limit > isize::MAX as usize - 33 {
                return Err(Error::new(Kind::OverflowError, "byte string is too large"));
            }
            bytes
                .try_reserve_exact(limit)
                .map_err(|_| Error::new(Kind::MemoryError, "bytes allocation failed"))?;
            let mut bulk = limit;
            if matches!(self.raw.get_ref(), Source::Plain(_)) {
                // Split a large file read at the last whole block so BufReader
                // keeps the surplus bytes from the final partial block.
                let remaining = limit.saturating_sub(self.raw.buffer().len());
                if remaining > self.raw.capacity() {
                    bulk -= remaining % self.raw.capacity();
                }
            }
            let read = self
                .raw
                .by_ref()
                .take(bulk as u64)
                .read_to_end(&mut bytes)
                .map_err(os_error)?;
            if read == bulk && read < limit {
                self.raw
                    .by_ref()
                    .take((limit - read) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(os_error)?;
            }
            // CPython releases the unused part of a short read. Keeping the
            // requested capacity alive with a tiny result exhausts address space.
            if bytes.len() < limit {
                bytes.shrink_to_fit();
            }
        } else {
            self.raw.read_to_end(&mut bytes).map_err(os_error)?;
        }
        Ok(bytes)
    }

    pub(super) fn binary_line(&mut self) -> Result<Bytes> {
        let mut bytes = Vec::new();
        self.raw.read_until(b'\n', &mut bytes).map_err(os_error)?;
        Ok(bytes)
    }

    pub(super) fn read_text(&mut self) -> Result<Str> {
        let mut bytes = self.text.carry.to_vec();
        self.raw.read_to_end(&mut bytes).map_err(os_error)?;
        let remaining = decode_utf8_text(bytes)?;
        self.text.carry.clear();
        let prefix = &self.text.decoded[self.text.pos..];
        if prefix.is_empty() {
            return Ok(remaining);
        }
        let mut result = Str::with_capacity(prefix.len() + remaining.len());
        result.push_str(prefix);
        result.push_str(&remaining);
        self.text.pos = self.text.decoded.len();
        Ok(result)
    }

    pub(super) fn text_line(&mut self) -> Result<Str> {
        let mut line = std::mem::take(&mut self.text.line_scratch);
        line.clear();
        loop {
            let available = &self.text.decoded[self.text.pos..];
            if let Some(newline) = memchr::memchr(b'\n', available.as_bytes()) {
                let len = newline + 1;
                self.text.pos += len;
                if line.is_empty() {
                    self.text.line_scratch = line;
                    return Ok(Str::new(&available[..len]));
                }
                line.push_str(&available[..len]);
                break;
            }
            line.push_str(available);
            self.text.pos = self.text.decoded.len();
            let chunk = self.raw.fill_buf().map_err(os_error)?;
            let length = chunk.len();
            let result = self.text.decode(chunk);
            self.raw.consume(length);
            result?;
            if length == 0 && self.text.decoded.is_empty() {
                break;
            }
        }
        // Keep a bounded buffer for lines that span multiple decoder chunks.
        // An unusually long line transfers its allocation to the caller.
        if line.capacity() > 128 * 1024 {
            Ok(Str::from(line))
        } else {
            let result = Str::new(&line);
            self.text.line_scratch = line;
            Ok(result)
        }
    }
}

#[derive(Default)]
struct TextBuffer {
    decoded: String,
    line_scratch: String,
    pos: usize,
    // A trailing CR and up to three UTF-8 prefix bytes can cross a chunk boundary.
    carry: SmallVec<[u8; 4]>,
}

impl TextBuffer {
    fn decode(&mut self, chunk: &[u8]) -> Result<()> {
        let eof = chunk.is_empty();
        let joined;
        let bytes = if self.carry.is_empty() {
            chunk
        } else {
            joined = [self.carry.as_slice(), chunk].concat();
            &joined
        };
        let text = match str::from_utf8(bytes) {
            Ok(text) => text,
            // CPython's incremental decoder also waits for the third byte of
            // a surrogate encoding before rejecting it, even in strict mode.
            Err(err)
                if !eof
                    && (err.error_len().is_none()
                        || matches!(&bytes[err.valid_up_to()..], [0xed, 0xa0..=0xbf])) =>
            {
                // SAFETY: valid_up_to is the byte length of the validated prefix.
                unsafe { str::from_utf8_unchecked(&bytes[..err.valid_up_to()]) }
            }
            Err(err) => {
                return Err(Error::new(
                    Kind::UnicodeDecodeError,
                    format!("invalid UTF-8 at byte {}", err.valid_up_to()),
                ));
            }
        };
        let mut text = if eof {
            text
        } else {
            text.strip_suffix('\r').unwrap_or(text)
        };
        self.carry.clear();
        self.carry.extend_from_slice(&bytes[text.len()..]);
        self.pos = 0;
        self.decoded.clear();
        while let Some(cr) = memchr::memchr(b'\r', text.as_bytes()) {
            self.decoded.push_str(&text[..cr]);
            self.decoded.push('\n');
            text = &text[cr + 1..];
            text = text.strip_prefix('\n').unwrap_or(text);
        }
        self.decoded.push_str(text);
        Ok(())
    }
}
