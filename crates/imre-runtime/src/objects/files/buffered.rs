//! Buffered binary and UTF-8 streams with Python's read and write boundaries.

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Seek, SeekFrom, Write};
use std::mem::MaybeUninit;

use smallvec::SmallVec;

use super::super::codecs::Codec;
use super::gzip::GzipDecoder;
use super::{Bytes, Error, Kind, Result, Str, decode_text, errno_error};
use crate::{BinaryOutput, StrOwner};

struct TextEncoder {
    codec: Codec,
    errors: Option<StrOwner>,
    newline: Option<&'static str>,
}

pub(super) struct Output {
    encoder: Option<TextEncoder>,
    raw: BinaryOutput<File>,
    text: Option<crate::text_output::PendingText>,
    line_buffering: bool,
}

impl Output {
    pub(super) fn new(file: File, capacity: usize, binary: bool) -> Result<Self> {
        Ok(Self {
            encoder: None,
            line_buffering: !binary && file.is_terminal(),
            raw: BinaryOutput::with_buffer(reserve(capacity)?, file),
            text: (!binary).then(crate::text_output::PendingText::default),
        })
    }

    pub(super) fn pipe(file: File, capacity: usize, line_buffering: bool) -> Result<Self> {
        // Popen creates TextIOWrapper(write_through=True) around the binary pipe.
        Ok(Self {
            raw: BinaryOutput::with_buffer(reserve(capacity)?, file),
            text: None,
            encoder: None,
            line_buffering,
        })
    }

    /// Keep a signature pending only when this stream starts at offset zero.
    pub(super) fn set_encoding(
        &mut self,
        mut codec: Codec,
        errors: Option<StrOwner>,
        newline: Option<&str>,
    ) {
        if matches!(codec, Codec::Utf8 { signature: true })
            && self
                .raw
                .inner
                .stream_position()
                .is_ok_and(|position| position != 0)
        {
            codec = Codec::Utf8 { signature: false };
        }
        // A native str already contains the bytes needed by an untranslated UTF-8 writer.
        if matches!(codec, Codec::Utf8 { signature: false })
            && !matches!(newline, Some("\r" | "\r\n"))
        {
            self.encoder = None;
            return;
        }
        self.encoder = Some(TextEncoder {
            codec,
            errors,
            newline: match newline {
                Some("\r") => Some("\r"),
                Some("\r\n") => Some("\r\n"),
                _ => None,
            },
        });
    }

    /// Encode the entire input before changing the file or its text buffer.
    /// An untranslated, buffered UTF-8 file can accept the fragments as one pending write.
    pub(super) fn try_write_parts<'a>(&mut self, parts: impl Iterator<Item = &'a [u8]>) -> bool {
        self.encoder.is_none()
            && !self.line_buffering
            && self
                .text
                .as_mut()
                .is_some_and(|pending| pending.try_append_parts(parts))
    }

    pub(super) fn write_text(
        &mut self,
        text: &str,
        input: impl FnOnce() -> Result<StrOwner>,
    ) -> Result<()> {
        let Some(encoder) = self.encoder.as_mut() else {
            return self.write(text.as_bytes()).map_err(errno_error);
        };
        let translated = encoder
            .newline
            .filter(|_| text.contains('\n'))
            .map(|newline| super::path_text::output_newlines(text, newline))
            .transpose()?;
        let encoded = encoder.codec.encode(
            translated.as_deref().unwrap_or(text),
            encoder.errors.as_deref().map(Str::as_str),
            || match &translated {
                Some(value) => super::super::text::try_owned_string(value),
                None => input(),
            },
        )?;
        if matches!(encoder.codec, Codec::Utf8 { signature: true }) {
            encoder.codec = Codec::Utf8 { signature: false };
        }
        self.write(&encoded).map_err(errno_error)
    }

    pub(super) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let flush_line = self.line_buffering && memchr::memchr2(b'\r', b'\n', bytes).is_some();
        let Some(pending) = &mut self.text else {
            self.raw.write_all(bytes)?;
            return if flush_line { self.raw.flush() } else { Ok(()) };
        };
        pending.write(&mut self.raw, bytes, flush_line)
    }

    pub(super) fn flush(&mut self) -> io::Result<()> {
        if let Some(pending) = &mut self.text {
            return pending.flush(&mut self.raw);
        }
        self.raw.flush()
    }

    /// Include buffered bytes in the binary offset without publishing them to the file.
    pub(super) fn binary_position(&mut self) -> io::Result<u64> {
        let pending = self.raw.buffer.len() - self.raw.written;
        Ok(self.raw.inner.stream_position()? + pending as u64)
    }

    pub(super) fn binary_seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        self.raw.flush()?;
        self.raw.inner.seek(offset)
    }

    pub(super) fn into_file(self) -> File {
        self.raw.inner
    }

    pub(super) fn finish(mut self) -> (File, io::Result<()>) {
        let flushed = self.flush();
        (self.raw.inner, flushed)
    }
}

enum Source {
    Plain(File),
    Gzip(Box<GzipFile>),
}

struct GzipFile {
    file: BufReader<File>,
    decoder: GzipDecoder,
}

impl Source {
    /// Read one raw chunk into the already reserved result, initializing its prefix.
    fn read_uninit(&mut self, buf: &mut [MaybeUninit<u8>]) -> io::Result<usize> {
        match self {
            Self::Plain(file) => rustix::io::retry_on_intr(|| {
                rustix::io::read(&*file, &mut *buf).map(|(initialized, _)| initialized.len())
            })
            .map_err(Into::into),
            Self::Gzip(gzip) => gzip.decoder.read_file(&mut gzip.file, buf),
        }
    }
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: both raw readers only write initialized bytes into the prefix.
        let buf = unsafe { &mut *(buf as *mut [u8] as *mut [MaybeUninit<u8>]) };
        self.read_uninit(buf)
    }
}

fn reserve(capacity: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    data.try_reserve_exact(capacity)
        .map_err(|_| Error::memory_error())?;
    Ok(data)
}

/// Return the current read-ahead bytes, issuing at most one successful raw read.
pub(crate) fn peek_bytes(reader: &mut impl BufRead) -> Result<Bytes> {
    loop {
        match reader.fill_buf() {
            Ok(bytes) => {
                let mut result = reserve(bytes.len())?;
                result.extend_from_slice(bytes);
                return Ok(result);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(Vec::new()),
            Err(error) => return Err(errno_error(error)),
        }
    }
}

// Standard BufReader cannot adopt a fallibly reserved buffer. Keep regular
// files on their existing reader and let a subprocess pipe own its reservation.
enum ReadBuffer {
    File(BufReader<io::Take<Source>>),
    Pipe {
        source: io::Take<Source>,
        bytes: Vec<u8>,
        start: usize,
        window: usize,
    },
}

impl ReadBuffer {
    /// Use File's direct read into spare capacity once the buffer cannot cover a whole file.
    fn read_plain_file(
        &mut self,
        bytes: &mut Vec<u8>,
        size_hint: usize,
    ) -> Option<io::Result<usize>> {
        let Self::File(reader) = self else {
            return None;
        };
        if size_hint <= reader.capacity()
            || !reader.buffer().is_empty()
            || reader.get_ref().limit() != u64::MAX
        {
            return None;
        }
        let source = reader.get_mut();
        let Source::Plain(file) = source.get_mut() else {
            return None;
        };
        let start = bytes.len();
        let result = file.read_to_end(bytes);
        source.set_limit(u64::MAX - (bytes.len() - start) as u64);
        Some(result)
    }

    fn into_inner(self) -> io::Take<Source> {
        match self {
            Self::File(r) => r.into_inner(),
            Self::Pipe { source, .. } => source,
        }
    }

    fn get_mut(&mut self) -> &mut io::Take<Source> {
        match self {
            Self::File(r) => r.get_mut(),
            Self::Pipe { source, .. } => source,
        }
    }
    fn buffer(&self) -> &[u8] {
        match self {
            Self::File(r) => r.buffer(),
            Self::Pipe { bytes, start, .. } => &bytes[*start..],
        }
    }
    fn capacity(&self) -> usize {
        match self {
            Self::File(r) => r.capacity(),
            Self::Pipe { window, .. } => *window,
        }
    }
}

impl BufRead for ReadBuffer {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        match self {
            Self::File(reader) => reader.fill_buf(),
            Self::Pipe {
                source,
                bytes,
                start,
                window,
            } => {
                if *start == bytes.len() {
                    bytes.clear();
                    *start = 0;
                    let available = source.limit().min(*window as u64) as usize;
                    if available != 0 {
                        // A short binary buffer (including unbuffered text) only
                        // needs the larger decoding window when a read occurs.
                        bytes
                            .try_reserve_exact(available)
                            .map_err(|_| io::ErrorKind::OutOfMemory)?;
                        let n = source
                            .get_mut()
                            .read_uninit(&mut bytes.spare_capacity_mut()[..available])?;
                        // SAFETY: read_uninit initialized exactly the first n reserved bytes.
                        unsafe { bytes.set_len(n) };
                        source.set_limit(source.limit() - n as u64);
                    }
                }
                Ok(&bytes[*start..])
            }
        }
    }
    fn consume(&mut self, count: usize) {
        match self {
            Self::File(reader) => reader.consume(count),
            Self::Pipe { bytes, start, .. } => *start += count.min(bytes.len() - *start),
        }
    }
}

impl Read for ReadBuffer {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if let Self::File(reader) = self {
            return reader.read(out);
        }
        if out.is_empty() {
            return Ok(0);
        }
        if self.buffer().is_empty() && out.len() >= self.capacity() {
            return self.get_mut().read(out);
        }
        let count = self.fill_buf()?.read(out)?;
        self.consume(count);
        Ok(count)
    }
}

const TEXT_CHUNK_SIZE: usize = 8192;

pub(super) struct Input {
    raw: ReadBuffer,
    text: TextBuffer,
    unbuffered: bool,
    pub(super) read_size_hint: usize,
}

impl Input {
    pub(super) fn new(file: File, gzip: bool, binary: bool, capacity: usize) -> Self {
        let source = if gzip {
            Source::Gzip(Box::new(GzipFile {
                file: BufReader::with_capacity(capacity, file),
                decoder: GzipDecoder::new(),
            }))
        } else {
            Source::Plain(file)
        };
        // TextIOWrapper reads 8192 bytes per decode. CPython 3.14's binary
        // BufferedReader reads 128 KiB ahead, exposing errors in that chunk.
        Self {
            raw: ReadBuffer::File(BufReader::with_capacity(
                if binary { 128 * 1024 } else { TEXT_CHUNK_SIZE },
                source.take(u64::MAX),
            )),
            text: TextBuffer::default(),
            read_size_hint: 0,
            unbuffered: false,
        }
    }

    pub(super) fn pipe(file: File, binary: bool, capacity: usize) -> Result<Self> {
        let window = if binary {
            capacity.max(1)
        } else {
            TEXT_CHUNK_SIZE
        };
        // Start at the end of the untouched zeroed window. The Python reader
        // can take the allocation, and a native refill can reuse its capacity.
        let bytes = crate::zeroed_io_buffer(capacity).map_err(|_| Error::memory_error())?;
        Ok(Self {
            raw: ReadBuffer::Pipe {
                source: Source::Plain(file).take(u64::MAX),
                start: bytes.len(),
                bytes,
                window,
            },
            text: TextBuffer::default(),
            read_size_hint: 0,
            unbuffered: binary && capacity == 0,
        })
    }

    pub(super) fn into_file(self) -> File {
        match self.raw.into_inner().into_inner() {
            Source::Plain(file) => file,
            Source::Gzip(gzip) => gzip.file.into_inner(),
        }
    }

    pub(super) fn preserve_newlines(&mut self) {
        self.text.preserve_newlines = true;
    }

    /// Exclude read-ahead from the file or decompressor's position.
    pub(super) fn binary_position(&mut self) -> Result<u64> {
        let position = match self.raw.get_mut().get_mut() {
            Source::Plain(file) => file.stream_position().map_err(super::errno_error)?,
            Source::Gzip(gzip) => gzip.decoder.position(),
        };
        Ok(position - self.raw.buffer().len() as u64)
    }

    /// Seek a plain BufferedReader at its logical position, discarding read-ahead
    /// only after a successful move.
    pub(super) fn seek(&mut self, mut at: SeekFrom) -> Result<u64> {
        let buffered = self.raw.buffer().len();
        let Source::Plain(file) = self.raw.get_mut().get_mut() else {
            return Err(Error::new(Kind::UnsupportedOperation, "seek"));
        };
        if let SeekFrom::Current(delta) = at {
            if delta == 0 {
                return self.binary_position();
            }
            let pos = i128::from(file.stream_position().map_err(super::errno_error)?)
                - buffered as i128
                + i128::from(delta);
            at = SeekFrom::Start(
                u64::try_from(pos)
                    .map_err(|_| super::errno_error(io::Error::from_raw_os_error(libc::EINVAL)))?,
            );
        }
        let pos = file.seek(at).map_err(super::errno_error)?;
        self.raw.consume(buffered);
        Ok(pos)
    }

    /// Hand the newly opened pipe's reserved read-ahead storage to its Python reader.
    pub(super) fn take_pipe_buffer(&mut self) -> Result<Bytes> {
        let ReadBuffer::Pipe {
            source,
            bytes,
            start,
            window,
        } = &mut self.raw
        else {
            unreachable!("Popen input has pipe storage")
        };
        if source.limit() == u64::MAX && *start == *window && bytes.len() == *window {
            *start = 0;
            return Ok(std::mem::take(bytes));
        }
        // A second wrapper must not take unread bytes belonging to the first.
        crate::zeroed_io_buffer(*window)
    }

    /// Detach bytes already prefetched by the public buffered pipe before reading its raw fd.
    pub(super) fn take_read_ahead(&mut self) -> Result<Bytes> {
        let buffered = self.raw.buffer();
        let mut bytes = reserve(buffered.len())?;
        bytes.extend_from_slice(buffered);
        self.raw.consume(bytes.len());
        Ok(bytes)
    }

    /// read1 consumes available read-ahead or makes a single raw request without prefetching.
    pub(super) fn read_once(&mut self, size: Option<usize>) -> Result<Bytes> {
        let size = size.unwrap_or(self.raw.capacity());
        let buffered = self.raw.buffer();
        if !buffered.is_empty() {
            let count = size.min(buffered.len());
            let mut data = reserve(count)?;
            data.extend_from_slice(&buffered[..count]);
            self.raw.consume(count);
            return Ok(data);
        }
        if size > crate::MAX_BYTES_LEN {
            return Err(Error::new(Kind::OverflowError, "byte string is too large"));
        }
        let mut data = reserve(size)?;
        if size != 0 {
            loop {
                match self
                    .raw
                    .get_mut()
                    .get_mut()
                    .read_uninit(&mut data.spare_capacity_mut()[..size])
                {
                    Ok(n) => {
                        // SAFETY: the raw read initialized precisely n bytes of spare capacity.
                        unsafe { data.set_len(n) };
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(errno_error(e)),
                }
                break;
            }
        }
        crate::compact_bytes(&mut data);
        Ok(data)
    }

    pub(super) fn peek(&mut self) -> Result<Bytes> {
        peek_bytes(&mut self.raw)
    }

    pub(super) fn read_into(&mut self, bytes: &mut [u8]) -> Result<usize> {
        self.raw.read(bytes).map_err(errno_error)
    }

    pub(super) fn read_bytes(&mut self, size: Option<usize>) -> Result<Bytes> {
        let mut bytes = Vec::new();
        if let Some(limit) = size {
            // A positive buffered read reserves its requested bytes object.
            if limit > crate::MAX_BYTES_LEN {
                return Err(Error::new(Kind::OverflowError, "byte string is too large"));
            }
            bytes
                .try_reserve_exact(limit)
                .map_err(|_| Error::new(Kind::MemoryError, "bytes allocation failed"))?;
            if self.unbuffered {
                let n = self
                    .raw
                    .get_mut()
                    .get_mut()
                    .read_uninit(&mut bytes.spare_capacity_mut()[..limit])
                    .map_err(errno_error)?;
                // SAFETY: read_uninit initialized exactly this many reserved bytes.
                unsafe { bytes.set_len(n) };
            } else {
                self.read_sized(&mut bytes, limit).map_err(errno_error)?;
            }
            // CPython releases the unused part of a short read. Keeping the
            // requested capacity alive with a tiny result exhausts address space.
            if bytes.len() < limit {
                crate::compact_bytes(&mut bytes);
            }
        } else {
            let hint = std::mem::take(&mut self.read_size_hint);
            self.raw
                .read_plain_file(&mut bytes, hint)
                .unwrap_or_else(|| self.raw.read_to_end(&mut bytes))
                .map_err(errno_error)?;
        }
        Ok(bytes)
    }

    /// Match BufferedReader's raw request sizes after short reads, including
    /// gzip member boundaries where looking one byte further can raise an error.
    fn read_sized(&mut self, bytes: &mut Bytes, limit: usize) -> io::Result<()> {
        let buffered = limit.min(self.raw.buffer().len());
        bytes.extend_from_slice(&self.raw.buffer()[..buffered]);
        self.raw.consume(buffered);
        let capacity = self.raw.capacity();
        while limit - bytes.len() >= capacity {
            let start = bytes.len();
            let remaining = limit - start;
            let bulk = remaining - remaining % capacity;
            let count = self
                .raw
                .get_mut()
                .get_mut()
                .read_uninit(&mut bytes.spare_capacity_mut()[..bulk])?;
            if count == 0 {
                return Ok(());
            }
            // SAFETY: the source initialized exactly the reported prefix.
            unsafe { bytes.set_len(start + count) };
        }
        // Fill one final block. Each short raw read reduces the space left in
        // that block, even when all of its returned bytes go straight to the caller.
        let mut room = capacity;
        while bytes.len() < limit {
            self.raw.get_mut().set_limit(room as u64);
            let read = self.raw.fill_buf().map(|chunk| {
                let count = (limit - bytes.len()).min(chunk.len());
                bytes.extend_from_slice(&chunk[..count]);
                (count, chunk.len())
            });
            self.raw.get_mut().set_limit(u64::MAX);
            let (count, filled) = read?;
            if filled == 0 {
                break;
            }
            self.raw.consume(count);
            room -= filled;
        }
        Ok(())
    }

    pub(super) fn binary_line(&mut self) -> Result<Bytes> {
        self.binary_line_limited(None)
    }

    pub(super) fn binary_line_limited(&mut self, size: Option<usize>) -> Result<Bytes> {
        let mut bytes = Vec::new();
        (&mut self.raw)
            .take(size.map_or(u64::MAX, |n| n as u64))
            .read_until(b'\n', &mut bytes)
            .map_err(errno_error)?;
        Ok(bytes)
    }

    pub(super) fn read_text(&mut self) -> Result<Str> {
        let mut bytes = self.text.carry.to_vec();
        // The file can grow or shrink after open. Its size only guides allocation.
        let hint = std::mem::take(&mut self.read_size_hint);
        let _ = bytes.try_reserve_exact(hint);
        self.raw
            .read_plain_file(&mut bytes, hint)
            .unwrap_or_else(|| match &mut self.raw {
                ReadBuffer::File(file) => file.read_to_end(&mut bytes),
                pipe => pipe.read_to_end(&mut bytes),
            })
            .map_err(errno_error)?;
        if bytes.len() < hint / 2 {
            bytes.shrink_to_fit();
        }
        let remaining = decode_text(bytes, self.text.preserve_newlines)?;
        self.text.carry.clear();
        let prefix = &self.text.decoded[self.text.pos..];
        if prefix.is_empty() {
            return Ok(remaining);
        }
        let mut result = Str::try_with_capacity(prefix.len() + remaining.len())
            .map_err(|_| Error::allocation())?;
        result.push_str(prefix);
        result.push_str(&remaining);
        self.text.pos = self.text.decoded.len();
        Ok(result)
    }

    /// Return the next short normalized line and a full in-bounds copy window.
    /// Bytes beyond the newline are padding; the caller exposes only its length.
    #[inline]
    pub(super) fn short_line(&mut self) -> Option<(usize, &[u8; 24])> {
        if self.text.preserve_newlines {
            return None;
        }
        let window = self.text.decoded.as_bytes()[self.text.pos..].first_chunk::<24>()?;
        #[inline]
        fn newline(bytes: &[u8; 8]) -> u64 {
            let word = u64::from_le_bytes(*bytes) ^ 0x0a0a_0a0a_0a0a_0a0a;
            word.wrapping_sub(0x0101_0101_0101_0101) & !word & 0x8080_8080_8080_8080
        }
        let (chunks, _) = window.as_chunks::<8>();
        for (i, chunk) in chunks.iter().enumerate() {
            let matches = newline(chunk);
            if matches != 0 {
                let end = i * 8 + matches.trailing_zeros() as usize / 8 + 1;
                self.text.pos += end;
                return Some((end, window));
            }
        }
        None
    }

    pub(super) fn text_line(&mut self) -> Result<Str> {
        if let Some((len, window)) = self.short_line() {
            // SAFETY: the decoded UTF-8 prefix ends just after an ASCII newline.
            return Ok(Str::new(unsafe {
                str::from_utf8_unchecked(&window[..len])
            }));
        }
        self.map_text_line(|text| match text {
            Cow::Borrowed(line) => Str::new(line),
            Cow::Owned(line) => Str::from(line),
        })
    }

    /// Consume the decoded line before its buffer advances. A long line
    /// crossing multiple decoder windows can transfer its allocation.
    pub(super) fn map_text_line<T>(
        &mut self,
        consume: impl FnOnce(Cow<'_, str>) -> T,
    ) -> Result<T> {
        let mut line = std::mem::take(&mut self.text.line_scratch);
        line.clear();
        loop {
            let available = &self.text.decoded[self.text.pos..];
            let bytes = available.as_bytes();
            let line_end = if self.text.preserve_newlines {
                // The decoder holds a trailing CR until the next chunk or EOF,
                // so an available CRLF always belongs to this same line.
                memchr::memchr2(b'\r', b'\n', bytes).map(|i| {
                    i + 1 + usize::from(bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n'))
                })
            } else {
                memchr::memchr(b'\n', bytes).map(|i| i + 1)
            };
            if let Some(len) = line_end {
                self.text.pos += len;
                if line.is_empty() {
                    self.text.line_scratch = line;
                    return Ok(consume(Cow::Borrowed(&available[..len])));
                }
                line.push_str(&available[..len]);
                break;
            }
            line.push_str(available);
            self.text.pos = self.text.decoded.len();
            let chunk = self.raw.fill_buf().map_err(errno_error)?;
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
            Ok(consume(Cow::Owned(line)))
        } else {
            let result = consume(Cow::Borrowed(&line));
            self.text.line_scratch = line;
            Ok(result)
        }
    }
}

/// Decode the same windows as a TextIOWrapper being iterated by line. Strict
/// exceptions retain the bytes and offsets from the failing decoder call.
pub(in crate::objects) fn decode_text_lines(bytes: &[u8], codec: Codec) -> Result<String> {
    let mut text = String::new();
    let mut chunks = bytes.chunks(TEXT_CHUNK_SIZE);
    if let Codec::Utf8 { signature } = codec {
        let mut decoder = TextBuffer::default();
        let mut first = chunks.next().unwrap_or_default();
        if signature {
            let bom = b"\xef\xbb\xbf";
            if bom.starts_with(first) {
                // The signature decoder consumes even an incomplete BOM at EOF.
                return Ok(text);
            }
            first = first.strip_prefix(bom).unwrap_or(first);
        }
        for chunk in std::iter::once(first).chain(chunks).chain([&[][..]]) {
            decoder.decode(chunk)?;
            text.try_reserve(decoder.decoded.len())
                .map_err(|_| Error::allocation())?;
            text.push_str(&decoder.decoded);
        }
        Ok(text)
    } else if matches!(codec, Codec::DoubleByte(_) | Codec::Wide(_)) {
        let mut start = 0;
        let mut end = 0;
        for chunk in chunks.chain([&[][..]]) {
            end += chunk.len();
            let input = &bytes[start..end];
            start += match codec {
                Codec::Wide(format) => {
                    format.decode_text(&mut text, input, None, chunk.is_empty())?
                }
                Codec::DoubleByte(format) => {
                    format.decode_chunk(&mut text, input, None, chunk.is_empty())?
                }
                _ => unreachable!(),
            };
        }
        Ok(super::super::codecs::universal_newlines(text))
    } else {
        for chunk in chunks {
            let decoded = codec.decode(chunk, None)?;
            text.try_reserve(decoded.len())
                .map_err(|_| Error::allocation())?;
            text.push_str(&decoded);
        }
        Ok(super::super::codecs::universal_newlines(text))
    }
}

#[derive(Default)]
struct TextBuffer {
    preserve_newlines: bool,
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
                // A decoded CR was retained for newline translation, independently
                // of the UTF-8 decoder's incomplete bytes from the previous chunk.
                let (source, error) = if self.carry.starts_with(b"\r") {
                    let source = &bytes[1..];
                    (source, str::from_utf8(source).unwrap_err())
                } else {
                    (bytes, err)
                };
                return Err(super::super::bytes::decode_error(source, error));
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
        if self.preserve_newlines {
            self.decoded.push_str(text);
            return Ok(());
        }
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
