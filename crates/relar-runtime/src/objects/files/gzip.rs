//! CPython's gzip member framing on top of flate2's streaming DEFLATE decoder.

use std::fs::File;
use std::io::{self, BufRead, Read};
use std::mem::MaybeUninit;
use std::ops::Range;

use flate2::{Crc, Decompress, FlushDecompress, Status};

use super::Kind;

const CHUNK_SIZE: usize = 128 * 1024;

#[derive(Debug)]
pub(super) struct GzipError(pub(super) Kind, pub(super) &'static str);

impl std::fmt::Display for GzipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.1)
    }
}

impl std::error::Error for GzipError {}

enum Phase {
    Header,
    Body { chunk_end: Option<usize> },
    Trailer,
}

/// Metadata reads and DEFLATE share unread bytes across gzip members. A new
/// DEFLATE chunk starts at the current compressed position, even after a header.
struct GzipInput {
    file: File,
    block: Box<[u8]>,
    unread: Range<usize>,
}

impl GzipInput {
    fn fill_block(&mut self) -> io::Result<usize> {
        if self.unread.start + CHUNK_SIZE > self.block.len() {
            self.block.copy_within(self.unread.clone(), 0);
            self.unread = 0..self.unread.len();
        }
        let end = self.unread.start + CHUNK_SIZE;
        while self.unread.end < end {
            match self.file.read(&mut self.block[self.unread.end..]) {
                Ok(0) => break,
                Ok(count) => self.unread.end += count,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
        Ok(end.min(self.unread.end))
    }
}

impl BufRead for GzipInput {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.unread.is_empty() {
            self.unread = 0..self.file.read(&mut self.block[..CHUNK_SIZE])?;
        }
        Ok(&self.block[self.unread.clone()])
    }

    fn consume(&mut self, count: usize) {
        self.unread.start += count;
    }
}

impl Read for GzipInput {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let count = self.fill_buf()?.read(out)?;
        self.consume(count);
        Ok(count)
    }
}

pub(super) struct GzipReader {
    input: GzipInput,
    decoder: Decompress,
    crc: Crc,
    phase: Phase,
}

impl GzipReader {
    pub(super) fn new(file: File) -> Self {
        Self {
            input: GzipInput {
                file,
                // Leave room to slide past small members without moving a full chunk each time.
                block: vec![0; CHUNK_SIZE + 4096].into_boxed_slice(),
                unread: 0..0,
            },
            decoder: Decompress::new(false),
            crc: Crc::new(),
            phase: Phase::Header,
        }
    }

    pub(super) fn into_file(self) -> File {
        self.input.file
    }

    /// Skip optional metadata without allocating in proportion to its size.
    /// Python consumes the header CRC and ignores reserved flag bits.
    fn header(&mut self) -> io::Result<bool> {
        if self.input.fill_buf()?.is_empty() {
            return Ok(false);
        }
        let mut magic = [0; 2];
        if let Err(err) = self.input.read_exact(&mut magic)
            && err.kind() != io::ErrorKind::UnexpectedEof
        {
            return Err(err);
        }
        if magic != [0x1f, 0x8b] {
            return Err(io::Error::other(GzipError(
                Kind::BadGzipFile,
                "Not a gzipped file",
            )));
        }
        let mut header = [0; 8];
        self.input.read_exact(&mut header)?;
        if header[0] != 8 {
            return Err(io::Error::other(GzipError(
                Kind::BadGzipFile,
                "Unknown compression method",
            )));
        }
        let flags = header[1];
        if flags & 4 != 0 {
            let mut length = [0; 2];
            self.input.read_exact(&mut length)?;
            let length = u64::from(u16::from_le_bytes(length));
            let copied = io::copy(&mut self.input.by_ref().take(length), &mut io::sink())?;
            if copied != length {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
        for flag in [8, 16] {
            if flags & flag != 0 {
                self.input.skip_until(0)?;
            }
        }
        if flags & 2 != 0 {
            self.input.read_exact(&mut [0; 2])?;
        }
        self.phase = Phase::Body { chunk_end: None };
        Ok(true)
    }

    /// Verify a finished member when the caller requests more bytes. A caller
    /// stopping at its last result has not yet observed its checksum or trailer.
    fn trailer(&mut self) -> io::Result<()> {
        let mut trailer = [0; 8];
        self.input.read_exact(&mut trailer)?;
        if trailer[..4] != self.crc.sum().to_le_bytes() {
            return Err(io::Error::other(GzipError(
                Kind::BadGzipFile,
                "CRC check failed",
            )));
        }
        if trailer[4..] != self.crc.amount().to_le_bytes() {
            return Err(io::Error::other(GzipError(
                Kind::BadGzipFile,
                "Incorrect length of data produced",
            )));
        }
        loop {
            let bytes = self.input.fill_buf()?;
            let padding = bytes.iter().take_while(|&&byte| byte == 0).count();
            if padding == 0 {
                break;
            }
            self.input.consume(padding);
        }
        self.decoder.reset(false);
        self.crc.reset();
        self.phase = Phase::Header;
        Ok(())
    }

    /// Decode one raw read, initializing only the returned prefix of the destination.
    pub(super) fn read(&mut self, out: &mut [MaybeUninit<u8>]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            match self.phase {
                Phase::Header => {
                    if !self.header()? {
                        return Ok(0);
                    }
                }
                Phase::Trailer => self.trailer()?,
                Phase::Body { chunk_end } => {
                    let end = match chunk_end {
                        Some(end) => end,
                        None => self.input.fill_block()?,
                    };
                    let input = &self.input.block[self.input.unread.start..end];
                    let empty = input.is_empty();
                    let before = (self.decoder.total_in(), self.decoder.total_out());
                    let status = self
                        .decoder
                        .decompress_uninit(input, out, FlushDecompress::None)
                        .map_err(|_| {
                            io::Error::other(GzipError(Kind::ZlibError, "invalid DEFLATE stream"))
                        })?;
                    let consumed = (self.decoder.total_in() - before.0) as usize;
                    let written = (self.decoder.total_out() - before.1) as usize;
                    self.input.consume(consumed);
                    if status == Status::StreamEnd {
                        self.phase = Phase::Trailer;
                    } else if written == 0 && consumed == 0 && (chunk_end.is_none() || !empty) {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    } else {
                        // A full output can leave bytes inside the inflater.
                        // Drain those before supplying another compressed chunk.
                        self.phase = Phase::Body {
                            chunk_end: (self.input.unread.start < end || written == out.len())
                                .then_some(end),
                        };
                    }
                    if written > 0 {
                        // SAFETY: total_out advanced by the initialized prefix length.
                        self.crc.update(unsafe { out[..written].assume_init_ref() });
                        return Ok(written);
                    }
                }
            }
        }
    }
}
