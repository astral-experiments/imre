//! Incremental Zstandard streams. Each object owns one libzstd context.
use crate::{Error, Kind, Result};
use zstd_safe::zstd_sys::ZSTD_EndDirective::{ZSTD_e_continue, ZSTD_e_end, ZSTD_e_flush};
use zstd_safe::{CCtx, CParameter, DCtx, InBuffer, OutBuffer, ResetDirective};

fn zstd_error(operation: &str, code: usize) -> Error {
    Error::new(
        Kind::ZstdError,
        format!("{operation}: {}", zstd_safe::get_error_name(code)),
    )
}

// Offer at most one more block to the codec, including a zero-sized block when
// max_length is zero: libzstd can still consume a frame header in that case.
fn output_block(
    bytes: &mut Vec<u8>,
    limit: usize,
    pump: impl FnOnce(&mut OutBuffer<'_, [u8]>) -> Result<usize>,
) -> Result<(usize, bool)> {
    let start = bytes.len();
    let space = (32 * 1024).min(limit - start);
    bytes.try_reserve(space).map_err(|_| Error::allocation())?;
    bytes.resize(start + space, 0);
    let mut out = OutBuffer::around(&mut bytes[start..]);
    let remaining = pump(&mut out)?;
    let written = out.pos();
    bytes.truncate(start + written);
    Ok((remaining, written == space))
}

pub(crate) struct Compressor {
    stream: CCtx<'static>,
    pub(crate) last_mode: i32,
}

impl Compressor {
    pub(super) fn new(level: Option<i32>) -> Result<Self> {
        let mut stream = CCtx::try_create()
            .ok_or_else(|| Error::new(Kind::ZstdError, "Unable to create ZSTD_CCtx instance."))?;
        if let Some(level) = level {
            let min = zstd_safe::min_c_level();
            let max = zstd_safe::max_c_level();
            if !(min..=max).contains(&level) {
                return Err(Error::new(
                    Kind::ValueError,
                    format!("illegal compression level {level}; the valid range is [{min}, {max}]"),
                ));
            }
            stream
                .set_parameter(CParameter::CompressionLevel(level))
                .map_err(|e| zstd_error("Unable to set compression level", e))?;
        }
        Ok(Self {
            stream,
            last_mode: 2,
        })
    }

    pub(super) fn set_pledged_input_size(&mut self, size: Option<u64>) -> Result<()> {
        if size.is_some_and(|n| n >= zstd_safe::CONTENTSIZE_ERROR) {
            return Err(Error::new(
                Kind::ValueError,
                format!(
                    "size argument should be a positive int less than {}",
                    zstd_safe::CONTENTSIZE_ERROR
                ),
            ));
        }
        if self.last_mode != 2 {
            return Err(Error::new(
                Kind::ValueError,
                "set_pledged_input_size() method must be called when last_mode == FLUSH_FRAME",
            ));
        }
        self.stream
            .set_pledged_src_size(size)
            .map_err(|e| zstd_error("Unable to set pledged uncompressed content size", e))?;
        Ok(())
    }

    pub(super) fn compress(&mut self, data: &[u8], mode: i32) -> Result<Vec<u8>> {
        let directive = match mode {
            0 => ZSTD_e_continue,
            1 => ZSTD_e_flush,
            2 => ZSTD_e_end,
            _ => {
                return Err(Error::new(
                    Kind::ValueError,
                    "mode argument wrong value, it should be one of ZstdCompressor.CONTINUE, ZstdCompressor.FLUSH_BLOCK, ZstdCompressor.FLUSH_FRAME.",
                ));
            }
        };
        let mut input = InBuffer { src: data, pos: 0 };
        let mut bytes = Vec::new();
        loop {
            let step = output_block(&mut bytes, usize::MAX, |out| {
                self.stream
                    .compress_stream2(out, &mut input, directive)
                    .map_err(|e| zstd_error("Unable to compress Zstandard data", e))
            });
            match step {
                Ok((0, _)) if input.pos == data.len() => {
                    self.last_mode = mode;
                    return Ok(bytes);
                }
                Ok(_) => (),
                Err(err) => {
                    let _ = self.stream.reset(ResetDirective::SessionOnly);
                    self.last_mode = 2;
                    return Err(err);
                }
            }
        }
    }
}

pub(crate) struct Decompressor {
    stream: DCtx<'static>,
    pending: Vec<u8>,
    offset: usize,
    pub(super) eof: bool,
    pub(super) needs_input: bool,
}

impl Decompressor {
    pub(super) fn new() -> Result<Self> {
        let stream = DCtx::try_create()
            .ok_or_else(|| Error::new(Kind::ZstdError, "Unable to create ZSTD_DCtx instance."))?;
        Ok(Self {
            stream,
            pending: Vec::new(),
            offset: 0,
            eof: false,
            needs_input: true,
        })
    }

    pub(super) fn decompress(&mut self, data: &[u8], max_length: usize) -> Result<Vec<u8>> {
        if self.eof {
            return Err(Error::new(
                Kind::EOFError,
                "Already at the end of a Zstandard frame.",
            ));
        }
        let result = self.decode(data, max_length);
        if result.is_err() {
            self.pending.clear();
            self.offset = 0;
            self.needs_input = true;
            self.eof = false;
            let _ = self.stream.reset(ResetDirective::SessionOnly);
        }
        result
    }

    fn decode(&mut self, data: &[u8], max_length: usize) -> Result<Vec<u8>> {
        if self.pending.capacity() - self.pending.len() < data.len() && self.offset != 0 {
            self.pending.drain(..self.offset);
            self.offset = 0;
        }
        self.pending
            .try_reserve(data.len())
            .map_err(|_| Error::allocation())?;
        self.pending.extend_from_slice(data);
        let mut input = InBuffer {
            src: &self.pending[self.offset..],
            pos: 0,
        };
        let mut bytes = Vec::new();
        loop {
            let (remaining, full) = output_block(&mut bytes, max_length, |out| {
                self.stream
                    .decompress_stream(out, &mut input)
                    .map_err(|e| zstd_error("Unable to decompress Zstandard data", e))
            })?;
            self.eof = remaining == 0;
            let consumed = input.pos == input.src.len();
            let limited = bytes.len() == max_length;
            // Exhausting input alone does not prove the stream needs more: a
            // full output window may leave buffered bytes inside libzstd.
            if self.eof || limited || (consumed && !full) {
                self.needs_input = consumed && !limited && !self.eof;
                self.offset += input.pos;
                if consumed {
                    self.pending.clear();
                    self.offset = 0;
                }
                return Ok(bytes);
            }
        }
    }

    pub(super) fn take_unused_data(&mut self) -> Vec<u8> {
        debug_assert!(self.eof);
        self.pending.drain(..self.offset);
        self.offset = 0;
        std::mem::take(&mut self.pending)
    }
}
