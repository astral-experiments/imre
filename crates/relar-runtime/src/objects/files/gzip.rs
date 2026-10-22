//! CPython's gzip member framing on top of flate2's streaming DEFLATE decoder.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};

use flate2::{Crc, Decompress, FlushDecompress, Status};

use super::Kind;

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
    Body,
    Trailer,
}

pub(super) struct GzipReader {
    input: BufReader<File>,
    decoder: Decompress,
    crc: Crc,
    phase: Phase,
}

impl GzipReader {
    pub(super) fn new(file: File) -> Self {
        Self {
            input: BufReader::with_capacity(128 * 1024, file),
            decoder: Decompress::new(false),
            crc: Crc::new(),
            phase: Phase::Header,
        }
    }

    pub(super) fn into_file(self) -> File {
        self.input.into_inner()
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
        self.phase = Phase::Body;
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
}

impl Read for GzipReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
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
                Phase::Body => {
                    let input = self.input.fill_buf()?;
                    let before = (self.decoder.total_in(), self.decoder.total_out());
                    let status = self
                        .decoder
                        .decompress(input, out, FlushDecompress::None)
                        .map_err(|_| {
                            io::Error::other(GzipError(Kind::ZlibError, "invalid DEFLATE stream"))
                        })?;
                    let consumed = (self.decoder.total_in() - before.0) as usize;
                    let written = (self.decoder.total_out() - before.1) as usize;
                    self.input.consume(consumed);
                    if status == Status::StreamEnd {
                        self.phase = Phase::Trailer;
                    } else if written == 0 && consumed == 0 {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                    if written > 0 {
                        self.crc.update(&out[..written]);
                        return Ok(written);
                    }
                }
            }
        }
    }
}
