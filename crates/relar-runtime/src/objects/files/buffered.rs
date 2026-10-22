//! A shared cursor for whole-file, bounded-byte, and incremental line reads.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};

use smallvec::SmallVec;

use super::gzip::{GzipError, GzipReader};
use super::{Bytes, Error, Kind, Result, Str, decode_utf8_text, os_error};

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
            self.raw
                .by_ref()
                .take(limit as u64)
                .read_to_end(&mut bytes)
                .map_err(os_error)?;
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
        let mut line = Str::default();
        loop {
            let available = &self.text.decoded[self.text.pos..];
            if let Some(newline) = memchr::memchr(b'\n', available.as_bytes()) {
                let len = newline + 1;
                line.push_str(&available[..len]);
                self.text.pos += len;
                return Ok(line);
            }
            line.push_str(available);
            self.text.pos = self.text.decoded.len();
            let chunk = self.raw.fill_buf().map_err(os_error)?;
            let length = chunk.len();
            let result = self.text.decode(chunk);
            self.raw.consume(length);
            result?;
            if length == 0 && self.text.decoded.is_empty() {
                return Ok(line);
            }
        }
    }
}

#[derive(Default)]
struct TextBuffer {
    decoded: String,
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
            Err(err) if err.error_len().is_none() && !eof => {
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
