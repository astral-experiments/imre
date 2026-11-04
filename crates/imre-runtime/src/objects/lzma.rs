//! Errors shared by the LZMA stream provider and native ZIP member cursors.

use super::exceptions::{Error, Kind};
use liblzma::stream::Error as CodecError;

pub(crate) fn codec_error(err: CodecError) -> Error {
    let message = match err {
        CodecError::Data => "Corrupt input data",
        CodecError::Format => "Input format not supported by decoder",
        CodecError::Options => "Invalid or unsupported options",
        CodecError::MemLimit => "Memory usage limit exceeded",
        CodecError::Mem => return Error::new(Kind::MemoryError, ""),
        CodecError::UnsupportedCheck => "Unsupported integrity check",
        CodecError::NoCheck | CodecError::Program => "Internal error",
    };
    Error::new(Kind::LzmaError, message)
}
