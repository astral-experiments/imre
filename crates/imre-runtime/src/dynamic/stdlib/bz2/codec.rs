//! One libbzip2-compatible stream per Python compressor or decompressor.
use super::{Error, Result};
use allocator_api2::boxed::Box;
use bzip2_sys as bz;

enum Direction {
    Encode(i32),
    Decode,
}

struct Stream {
    // The codec retains this address for the lifetime of its state.
    raw: Box<bz::bz_stream>,
    direction: Direction,
}

fn check(status: i32) -> Result<()> {
    match status {
        bz::BZ_OK | bz::BZ_RUN_OK | bz::BZ_FINISH_OK | bz::BZ_STREAM_END => Ok(()),
        bz::BZ_MEM_ERROR => Err(Error::new("MemoryError", "")),
        bz::BZ_DATA_ERROR | bz::BZ_DATA_ERROR_MAGIC => {
            Err(Error::new("OSError", "Invalid data stream"))
        }
        bz::BZ_PARAM_ERROR => Err(Error::new(
            "ValueError",
            "Internal error - invalid parameters passed to libbzip2",
        )),
        bz::BZ_SEQUENCE_ERROR => Err(Error::new(
            "RuntimeError",
            "Internal error - Invalid sequence of commands sent to libbzip2",
        )),
        _ => Err(Error::new(
            "OSError",
            format!("Unrecognized error from libbzip2: {status}"),
        )),
    }
}

impl Stream {
    fn new(direction: Direction) -> Result<Self> {
        // SAFETY: bz_stream contains only integers, raw pointers, and nullable callbacks;
        // the all-zero value selects the default libbzip2 allocator.
        let empty: bz::bz_stream = unsafe { std::mem::zeroed() };
        let mut raw = Box::try_new(empty).map_err(|_| Error::new("MemoryError", ""))?;
        // SAFETY: raw is a stable, zero-initialized stream using the default allocator.
        let status = unsafe {
            match direction {
                Direction::Encode(level) => bz::BZ2_bzCompressInit(&mut *raw, level, 0, 0),
                Direction::Decode => bz::BZ2_bzDecompressInit(&mut *raw, 0, 0),
            }
        };
        check(status)?;
        Ok(Self { raw, direction })
    }

    fn process(
        &mut self,
        input: &[u8],
        limit: usize,
        finish: bool,
    ) -> Result<(Vec<u8>, usize, bool)> {
        // CPython's output window sizes. With partial input a decoder can exhaust
        // its input at a window boundary while still holding undelivered output.
        const WINDOW_KIB: &[usize] = &[
            32, 64, 256, 1024, 4096, 8192, 16384, 16384, 32768, 32768, 32768, 32768, 65536, 65536,
            131072, 131072, 262144,
        ];
        let mut windows = WINDOW_KIB.iter().copied().chain(std::iter::repeat(262144));
        let mut output = Vec::new();
        let mut end = 0;
        let mut offset = 0;
        loop {
            let written = output.len();
            if written == end {
                let space = (windows.next().unwrap() * 1024).min(limit - written);
                output
                    .try_reserve(space)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                end += space;
            }
            output.resize(end, 0);
            let n = (input.len() - offset).min(u32::MAX as usize) as u32;
            self.raw.next_in = input[offset..].as_ptr().cast_mut().cast();
            self.raw.avail_in = n;
            self.raw.next_out = output[written..].as_mut_ptr().cast();
            self.raw.avail_out = (end - written) as u32;
            // SAFETY: input and output are disjoint live slices, bounded by the
            // indicated lengths. libbzip2 reads but does not mutate next_in.
            // No pointer into either slice escapes this call.
            let status = unsafe {
                match self.direction {
                    Direction::Decode => bz::BZ2_bzDecompress(&mut *self.raw),
                    Direction::Encode(_) => bz::BZ2_bzCompress(
                        &mut *self.raw,
                        if finish { bz::BZ_FINISH } else { bz::BZ_RUN },
                    ),
                }
            };
            self.raw.next_in = std::ptr::null_mut();
            self.raw.next_out = std::ptr::null_mut();
            check(status)?;
            offset += (n - self.raw.avail_in) as usize;
            output.truncate(end - self.raw.avail_out as usize);
            let eof = status == bz::BZ_STREAM_END;
            if eof || (!finish && (offset == input.len() || output.len() == limit)) {
                return Ok((output, offset, eof));
            }
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: this stream successfully initialized in the corresponding direction,
        // owns its codec state, and releases it once while raw is still at its original address.
        unsafe {
            match self.direction {
                Direction::Decode => bz::BZ2_bzDecompressEnd(&mut *self.raw),
                Direction::Encode(_) => bz::BZ2_bzCompressEnd(&mut *self.raw),
            };
        }
    }
}

pub(in crate::dynamic) struct Compressor(Option<Stream>);

impl Compressor {
    pub(in crate::dynamic) fn new(level: i32) -> Result<Self> {
        if !(1..=9).contains(&level) {
            return Err(Error::new(
                "ValueError",
                "compresslevel must be between 1 and 9",
            ));
        }
        Ok(Self(Some(Stream::new(Direction::Encode(level))?)))
    }

    pub(in crate::dynamic) fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        let stream = self
            .0
            .as_mut()
            .ok_or_else(|| Error::new("ValueError", "Compressor has been flushed"))?;
        if input.is_empty() {
            return Ok(Vec::new());
        }
        Ok(stream.process(input, usize::MAX, false)?.0)
    }

    pub(in crate::dynamic) fn flush(&mut self) -> Result<Vec<u8>> {
        let mut stream = self
            .0
            .take()
            .ok_or_else(|| Error::new("ValueError", "Repeated call to flush()"))?;
        Ok(stream.process(&[], usize::MAX, true)?.0)
    }
}

pub(in crate::dynamic) struct Decompressor {
    stream: Stream,
    pending: Vec<u8>,
    offset: usize,
    eof: bool,
    needs_input: bool,
}

impl Decompressor {
    pub(in crate::dynamic) fn new() -> Result<Self> {
        Ok(Self {
            stream: Stream::new(Direction::Decode)?,
            pending: Vec::new(),
            offset: 0,
            eof: false,
            needs_input: true,
        })
    }

    pub(in crate::dynamic) fn decompress(&mut self, data: &[u8], max: usize) -> Result<Vec<u8>> {
        if self.eof {
            return Err(Error::new("EOFError", "End of stream already reached"));
        }
        let buffered = !self.pending.is_empty();
        let input = if buffered {
            if self.offset > 0 && self.pending.capacity() - self.pending.len() < data.len() {
                self.pending.drain(..self.offset);
                self.offset = 0;
            }
            self.pending
                .try_reserve(data.len())
                .map_err(|_| Error::new("MemoryError", ""))?;
            self.pending.extend_from_slice(data);
            &self.pending[self.offset..]
        } else {
            data
        };
        let (output, consumed, eof) = match self.stream.process(input, max, false) {
            Ok(result) => result,
            Err(err) => {
                self.pending.clear();
                self.offset = 0;
                return Err(err);
            }
        };
        let has_tail = consumed < input.len();
        self.eof = eof;
        self.needs_input = !eof && !has_tail;
        if !has_tail {
            self.pending.clear();
            self.offset = 0;
        } else if buffered {
            self.offset += consumed;
        } else {
            let tail = &data[consumed..];
            self.pending
                .try_reserve(tail.len())
                .map_err(|_| Error::new("MemoryError", ""))?;
            self.pending.extend_from_slice(tail);
        }
        Ok(output)
    }

    pub(in crate::dynamic) fn eof(&self) -> bool {
        self.eof
    }
    pub(in crate::dynamic) fn needs_input(&self) -> bool {
        self.needs_input
    }
    pub(in crate::dynamic) fn take_unused_data(&mut self) -> Vec<u8> {
        debug_assert!(self.eof);
        self.pending.drain(..self.offset);
        self.offset = 0;
        std::mem::take(&mut self.pending)
    }
}
