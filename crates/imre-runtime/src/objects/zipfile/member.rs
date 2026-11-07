//! Independent member cursors with incremental CRC.
use crate::StrOwner;
use flate2::{Crc, Decompress, FlushDecompress, Status};
use num_bigint::Sign;
use num_traits::ToPrimitive;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use zlib_rs::{Inflate, InflateFlush, Status as InflateStatus};

use super::super::{
    Bytes, Str, bytes,
    exceptions::{Error, Kind, Result, string_repr},
    files,
};
use super::{
    Info,
    directory::{bad, grow, name, u16_at},
    source::Source,
};
use crate::{Int, zstd_streams};

const BLOCK: usize = 64 * 1024;

/// ZIP method 8 uses raw DEFLATE with a 32 KiB window.
fn raw_inflater() -> Inflate {
    Inflate::new(false, 15)
}

fn deflate_error(message: &str) -> Error {
    Error::new(
        Kind::ZlibError,
        format!("Error -3 while decompressing data: {message}"),
    )
}

/// Read exactly the cached compressed byte count using the current local header.
pub(super) fn read_import_data(path: &str, info: &Info) -> Result<Bytes> {
    let (mut file, _) = files::open_file(path, 0)?;
    let seek_error = |_| {
        Error::new(
            Kind::ImportError,
            format!("can't read Zip file: {}", string_repr(path)),
        )
    };
    file.seek(SeekFrom::Start(files::disk_offset(info.offset)?))
        .map_err(seek_error)?;
    let mut header = [0; 30];
    file.read_exact(&mut header).map_err(|err| {
        if err.kind() == ErrorKind::UnexpectedEof {
            Error::new(Kind::EOFError, "EOF read where not expected")
        } else {
            files::errno_error(err)
        }
    })?;
    if !header.starts_with(b"PK\x03\x04") {
        return Err(Error::new(
            Kind::ImportError,
            format!("bad local file header: {}", string_repr(path)),
        ));
    }
    let skip = i64::from(u16_at(&header, 26)) + i64::from(u16_at(&header, 28));
    file.seek(SeekFrom::Current(skip)).map_err(seek_error)?;
    let mut raw = Vec::new();
    file.take(info.compress_size)
        .read_to_end(&mut raw)
        .map_err(files::errno_error)?;
    if raw.len() as u64 != info.compress_size {
        return Err(Error::new(Kind::OSError, "zipimport: can't read data"));
    }
    if info.method == 0 {
        return Ok(raw);
    }

    let mut inflater = Decompress::new(false);
    let mut input = raw.as_slice();
    let mut result = Vec::new();
    let mut chunk = [0; 16 * 1024];
    loop {
        let before_in = inflater.total_in();
        let before_out = inflater.total_out();
        let status = inflater
            .decompress(input, &mut chunk, FlushDecompress::None)
            .map_err(|err| deflate_error(err.message().unwrap_or("invalid compressed data")))?;
        let used = (inflater.total_in() - before_in) as usize;
        let written = (inflater.total_out() - before_out) as usize;
        result
            .try_reserve(written)
            .map_err(|_| Error::memory_error_with_message("Unable to allocate output buffer."))?;
        result.extend_from_slice(&chunk[..written]);
        if status == Status::StreamEnd {
            return Ok(result);
        }
        if used == 0 && written == 0 {
            return Err(Error::new(
                Kind::ZlibError,
                "Error -5 while decompressing data: incomplete or truncated stream",
            ));
        }
        input = &input[used..];
    }
}

enum Decoder {
    Stored,
    Deflate(Inflate),
    Zstandard(zstd_streams::Decompressor),
}

impl Decoder {
    fn new(method: u16) -> Result<Self> {
        match method {
            0 => Ok(Self::Stored),
            8 => Ok(Self::Deflate(raw_inflater())),
            93 => Ok(Self::Zstandard(zstd_streams::Decompressor::new()?)),
            12 | 14 => {
                let module = if method == 12 { "bz2" } else { "lzma" };
                Err(Error::new(
                    Kind::RuntimeError,
                    format!("Compression requires the (missing) {module} module"),
                ))
            }
            _ => Err(Error::new(
                Kind::NotImplementedError,
                "That compression method is not supported",
            )),
        }
    }
}

pub(in crate::objects) struct Member {
    source: Source,
    info: crate::Shared<Info>,
    start: u64,
    packed_at: u64,
    packed_left: u64,
    left: u64,
    decoder: Decoder,
    packed: Bytes,
    packed_pos: usize,
    buffer: Bytes,
    pos: usize,
    eof: bool,
    crc: Option<Crc>,
    pub(in crate::objects) min_read: usize,
}

impl Member {
    pub(super) fn open(
        source: Source,
        info: crate::Shared<Info>,
        encrypted_name: impl FnOnce() -> Str,
    ) -> Result<Self> {
        let at = source.absolute_offset(info.offset)?;
        let mut header = [0; 30];
        if source.read(at, &mut header)? != 30 {
            return Err(bad("Truncated file header"));
        }
        if !header.starts_with(b"PK\x03\x04") {
            return Err(bad("Bad magic number for file header"));
        }
        let mut raw = Vec::new();
        grow(&mut raw, usize::from(u16_at(&header, 26)))?;
        let after_header = at + 30;
        let n = source.read(after_header, &mut raw)?;
        raw.truncate(n);
        let start = after_header + n as u64 + u64::from(u16_at(&header, 28));
        if u16_at(&header, 28) != 0 {
            source.position(start)?;
        }
        if info.flags & (1 << 5) != 0 {
            return Err(Error::new(
                Kind::NotImplementedError,
                "compressed patched data (flag bit 5)",
            ));
        }
        if info.flags & (1 << 6) != 0 {
            return Err(Error::new(
                Kind::NotImplementedError,
                "strong encryption (flag bit 6)",
            ));
        }
        if name(&raw, u16_at(&header, 6) & (1 << 11) != 0)? != *info.original {
            let mut repr = String::new();
            bytes::write_repr(&mut repr, &raw);
            return Err(bad(format!(
                "File name in directory {} and header {} differ.",
                string_repr(&info.original),
                repr
            )));
        }
        if i128::from(start) + i128::from(info.compress_size) > info.end && info.end != info.offset
        {
            return Err(bad(format!(
                "Overlapped entries: {} (possible zip bomb)",
                string_repr(&info.original)
            )));
        }
        if info.flags & 1 != 0 {
            return Err(Error::new(
                Kind::RuntimeError,
                format!(
                    "File {} is encrypted, password required for extraction",
                    encrypted_name()
                ),
            ));
        }
        Ok(Self {
            source,
            start,
            packed_at: start,
            packed_left: info.compress_size,
            left: info.file_size,
            decoder: Decoder::new(info.method)?,
            info,
            packed: Vec::new(),
            packed_pos: 0,
            buffer: Vec::new(),
            pos: 0,
            eof: false,
            crc: Some(Crc::new()),
            min_read: 4096,
        })
    }

    pub(in crate::objects) fn name(&self) -> &StrOwner {
        &self.info.filename
    }

    /// Each cursor supplies its own file position so another reader on the
    /// same source may run between calls.
    fn input(&mut self, n: usize, limit: usize) -> Result<()> {
        let pending = self.packed.len() - self.packed_pos;
        if self.packed_left == 0 || n <= pending {
            return Ok(());
        }
        self.packed.copy_within(self.packed_pos.., 0);
        self.packed.truncate(pending);
        self.packed_pos = 0;
        let request = (n - pending)
            .max(self.min_read)
            .min(limit)
            .min(self.packed_left as usize);
        self.packed
            .try_reserve(request)
            .map_err(|_| Error::allocation())?;
        self.packed.resize(pending + request, 0);
        let count = match self
            .source
            .read(self.packed_at, &mut self.packed[pending..])
        {
            Ok(count) => count,
            Err(error) => {
                self.packed.truncate(pending);
                return Err(error);
            }
        };
        self.packed.truncate(pending + count);
        if count == 0 {
            return Err(Error::new(Kind::EOFError, ""));
        }
        self.packed_at += count as u64;
        self.packed_left -= count as u64;
        Ok(())
    }

    /// Refill only when needed and validate at logical EOF, including any data
    /// read ahead for small reads (the observable point at which bad CRC raises).
    fn refill(&mut self, want: usize) -> Result<()> {
        self.buffer.clear();
        self.pos = 0;
        self.read_block(want).inspect_err(|_| self.buffer.clear())
    }

    /// Fill the member buffer with one decoded block and advance its logical size.
    fn read_block(&mut self, want: usize) -> Result<()> {
        // ZipExtFile reads the full requested Zstandard input before invoking
        // the codec, even if a bad frame or checksum then makes that read fail.
        let input_limit = if matches!(self.decoder, Decoder::Zstandard(_)) {
            usize::MAX
        } else {
            BLOCK
        };
        while !self.eof && self.buffer.is_empty() {
            self.input(want.min(input_limit), input_limit)?;
            if let Decoder::Deflate(inflater) = &mut self.decoder {
                let len = want.max(self.min_read).min(BLOCK);
                // A small member may produce only a few bytes of a full read buffer.
                self.buffer
                    .try_reserve(len)
                    .map_err(|_| Error::allocation())?;
                let before_in = inflater.total_in();
                let before_out = inflater.total_out();
                let result = inflater.decompress_uninit(
                    &self.packed[self.packed_pos..],
                    &mut self.buffer.spare_capacity_mut()[..len],
                    InflateFlush::NoFlush,
                );
                let consumed = (inflater.total_in() - before_in) as usize;
                self.packed_pos += consumed;
                let status = match result {
                    Ok(status) => status,
                    Err(_) => {
                        let message = inflater
                            .error_message()
                            .unwrap_or("invalid compressed data");
                        let diagnostic = if message == "repeated call with bad state" {
                            self.diagnose_deflate()
                        } else {
                            None
                        };
                        return Err(deflate_error(diagnostic.as_deref().unwrap_or(message)));
                    }
                };
                let written = (inflater.total_out() - before_out) as usize;
                // SAFETY: the buffer was empty and the inflater initialized exactly
                // the prefix counted by the change in total_out.
                unsafe { self.buffer.set_len(written) };
                self.eof = status == InflateStatus::StreamEnd
                    || (self.packed_left == 0 && self.packed_pos == self.packed.len());
                if self.eof && status != InflateStatus::StreamEnd && written == len {
                    // CPython flushes as soon as the last compressed byte has
                    // been consumed. A pending distance run can cross the read
                    // boundary, so the final CRC may raise on this very read.
                    let mut last = [0; 1024];
                    loop {
                        let before = inflater.total_out();
                        let status = inflater
                            .decompress(&[], &mut last, InflateFlush::Finish)
                            .map_err(|_| {
                                deflate_error(
                                    inflater
                                        .error_message()
                                        .unwrap_or("invalid compressed data"),
                                )
                            })?;
                        let added = (inflater.total_out() - before) as usize;
                        self.buffer
                            .try_reserve(added)
                            .map_err(|_| Error::allocation())?;
                        self.buffer.extend_from_slice(&last[..added]);
                        if added < last.len() || status == InflateStatus::StreamEnd {
                            break;
                        }
                    }
                }
                if consumed == 0 && written == 0 && !self.eof {
                    return Err(Error::new(
                        Kind::ZlibError,
                        "Error -3 while decompressing data: invalid compressed data",
                    ));
                }
            } else if let Decoder::Zstandard(stream) = &mut self.decoder {
                // ZipExtFile decodes the entire compressed input chunk before
                // returning a small read, including its EOF and CRC effects.
                let decoded = stream.decompress(&self.packed, self.left as usize);
                self.packed.clear();
                self.buffer = decoded?;
                // A member can declare a smaller size than its frame expands
                // to. Validate the rest of this input without retaining it.
                while !stream.eof && !stream.needs_input {
                    stream.decompress(&[], BLOCK)?;
                }
                self.eof = stream.eof || self.packed_left == 0;
            } else {
                std::mem::swap(&mut self.buffer, &mut self.packed);
                self.packed.clear();
                self.packed_pos = 0;
                self.eof = self.packed_left == 0;
            }
            self.buffer.truncate(self.left as usize);
            self.left -= self.buffer.len() as u64;
            self.eof |= self.left == 0;
            if let Some(crc) = &mut self.crc {
                crc.update(&self.buffer);
                if self.eof && crc.sum() != self.info.crc {
                    return Err(bad(format!(
                        "Bad CRC-32 for file {}",
                        string_repr(&self.info.filename)
                    )));
                }
            }
        }
        Ok(())
    }

    /// zlib-rs's fast decoder can replace the original corrupt-code diagnostic
    /// with a generic bad-state message. On that error only, replay with an
    /// output window shorter than a DEFLATE match to recover the real message.
    #[cold]
    fn diagnose_deflate(&self) -> Option<String> {
        let mut inflater = Decompress::new(false);
        let mut at = self.start;
        let mut input = [0; 4096];
        let mut output = [0; 256];
        let result = (|| {
            while at < self.packed_at {
                let n = input.len().min((self.packed_at - at) as usize);
                let read = self.source.read(at, &mut input[..n]).ok()?;
                if read == 0 {
                    return None;
                }
                at += read as u64;
                let mut pos = 0;
                while pos < read {
                    let before = inflater.total_in();
                    let produced = inflater.total_out();
                    match inflater.decompress(&input[pos..read], &mut output, FlushDecompress::None)
                    {
                        Err(err) => return err.message().map(str::to_owned),
                        Ok(Status::StreamEnd) => return None,
                        _ => {}
                    }
                    let consumed = (inflater.total_in() - before) as usize;
                    if consumed == 0 && produced == inflater.total_out() {
                        return None;
                    }
                    pos += consumed;
                }
            }
            None
        })();
        let _ = self.source.position(self.packed_at);
        result
    }

    /// ZipExtFile reads compressed input before zlib checks the output limit.
    /// Discard those new bytes in bounded blocks, keeping the inflater's existing tail.
    #[cold]
    fn oversized_deflate_read(&mut self) -> Result<Bytes> {
        self.buffer.clear();
        self.pos = 0;
        let pending = self.packed.len() - self.packed_pos;
        while self.packed_left != 0 {
            let before = self.packed_left;
            let read = self.input(usize::MAX, BLOCK);
            self.packed.truncate(pending);
            read?;
            if before - self.packed_left < before.min(BLOCK as u64) {
                break;
            }
        }
        Err(Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C ssize_t",
        ))
    }

    pub(in crate::objects) fn read(&mut self, size: Option<usize>) -> Result<Bytes> {
        let mut out = Vec::new();
        let limit = size.unwrap_or(usize::MAX);
        while out.len() < limit {
            if self.pos == self.buffer.len() {
                if self.eof {
                    break;
                }
                let remaining = limit - out.len();
                if size.is_some()
                    && remaining > isize::MAX as usize
                    && matches!(self.decoder, Decoder::Deflate(_))
                {
                    return self.oversized_deflate_read();
                }
                self.refill(size.map_or(i32::MAX as usize, |_| remaining))?;
                if self.buffer.is_empty() {
                    break;
                }
            }
            let n = (limit - out.len()).min(self.buffer.len() - self.pos);
            out.try_reserve(n).map_err(|_| Error::allocation())?;
            out.extend_from_slice(&self.buffer[self.pos..self.pos + n]);
            self.pos += n;
        }
        if self.pos == self.buffer.len() {
            self.buffer.clear();
            self.pos = 0;
        }
        Ok(out)
    }

    pub(in crate::objects) fn readline(&mut self) -> Result<Bytes> {
        let mut out = Vec::new();
        loop {
            let cached = out.is_empty() && self.pos < self.buffer.len();
            if self.pos == self.buffer.len() {
                if self.eof {
                    // The buffered readline fallback reads past the cached newline.
                    self.buffer.clear();
                    self.pos = 0;
                    break;
                }
                self.refill(1)?;
            }
            let data = &self.buffer[self.pos..];
            let line = memchr::memchr(b'\n', data).map(|i| i + 1);
            let n = line.unwrap_or(data.len());
            out.try_reserve(n).map_err(|_| Error::allocation())?;
            out.extend_from_slice(&data[..n]);
            self.pos += n;
            // ZipExtFile's newline shortcut retains its existing read buffer.
            // The buffered readline fallback consumes chunks through read().
            if self.pos == self.buffer.len() && !(cached && line.is_some()) {
                self.buffer.clear();
                self.pos = 0;
            }
            if line.is_some() || n == 0 {
                break;
            }
        }
        Ok(out)
    }

    fn tell(&self) -> u64 {
        self.info.file_size - self.left - (self.buffer.len() - self.pos) as u64
    }

    /// Resolve a Python seek relative to the member before clamping its position.
    pub(in crate::objects) fn seek_offset(&mut self, offset: &Int, whence: i64) -> Result<u64> {
        let base = match whence {
            0 => 0,
            1 => self.tell(),
            2 => self.info.file_size,
            _ => {
                return Err(Error::new(
                    Kind::ValueError,
                    "whence must be os.SEEK_SET (0), os.SEEK_CUR (1), or os.SEEK_END (2)",
                ));
            }
        };
        let target = if let Some(delta) = offset.as_small() {
            base.saturating_add_signed(delta)
        } else {
            let delta = offset.big();
            let magnitude = delta.magnitude().to_u64().unwrap_or(u64::MAX);
            if delta.sign() == Sign::Minus {
                base.saturating_sub(magnitude)
            } else {
                base.saturating_add(magnitude)
            }
        };
        self.seek(SeekFrom::Start(target))
    }

    pub(in crate::objects) fn seek(&mut self, at: SeekFrom) -> Result<u64> {
        let now = self.tell();
        let to = match at {
            SeekFrom::Start(n) => n,
            SeekFrom::Current(d) => now.saturating_add_signed(d),
            SeekFrom::End(d) => self.info.file_size.saturating_add_signed(d),
        }
        .min(self.info.file_size);
        if to == now {
            return Ok(now);
        }
        let offset = i128::from(to) - i128::from(now) + self.pos as i128;
        if (0..self.buffer.len() as i128).contains(&offset) {
            self.pos = offset as usize;
            return Ok(to);
        }
        if self.info.method == 0 {
            let at = self
                .source
                .absolute_offset(i128::from(self.start) + i128::from(to))?;
            self.source.position(at)?;
            // Seeking a stored member skips data: its full CRC cannot be checked.
            self.crc = None;
            self.packed_at = at;
            self.left = self.info.file_size - to;
            self.packed_left = self.info.compress_size.saturating_sub(to);
            self.buffer.clear();
            self.pos = 0;
            self.eof = self.left == 0;
        } else {
            if to < now {
                let decoder = Decoder::new(self.info.method)?;
                self.source.position(self.start)?;
                self.packed_at = self.start;
                self.packed_left = self.info.compress_size;
                self.left = self.info.file_size;
                self.decoder = decoder;
                self.crc = Some(Crc::new());
                self.eof = false;
                self.packed.clear();
                self.packed_pos = 0;
                self.buffer.clear();
                self.pos = 0;
            }
            let seek_read = if matches!(self.decoder, Decoder::Zstandard(_)) {
                1 << 24 // ZipExtFile.MAX_SEEK_READ
            } else {
                BLOCK
            };
            while self.tell() < to {
                let n = (to - self.tell()).min(seek_read as u64) as usize;
                if self.read(Some(n))?.is_empty() {
                    break;
                }
            }
        }
        Ok(self.tell())
    }
}

#[cfg(test)]
mod tests {
    use super::super::Archive;
    use super::*;
    use crate::objects::files::FileState;

    // CPython-generated ZIP_DEFLATED archive containing exactly 4,097 NULs.
    const FIXTURE: &[u8] = b"PK\x03\x04\x14\x00\x00\x00\x08\x00\x00\x00!\x00\x7f\xd3u\xb8\x14\x00\x00\x00\x01\x10\x00\x00\x01\x00\x00\x00p\xed\xc1\x01\x0d\x00\x00\x00\xc2\xa0\xf7Om\x0f\x07\x14\x00\x00\x00poPK\x01\x02\x14\x03\x14\x00\x00\x00\x08\x00\x00\x00!\x00\x7f\xd3u\xb8\x14\x00\x00\x00\x01\x10\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x80\x01\x00\x00\x00\x00pPK\x05\x06\x00\x00\x00\x00\x01\x00\x01\x00/\x00\x00\x003\x00\x00\x00\x00\x00";

    #[test]
    fn zstandard_read_into_preserves_positions_and_propagates_errors() {
        // The pinned CPython ZstdBadCrcTests archive, with its directory CRC repaired.
        let archive = b"PK\x03\x04?\0\0\0]\0\0\0!\0V\xb1\x17J\x14\0\0\0\x0b\0\0\0\x05\0\0\0afile(\
            \xb5/\xfd\0XY\0\0Hello WorldPK\x01\x02?\x03?\0\0\0]\0\0\0!\0V\xb1\x17J\x14\0\0\0\
            \x0b\0\0\0\x05\0\0\0\0\0\0\0\0\0\0\0\x80\x01\0\0\0\0afilePK\x05\x06\0\0\0\0\
            \x01\0\x01\x003\0\0\x007\0\0\0\0\0";
        let open = |bytes: Vec<u8>| {
            let input = Source::Borrowed(FileState::memory(crate::BytesOwner::new(bytes)));
            let archive = Archive::open(input, None).ok().unwrap();
            FileState::member(archive.member_by_name(&Str::new("afile")).ok().unwrap())
        };
        let reader = open(archive.to_vec());
        // SAFETY: the local owner is the sole accessor throughout these reads.
        unsafe {
            reader.0.with_mut(|s| {
                let file = s.file_mut();
                let mut buffer = *b"_____";
                assert_eq!(file.read_into(&mut []).ok(), Some(0));
                assert_eq!(file.read_into(&mut buffer[1..4]).ok(), Some(3));
                assert_eq!(&buffer, b"_Hel_");
                assert_eq!(file.tell().ok(), Some(3));
                assert_eq!(file.seek(SeekFrom::End(-2)).ok(), Some(9));
                assert_eq!(file.read_into(&mut buffer).ok(), Some(2));
                assert_eq!(&buffer, b"ldel_");
                assert_eq!(file.read_into(&mut buffer).ok(), Some(0));
                assert_eq!(&buffer, b"ldel_");
                assert_eq!(file.seek(SeekFrom::Start(0)).ok(), Some(0));
                assert_eq!(file.read_into(&mut buffer).ok(), Some(5));
                assert_eq!(&buffer, b"Hello");
            })
        }
        for (offset, kind, position) in [(35, Kind::ZstdError, 0), (72, Kind::BadZipFile, 11)] {
            let mut bytes = archive.to_vec();
            bytes[offset] ^= 1; // Frame magic or central directory CRC.
            let reader = open(bytes);
            // SAFETY: the local owner is the sole accessor throughout this read.
            unsafe {
                reader.0.with_mut(|s| {
                    let file = s.file_mut();
                    let mut buffer = *b"___";
                    assert!(file.read_into(&mut buffer).unwrap_err().is_kind(kind));
                    assert_eq!(&buffer, b"___");
                    assert_eq!(file.tell().ok(), Some(position));
                })
            }
        }
    }

    #[test]
    fn stored_zip64_seek_checks_the_absolute_offset() {
        // A one-byte stored member declares u64::MAX uncompressed bytes.
        let bytes = b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00!\x00K6\xb2\xb7\x01\x00\x00\x00\x01\x00\x00\x00\x01\x00\x0c\x00x\x01\x00\x08\x00\xff\xff\xff\xff\xff\xff\xff\xffXPK\x01\x02\x14\x03\x14\x00\x00\x00\x00\x00\x00\x00!\x00K6\xb2\xb7\x01\x00\x00\x00\xff\xff\xff\xff\x01\x00\x0c\x00\x00\x00\x00\x00\x00\x00\x00\x00\x80\x01\x00\x00\x00\x00x\x01\x00\x08\x00\xff\xff\xff\xff\xff\xff\xff\xffPK\x05\x06\x00\x00\x00\x00\x01\x00\x01\x00;\x00\x00\x00,\x00\x00\x00\x00\x00";
        let input = Source::Borrowed(FileState::memory(crate::BytesOwner::new(bytes.to_vec())));
        let archive = Archive::open(input, None).ok().unwrap();
        let mut member = archive.member_by_name(&Str::new("x")).ok().unwrap();

        let error = member.seek(SeekFrom::End(0)).unwrap_err();
        assert!(error.is_kind(Kind::OverflowError));
        assert_eq!(member.tell(), 0);
        let almost_size = Int::new((1 - i128::from(u64::MAX)).into());
        assert_eq!(member.seek_offset(&almost_size, 2).ok(), Some(1));
        assert_eq!(
            member.seek_offset(&Int::new(i128::MIN.into()), 2).ok(),
            Some(0)
        );
        assert_eq!(member.read(Some(1)).ok().unwrap(), b"X");
    }

    #[test]
    fn invalid_deflate_block_consumes_input_without_advancing_member_position() {
        let mut raw = FIXTURE.to_vec();
        raw[31] = 7; // Reserved DEFLATE block type in the first payload byte.
        let input = Source::Borrowed(FileState::memory(crate::BytesOwner::new(raw)));
        let archive = Archive::open(input, None).ok().unwrap();
        let mut member = archive.member_by_name(&Str::new("p")).ok().unwrap();
        member.min_read = 1;
        assert!(member.read(Some(1)).unwrap_err().is_kind(Kind::ZlibError));
        assert_eq!(member.tell(), 0);
        let packed_at = member.packed_at;
        assert!(member.read(Some(1)).unwrap_err().is_kind(Kind::ZlibError));
        assert_eq!(member.packed_at, packed_at + 1);
        assert_eq!(member.tell(), 0);
    }

    #[test]
    fn last_compressed_byte_flushes_before_returning_an_exact_sized_read() {
        let mut raw = FIXTURE.to_vec();
        let central = memchr::memmem::find(&raw, b"PK\x01\x02").unwrap();
        raw[central + 16] ^= 1;
        let input = Source::Borrowed(FileState::memory(crate::BytesOwner::new(raw)));
        let archive = Archive::open(input, None).ok().unwrap();
        let mut member = archive.member_by_name(&Str::new("p")).ok().unwrap();
        member.min_read = 2;
        assert!(member.read(Some(0)).ok().unwrap().is_empty());
        // zlib has consumed all compressed input after producing 4096 bytes,
        // with one uncompressed byte still pending. Its CRC fails on this read.
        assert!(
            member
                .read(Some(4096))
                .unwrap_err()
                .is_kind(Kind::BadZipFile)
        );
    }

    #[test]
    fn independent_members_survive_archive_close_and_rewind() {
        let input = Source::Borrowed(FileState::memory(crate::BytesOwner::new(FIXTURE.to_vec())));
        let mut archive = Archive::open(input, None).ok().unwrap();
        let mut left = archive.member_by_name(&Str::new("p")).ok().unwrap();
        let mut right = archive.member_by_name(&Str::new("p")).ok().unwrap();
        archive.source.take();
        assert!(archive.member_by_name(&Str::new("p")).is_err());
        assert_eq!(left.read(Some(4096)).ok().unwrap().len(), 4096);
        assert_eq!(right.seek(SeekFrom::End(-2)).ok().unwrap(), 4095);
        assert_eq!(right.read(None).ok().unwrap(), [0; 2]);
        assert_eq!(left.read(None).ok().unwrap(), [0]);
        assert_eq!(right.seek(SeekFrom::Start(1)).ok().unwrap(), 1);
        assert_eq!(right.read(None).ok().unwrap().len(), 4096);
    }
}
