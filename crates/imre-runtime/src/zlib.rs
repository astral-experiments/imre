//! Checked compression of an owned typed bytes argument through the same zlib backend.

use crate::{Bytes, BytesOwner, Error, Int, Kind, Str, finish};
use std::{ffi::CStr, fmt::Write, mem::MaybeUninit};
use zlib_rs::{DeflateConfig, DeflateFlush, ReturnCode, c_api::z_stream, deflate};

const OUTPUT_CHUNK: usize = 32 * 1024;
const OUTPUT_MEMORY: &str = "Unable to allocate output buffer.";
const CODEC_MEMORY: &str = "Out of memory while compressing data";

enum Failure {
    Static(Kind, &'static str),
    Message(Kind, Str),
    Allocation,
}

impl Failure {
    fn output_memory() -> Self {
        Self::Static(Kind::MemoryError, OUTPUT_MEMORY)
    }

    fn codec_memory() -> Self {
        Self::Static(Kind::MemoryError, CODEC_MEMORY)
    }

    fn publish(self) -> *const Bytes {
        let null = std::ptr::null();
        match self {
            Self::Static(kind, message) => Error::return_static_message(kind, message, null),
            Self::Message(kind, message) => Error::return_checked_message(kind, message, null),
            Self::Allocation => finish(Err(Error::memory_error()), null),
        }
    }
}

/// Converts the canonical tagged integer without truncation.
fn c_int(word: usize) -> Result<i32, Failure> {
    // SAFETY: generated code roots the canonical tagged integer for this call.
    let value = unsafe { Int::borrow(word) };
    value
        .as_small()
        .and_then(|number| i32::try_from(number).ok())
        .ok_or(Failure::Static(
            Kind::OverflowError,
            "Python int too large to convert to C int",
        ))
}

struct Stream {
    raw: z_stream,
}

impl Stream {
    fn new(level: i32, window_bits: i32) -> Result<Self, Failure> {
        let mut raw = z_stream::default();
        let status = deflate::init(
            &mut raw,
            DeflateConfig {
                level,
                window_bits,
                ..DeflateConfig::default()
            },
        );
        match status {
            ReturnCode::Ok => Ok(Self { raw }),
            ReturnCode::MemError => Err(Failure::codec_memory()),
            _ => Err(Failure::Static(Kind::ZlibError, "Bad compression level")),
        }
    }

    fn step(
        &mut self,
        input: &[u8],
        output: &mut [MaybeUninit<u8>],
    ) -> (ReturnCode, usize, usize, bool) {
        let count = input.len().min(u32::MAX as usize) as u32;
        let space = output.len().min(u32::MAX as usize) as u32;
        self.raw.next_in = input.as_ptr().cast_mut();
        self.raw.avail_in = count;
        self.raw.next_out = output.as_mut_ptr().cast();
        self.raw.avail_out = space;
        let flush = if input.len() == count as usize {
            DeflateFlush::Finish
        } else {
            DeflateFlush::NoFlush
        };
        // SAFETY: initialization succeeded; the disjoint input and uninitialized output
        // regions cover their advertised lengths and remain live through this step.
        let status = unsafe {
            deflate::deflate(
                deflate::DeflateStream::from_stream_mut(&mut self.raw).unwrap(),
                flush,
            )
        };
        let result = (
            status,
            (count - self.raw.avail_in) as usize,
            (space - self.raw.avail_out) as usize,
            self.raw.avail_out == 0,
        );
        self.raw.next_in = std::ptr::null();
        self.raw.next_out = std::ptr::null_mut();
        self.raw.avail_in = 0;
        self.raw.avail_out = 0;
        result
    }

    fn error(&self, status: ReturnCode) -> Failure {
        if status == ReturnCode::MemError {
            return Failure::codec_memory();
        }
        let detail = if self.raw.msg.is_null() {
            match status {
                ReturnCode::BufError => "incomplete or truncated stream",
                ReturnCode::DataError => "invalid input data",
                ReturnCode::StreamError => "inconsistent stream state",
                _ => "",
            }
        } else {
            // SAFETY: the backend supplies static NUL-terminated diagnostic strings.
            unsafe { CStr::from_ptr(self.raw.msg) }.to_str().unwrap()
        };
        let Some(capacity) = detail.len().checked_add(64) else {
            return Failure::Allocation;
        };
        let Ok(mut message) = Str::try_with_capacity(capacity) else {
            return Failure::Allocation;
        };
        let separator = if detail.is_empty() { "" } else { ": " };
        if write!(
            &mut message,
            "Error {} while compressing data{separator}{detail}",
            status as i32
        )
        .is_err()
        {
            return Failure::Allocation;
        }
        Failure::Message(Kind::ZlibError, message)
    }

    fn process(&mut self, input: &[u8], mut output: Vec<u8>) -> Result<Vec<u8>, Failure> {
        let mut input_pos = 0;
        loop {
            if output.len() == output.capacity() {
                output
                    .try_reserve(OUTPUT_CHUNK)
                    .map_err(|_| Failure::output_memory())?;
            }
            let (status, consumed, produced, output_full) =
                self.step(&input[input_pos..], output.spare_capacity_mut());
            input_pos += consumed;
            // SAFETY: the stream initialized exactly produced bytes of the spare capacity.
            unsafe { output.set_len(output.len() + produced) };
            match status {
                ReturnCode::StreamEnd => return Ok(output),
                ReturnCode::Ok | ReturnCode::BufError
                    if (consumed != 0 || produced != 0)
                        && (output_full || input_pos < input.len()) => {}
                ReturnCode::Ok | ReturnCode::BufError => {
                    return Err(self.error(ReturnCode::BufError));
                }
                other => return Err(self.error(other)),
            }
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: Drop runs only for an initialized stream, and ends it once.
        unsafe {
            let _ = deflate::end(deflate::DeflateStream::from_stream_mut(&mut self.raw).unwrap());
        }
    }
}

fn compress(input: &[u8], level: i32, window_bits: i32) -> Result<Vec<u8>, Failure> {
    // CPython's Argument Clinic allocates the initial output before initializing zlib.
    let mut output = Vec::new();
    output
        .try_reserve_exact(OUTPUT_CHUNK)
        .map_err(|_| Failure::output_memory())?;
    Stream::new(level, window_bits)?.process(input, output)
}

/// Compresses a rooted immutable bytes value with checked C-int level and window arguments.
///
/// # Safety
/// `data` points to a live immutable bytes owner and both integer words are canonical rooted
/// tagged integers for the duration of the call. A non-null result transfers one owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zlib_compress(
    data: *const Bytes,
    level: usize,
    window_bits: usize,
) -> *const Bytes {
    let result = (|| {
        let level = c_int(level)?;
        let window_bits = c_int(window_bits)?;
        // SAFETY: the private ABI keeps data rooted for this call.
        let output = compress(unsafe { &*data }, level, window_bits)?;
        BytesOwner::try_new(output)
            .map(BytesOwner::into_raw)
            .map_err(|_| Failure::Allocation)
    })();
    match result {
        Ok(value) => value,
        Err(error) => error.publish(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_stream_matches_zlib_bytes_and_validates_settings() {
        assert_eq!(
            compress(b"abc", 9, 15).unwrap_or_else(|_| panic!("compression failed")),
            b"\x78\xda\x4b\x4c\x4a\x06\x00\x02\x4d\x01\x27"
        );
        assert!(matches!(
            compress(b"abc", 10, 15),
            Err(Failure::Static(Kind::ZlibError, "Bad compression level"))
        ));
    }
}
