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
    let mut allowed = [false; 128];
    for &byte in safe.iter().filter(|b| b.is_ascii()) {
        allowed[usize::from(byte)] = true;
    }
    let safe = |b: u8| {
        (plus && b == b' ')
            || b.is_ascii_alphanumeric()
            || b"_.-~".contains(&b)
            || (b.is_ascii() && allowed[usize::from(b)])
    };
    let extra = input.iter().filter(|&&b| !safe(b)).count();
    let size = extra
        .checked_mul(2)
        .and_then(|n| n.checked_add(input.len()))
        .ok_or_else(Error::allocation)?;
    let mut out = String::new();
    out.try_reserve_exact(size)
        .map_err(|_| Error::allocation())?;
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in input {
        if plus && b == b' ' {
            out.push('+');
        } else if safe(b) {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
    }
    Ok(out)
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

/// Quote text or octets, sharing the codec and safe-set rules with runtime imports.
pub(crate) fn quote_component(
    source: UrlInput<'_>,
    safe: &[u8],
    encoding: Option<&str>,
    errors: Option<&str>,
    plus: bool,
) -> Result<String> {
    quote_with_owner(
        source,
        safe,
        encoding,
        errors,
        plus,
        super::try_owned_string,
    )
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

fn decode_text<E: From<Error>>(
    input: &[u8],
    mut decode: impl FnMut(&[u8]) -> std::result::Result<String, E>,
) -> std::result::Result<String, E> {
    let mut out = String::new();
    out.try_reserve_exact(input.len())
        .map_err(|_| Error::allocation())?;
    for span in input.chunk_by(|a, b| a.is_ascii() == b.is_ascii()) {
        let decoded;
        let text = if span[0].is_ascii() {
            decoded = decode(&unquoted(span)?)?;
            decoded.as_str()
        } else {
            // SAFETY: an ASCII boundary cannot bisect a scalar in the original string.
            unsafe { std::str::from_utf8_unchecked(span) }
        };
        out.try_reserve(text.len())
            .map_err(|_| Error::allocation())?;
        out.push_str(text);
    }
    Ok(out)
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
    decode_component_with(input, encoding, errors, |codec, data, errors| {
        codec.decode(data, Some(errors))
    })
}

/// Supply the decoder used on percent-decoded ASCII spans, retaining non-ASCII text verbatim.
pub(crate) fn decode_component_with<E: From<Error>>(
    input: UrlInput<'_>,
    encoding: Option<&str>,
    errors: Option<&str>,
    mut decode: impl FnMut(Codec, &[u8], &str) -> std::result::Result<String, E>,
) -> std::result::Result<String, E> {
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
        Ok(String::new())
    } else if bytes {
        decode(Codec::lookup(encoding)?, &unquoted(input)?, errors)
    } else {
        let codec = Codec::lookup(encoding)?;
        decode_text(input, |span| decode(codec, span, errors))
    }
}

mod query;
