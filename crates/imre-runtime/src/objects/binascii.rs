//! Standard and URL-safe base64 over rooted immutable inputs.

use base64::Engine;
use base64::engine::general_purpose::{GeneralPurpose, STANDARD, URL_SAFE};

use super::bytes::allocate;
use super::exceptions::{Error, Kind, Result, finish};
use super::{Bytes, Str};
use crate::BytesOwner;

fn owned(bytes: Bytes) -> *const Bytes {
    BytesOwner::into_raw(BytesOwner::new(bytes))
}

fn encode(data: &[u8], alphabet: &GeneralPurpose, newline: bool) -> Result<*const Bytes> {
    let n = base64::encoded_len(data.len(), true).ok_or_else(Error::allocation)?;
    let mut encoded = allocate(
        n.checked_add(usize::from(newline))
            .ok_or_else(Error::allocation)?,
    )?;
    encoded.resize(n, 0);
    alphabet
        .encode_slice(data, &mut encoded)
        .expect("exact base64 capacity");
    if newline {
        encoded.push(b'\n');
    }
    Ok(owned(encoded))
}

fn digit(byte: u8) -> Option<u8> {
    Some(match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    })
}

fn decode(data: &[u8], strict: bool) -> Result<*const Bytes> {
    let mut decoded = allocate(data.len().div_ceil(4) * 3)?;
    let mut count = 0;
    let mut pads = 0;
    let mut remaining = 0;
    for (index, &byte) in data.iter().enumerate() {
        let position = count % 4;
        if byte == b'=' {
            pads += 1;
            if !strict || (position >= 2 && position + pads <= 4) {
                continue;
            }
            if position == 1 {
                break; // The length error below takes precedence over bad padding.
            }
            let message = if index == 0 {
                "Leading padding not allowed"
            } else {
                "Excess padding not allowed"
            };
            return Err(Error::new(Kind::BinasciiError, message));
        }
        let Some(value) = digit(byte) else {
            if strict {
                return Err(Error::new(
                    Kind::BinasciiError,
                    "Only base64 data is allowed",
                ));
            }
            continue;
        };
        if strict && pads != 0 {
            let message = if position + pads == 4 {
                "Excess data after padding"
            } else {
                "Discontinuous padding not allowed"
            };
            return Err(Error::new(Kind::BinasciiError, message));
        }
        pads = 0;
        if position != 0 {
            decoded.push((remaining << (2 * position)) | (value >> (6 - 2 * position)));
        }
        remaining = value;
        count += 1;
    }
    let position = count % 4;
    if position == 1 {
        return Err(Error::new(
            Kind::BinasciiError,
            format!(
                "Invalid base64-encoded string: number of data characters ({count}) cannot be 1 more than a multiple of 4"
            ),
        ));
    }
    if position != 0 && position + pads < 4 {
        return Err(Error::new(Kind::BinasciiError, "Incorrect padding"));
    }
    Ok(owned(decoded))
}

/// # Safety
/// The caller roots the immutable string throughout validation and copying.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binascii_ascii_input(value: *const Str) -> *const Bytes {
    // SAFETY: the caller supplied an exact str owner.
    let data = unsafe { (&*value).as_bytes() };
    let result = if data.is_ascii() {
        allocate(data.len()).map(|mut bytes| {
            bytes.extend_from_slice(data);
            owned(bytes)
        })
    } else {
        Err(Error::new(
            Kind::ValueError,
            "string argument should contain only ASCII characters",
        ))
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// The caller roots the immutable bytes. The result owns its output, including padding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binascii_encode(value: *const Bytes, newline: bool) -> *const Bytes {
    // SAFETY: the caller supplied an exact bytes owner.
    finish(
        encode(unsafe { &*value }, &STANDARD, newline),
        std::ptr::null(),
    )
}

/// # Safety
/// The caller roots the immutable bytes through the whole decoding operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binascii_decode(value: *const Bytes, strict: bool) -> *const Bytes {
    // SAFETY: the caller supplied an exact bytes owner.
    finish(decode(unsafe { &*value }, strict), std::ptr::null())
}

/// # Safety
/// The caller roots the immutable byte input. The result owns its bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_urlsafe_b64encode(value: *const Bytes) -> *const Bytes {
    // SAFETY: the caller supplied an exact bytes owner.
    finish(
        encode(unsafe { &*value }, &URL_SAFE, false),
        std::ptr::null(),
    )
}
