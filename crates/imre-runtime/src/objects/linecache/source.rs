use super::super::{Error, Kind};
pub(crate) use imre_ast::source_encoding::SourceDecodeError;
use std::borrow::Cow;

fn utf8_error(bytes: &[u8], error: std::str::Utf8Error) -> String {
    super::super::bytes::utf8_error_details(bytes, error).2
}

pub(crate) fn decode_compile_source(bytes: &[u8]) -> Result<String, SourceDecodeError> {
    imre_ast::source_encoding::decode_compile_source(bytes, utf8_error)
}

/// Decode boxed disk reads using the same cookie detection and physical lines as typed linecache.
pub(crate) fn decode_source_lines(bytes: &[u8]) -> Result<Vec<String>, SourceDecodeError> {
    let text = imre_ast::source_encoding::decode_source(bytes, utf8_error)?;
    Ok(lines(&text).map(|line| format!("{line}\n")).collect())
}

pub(super) fn decode(bytes: &[u8]) -> Result<Option<Cow<'_, str>>, Error> {
    match imre_ast::source_encoding::decode_source(bytes, utf8_error) {
        Ok(text) => Ok(Some(text)),
        Err(SourceDecodeError::Allocation) => Err(Error::allocation()),
        Err(SourceDecodeError::Null | SourceDecodeError::Invalid(..)) => Ok(None),
        Err(SourceDecodeError::NonText(msg)) => Err(Error::new(Kind::LookupError, msg)),
        Err(SourceDecodeError::Unsupported(name)) => Err(Error::new(
            Kind::NotImplementedError,
            format!("linecache: native source decoder for '{name}' is unavailable"),
        )),
    }
}

pub(super) fn lines(source: &str) -> impl Iterator<Item = &str> {
    let mut tail = source;
    std::iter::from_fn(move || {
        if tail.is_empty() {
            return None;
        }
        if let Some(i) = tail.find(['\r', '\n']) {
            let head = &tail[..i];
            let skip = if tail[i..].starts_with("\r\n") { 2 } else { 1 };
            tail = &tail[i + skip..];
            Some(head)
        } else {
            Some(std::mem::take(&mut tail))
        }
    })
}
