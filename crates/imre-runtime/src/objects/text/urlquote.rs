//! Percent escapes operate on octets. Original Unicode text bypasses decoding.
use super::super::codecs::Codec;
use super::{Error, Kind, Result, Str, StrOwner, UrlInput, finish, percent_bytes, url_data};
use std::{borrow::Cow, ffi::c_void};

fn check_option(value: &str) -> Result<()> {
    if value.contains('\0') {
        return Err(Error::new(Kind::ValueError, "embedded null character"));
    }
    Ok(())
}

fn quoted(input: &[u8], safe: &[u8], plus: bool) -> Result<String> {
    const UNRESERVED: [bool; 256] = {
        let mut allowed = [false; 256];
        let mut byte = 0;
        while byte < 128 {
            let b = byte as u8;
            allowed[byte] = b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~');
            byte += 1;
        }
        allowed
    };

    let mut allowed = UNRESERVED;
    for &byte in safe.iter().filter(|b| b.is_ascii()) {
        allowed[usize::from(byte)] = true;
    }
    if plus {
        allowed[usize::from(b' ')] = true;
    }
    let extra = input.iter().filter(|&&b| !allowed[usize::from(b)]).count();
    let size = extra
        .checked_mul(2)
        .and_then(|n| n.checked_add(input.len()))
        .ok_or_else(Error::allocation)?;
    let mut out = Vec::new();
    out.try_reserve_exact(size)
        .map_err(|_| Error::allocation())?;
    if extra == 0 && (!plus || !input.contains(&b' ')) {
        out.extend_from_slice(input);
    } else {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for &b in input {
            if plus && b == b' ' {
                out.push(b'+');
            } else if allowed[usize::from(b)] {
                out.push(b);
            } else {
                out.extend_from_slice(&[b'%', HEX[usize::from(b >> 4)], HEX[usize::from(b & 15)]]);
            }
        }
    }
    // SAFETY: Only ASCII bytes can be allowed, and '+' and percent escapes are ASCII.
    Ok(unsafe { String::from_utf8_unchecked(out) })
}

/// Escape a URL component with an ASCII safe set and the requested text codec.
///
/// # Safety
/// The caller roots source and safe in the layouts selected by their tags. The
/// optional codec arguments point to rooted strings when present.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_url_quote(
    source: *const c_void,
    bytes: i32,
    safe: *const c_void,
    safe_bytes: i32,
    encoding: *const Str,
    errors: *const Str,
) -> *const Str {
    // SAFETY: the same rooted inputs are passed through with their original tags.
    unsafe { quote(source, bytes, safe, safe_bytes, encoding, errors, false) }
}

/// Quote a form component, replacing each ASCII space with a plus.
///
/// # Safety
/// The rooted source, safe set and optional codec arguments follow `imre_url_quote`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_url_quote_plus(
    source: *const c_void,
    bytes: i32,
    safe: *const c_void,
    safe_bytes: i32,
    encoding: *const Str,
    errors: *const Str,
) -> *const Str {
    // SAFETY: the same rooted inputs are passed through with their original tags.
    unsafe { quote(source, bytes, safe, safe_bytes, encoding, errors, true) }
}

unsafe fn quote(
    source: *const c_void,
    bytes: i32,
    safe: *const c_void,
    safe_bytes: i32,
    encoding: *const Str,
    errors: *const Str,
    plus: bool,
) -> *const Str {
    let result = (|| {
        // SAFETY: the typed caller retains safe and provides its layout.
        let safe = unsafe { url_data(safe, safe_bytes != 0) };
        // SAFETY: The source and optional strings remain rooted with their declared layouts.
        let (source_value, encoding, errors) = unsafe {
            (
                if bytes != 0 {
                    UrlInput::Bytes(url_data(source, true))
                } else {
                    UrlInput::Text(&*source.cast::<Str>())
                },
                encoding.as_ref().map(Str::as_str),
                errors.as_ref().map(Str::as_str),
            )
        };
        if matches!(source_value, UrlInput::Text("")) {
            // SAFETY: Retain the original empty string.
            return Ok(StrOwner::into_raw(unsafe {
                StrOwner::acquire_word(source as usize)
            }));
        }
        let value = quote_with_owner(source_value, safe, encoding, errors, plus, |_| {
            // SAFETY: Only encoding a text argument can request an error owner.
            Ok(unsafe { StrOwner::acquire_word(source as usize) })
        })?;
        crate::checked_owned_str(value.into())
    })();
    finish(result, std::ptr::null())
}

fn quote_with_owner(
    source: UrlInput<'_>,
    safe: &[u8],
    encoding: Option<&str>,
    errors: Option<&str>,
    plus: bool,
    owner: impl FnOnce(&str) -> Result<StrOwner>,
) -> Result<String> {
    match source {
        UrlInput::Bytes(bytes) => {
            if encoding.is_some() {
                return Err(Error::new(
                    Kind::TypeError,
                    "quote() doesn't support 'encoding' for bytes",
                ));
            }
            if errors.is_some() {
                return Err(Error::new(
                    Kind::TypeError,
                    "quote() doesn't support 'errors' for bytes",
                ));
            }
            quoted(bytes, safe, plus)
        }
        UrlInput::Text("") => Ok(String::new()),
        UrlInput::Text(text) => {
            let encoding = encoding.unwrap_or("utf-8");
            let errors = errors.unwrap_or("strict");
            let codec = Codec::for_encode(encoding, errors)?;
            let data = codec.encode(text, Some(errors), || owner(text))?;
            quoted(&data, safe, plus && text.contains(' '))
        }
    }
}

fn unquoted(input: &[u8]) -> Result<Cow<'_, [u8]>> {
    match memchr::memchr(b'%', input) {
        Some(first) => percent_bytes(input, first)
            .map(Cow::Owned)
            .ok_or_else(Error::allocation),
        None => Ok(Cow::Borrowed(input)),
    }
}

/// Decode percent escapes in ASCII spans, or decode the entire bytes argument.
///
/// # Safety
/// Source and optional strings remain rooted, with source layout selected by bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_url_unquote(
    source: *const c_void,
    bytes: i32,
    encoding: *const Str,
    errors: *const Str,
) -> *const Str {
    let result = (|| {
        // SAFETY: the tag and the optional option pointers come from rooted typed operands.
        let (input, encoding, errors) = unsafe {
            (
                if bytes != 0 {
                    UrlInput::Bytes(url_data(source, true))
                } else {
                    UrlInput::Text(&*source.cast::<Str>())
                },
                encoding.as_ref(),
                errors.as_ref(),
            )
        };
        if input.is_unescaped_text() {
            // SAFETY: The original unescaped scalar string remains rooted.
            return Ok(StrOwner::into_raw(unsafe {
                StrOwner::acquire_word(source as usize)
            }));
        }
        let text = decode_component(input, encoding.map(Str::as_str), errors.map(Str::as_str))?;
        crate::checked_owned_str(text.into())
    })();
    finish(result, std::ptr::null())
}

/// Decode an entire bytes value, or the ASCII spans of percent-escaped UTF-8 text.
pub(crate) fn decode_component(
    input: UrlInput<'_>,
    encoding: Option<&str>,
    errors: Option<&str>,
) -> Result<String> {
    let mut out = String::new();
    visit_unquoted(input, encoding, errors, |part| {
        let decoded;
        let text = match part {
            UnquotedPart::Literal(text) => text,
            UnquotedPart::Encoded(codec, data, errors) => {
                decoded = codec.decode(data, Some(errors))?;
                if out.is_empty() {
                    out = decoded;
                    return Ok(());
                }
                &decoded
            }
        };
        out.try_reserve(text.len())
            .map_err(|_| Error::allocation())?;
        out.push_str(text);
        Ok::<_, Error>(())
    })?;
    Ok(out)
}

/// Only ASCII runs of a text URL pass through the requested byte codec.
pub(crate) enum UnquotedPart<'a> {
    Literal(&'a str),
    Encoded(Codec, &'a [u8], &'a str),
}

pub(crate) fn visit_unquoted<E: From<Error>>(
    input: UrlInput<'_>,
    encoding: Option<&str>,
    errors: Option<&str>,
    mut visit: impl FnMut(UnquotedPart<'_>) -> std::result::Result<(), E>,
) -> std::result::Result<(), E> {
    let bytes = matches!(input, UrlInput::Bytes(_));
    if bytes && encoding.is_none() {
        return Err(Error::new(
            Kind::TypeError,
            "decode() argument 'encoding' must be str, not None",
        )
        .into());
    }
    let encoding = encoding.unwrap_or("utf-8");
    check_option(encoding)?;
    if bytes && errors.is_none() {
        return Err(Error::new(
            Kind::TypeError,
            "decode() argument 'errors' must be str, not None",
        )
        .into());
    }
    let errors = errors.unwrap_or("replace");
    check_option(errors)?;
    let input = input.as_bytes();
    if input.is_empty() {
        return Ok(());
    }
    let codec = Codec::lookup(encoding)?;
    if bytes {
        return visit(UnquotedPart::Encoded(codec, &unquoted(input)?, errors));
    }
    for span in input.chunk_by(|a, b| a.is_ascii() == b.is_ascii()) {
        if span[0].is_ascii() {
            visit(UnquotedPart::Encoded(codec, &unquoted(span)?, errors))?;
        } else {
            // ASCII boundaries cannot bisect a UTF-8 scalar.
            visit(UnquotedPart::Literal(std::str::from_utf8(span).unwrap()))?;
        }
    }
    Ok(())
}

mod query;
