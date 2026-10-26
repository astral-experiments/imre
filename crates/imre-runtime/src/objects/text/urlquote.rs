//! Percent escapes operate on octets. Original Unicode text bypasses decoding.
use super::super::codecs::Codec;
use super::{Error, Kind, Result, Str, StrOwner, finish, percent_bytes, url_data};
use std::{borrow::Cow, ffi::c_void};

fn check_option(value: &str) -> Result<()> {
    if value.contains('\0') {
        return Err(Error::new(Kind::ValueError, "embedded null character"));
    }
    Ok(())
}

fn quoted(input: &[u8], safe: &[u8]) -> Result<*const Str> {
    let mut allowed = [false; 128];
    for &byte in safe.iter().filter(|b| b.is_ascii()) {
        allowed[usize::from(byte)] = true;
    }
    let safe = |b: u8| {
        b.is_ascii_alphanumeric()
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
        if safe(b) {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
    }
    crate::checked_owned_str(out.into())
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
    let result = (|| {
        // SAFETY: the typed caller retains safe and provides its layout.
        let safe = unsafe { url_data(safe, safe_bytes != 0) };
        if bytes != 0 {
            for (option, message) in [
                (encoding, "quote() doesn't support 'encoding' for bytes"),
                (errors, "quote() doesn't support 'errors' for bytes"),
            ] {
                if !option.is_null() {
                    return Err(Error::new(Kind::TypeError, message));
                }
            }
            // SAFETY: the byte flag selects byte storage.
            return quoted(unsafe { url_data(source, true) }, safe);
        }
        // SAFETY: this branch selects a rooted scalar string.
        let text = unsafe { &*source.cast::<Str>() };
        if text.is_empty() {
            // SAFETY: the original owner is retained for the result.
            return Ok(StrOwner::into_raw(unsafe {
                StrOwner::acquire_word(source as usize)
            }));
        }
        // SAFETY: non-null options select live scalar string owners.
        let (encoding, errors) = unsafe { (encoding.as_ref(), errors.as_ref()) };
        let encoding = encoding.map_or("utf-8", Str::as_str);
        let errors = errors.map_or("strict", Str::as_str);
        check_option(encoding)?;
        check_option(errors)?;
        let data = Codec::lookup(encoding)?.encode(text, Some(errors), || {
            // SAFETY: any UnicodeEncodeError owns a share of its source.
            Ok(unsafe { StrOwner::acquire_word(source as usize) })
        })?;
        quoted(&data, safe)
    })();
    finish(result, std::ptr::null())
}

fn unquoted(input: &[u8]) -> Result<Cow<'_, [u8]>> {
    match memchr::memchr(b'%', input) {
        Some(first) => percent_bytes(input, first)
            .map(Cow::Owned)
            .ok_or_else(Error::allocation),
        None => Ok(Cow::Borrowed(input)),
    }
}

fn decode_text(input: &[u8], codec: Codec, errors: &str) -> Result<String> {
    let mut out = String::new();
    out.try_reserve_exact(input.len())
        .map_err(|_| Error::allocation())?;
    for span in input.chunk_by(|a, b| a.is_ascii() == b.is_ascii()) {
        let decoded;
        let text = if span[0].is_ascii() {
            decoded = codec.decode(&unquoted(span)?, Some(errors))?;
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
                url_data(source, bytes != 0),
                encoding.as_ref(),
                errors.as_ref(),
            )
        };
        if bytes == 0 && !input.contains(&b'%') {
            // SAFETY: the original scalar string remains live for transfer to the caller.
            return Ok(StrOwner::into_raw(unsafe {
                StrOwner::acquire_word(source as usize)
            }));
        }
        let encoding = encoding.map(Str::as_str);
        let errors = errors.map(Str::as_str);
        if bytes != 0 && encoding.is_none() {
            return Err(Error::new(
                Kind::TypeError,
                "decode() argument 'encoding' must be str, not None",
            ));
        }
        let encoding = encoding.unwrap_or("utf-8");
        check_option(encoding)?;
        if bytes != 0 && errors.is_none() {
            return Err(Error::new(
                Kind::TypeError,
                "decode() argument 'errors' must be str, not None",
            ));
        }
        let errors = errors.unwrap_or("replace");
        check_option(errors)?;
        let text = if input.is_empty() {
            String::new()
        } else if bytes != 0 {
            Codec::lookup(encoding)?.decode(&unquoted(input)?, Some(errors))?
        } else {
            decode_text(input, Codec::lookup(encoding)?, errors)?
        };
        crate::checked_owned_str(text.into())
    })();
    finish(result, std::ptr::null())
}
