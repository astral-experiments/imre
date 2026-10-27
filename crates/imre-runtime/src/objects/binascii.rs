//! Base64 and checksums over rooted immutable inputs.

use base64::Engine;
use base64::engine::general_purpose::{GeneralPurpose, STANDARD, URL_SAFE};

use super::bytes::allocate;
use super::exceptions::{Error, Kind, Result, finish};
use super::{Bytes, Str};
use crate::BytesOwner;

/// Compute the unsigned Adler-32 of immutable bytes from an exact Python integer seed.
///
/// # Safety
/// The caller roots the byte buffer and the tagged integer for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zlib_adler32(data: *const Bytes, seed: usize) -> usize {
    // SAFETY: both inputs stay rooted and neither can change during the checksum.
    let (data, seed) = unsafe { (&*data, crate::Int::borrow(seed)) };
    crate::Int::from_usize(adler32(data, &seed) as usize).into_raw()
}

/// Compute zlib's unsigned Adler-32 over a borrowed buffer and an arbitrary Python integer seed.
pub(crate) fn adler32(data: &[u8], seed: &crate::Int) -> u32 {
    let seed = if let Some(small) = seed.as_small() {
        small as u32
    } else {
        let big = seed.big();
        let low = big.magnitude().iter_u32_digits().next().unwrap_or(0);
        if big.sign() == num_bigint::Sign::Minus {
            low.wrapping_neg()
        } else {
            low
        }
    };
    const BASE: u32 = 65_521;
    if data.len() == 1 {
        // Match zlib's observable one-byte fast path, including its single
        // subtraction for each noncanonical seed half.
        let mut sum1 = (seed & 0xffff) + u32::from(data[0]);
        if sum1 >= BASE {
            sum1 -= BASE;
        }
        let mut sum2 = (seed >> 16) + sum1;
        if sum2 >= BASE {
            sum2 -= BASE;
        }
        (sum2 << 16) | sum1
    } else {
        // zlib normalizes both halves for empty and multi-byte inputs.
        let seed = (((seed >> 16) % BASE) << 16) | ((seed & 0xffff) % BASE);
        zlib_rs::adler32::adler32(seed, data)
    }
}

fn owned(bytes: Bytes) -> *const Bytes {
    BytesOwner::into_raw(BytesOwner::new(bytes))
}

fn encode(data: &[u8], alphabet: &GeneralPurpose, newline: bool) -> Result<Bytes> {
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
    Ok(encoded)
}

fn digit(byte: u8, urlsafe: bool) -> Option<u8> {
    Some(match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        b'-' if urlsafe => 62,
        b'_' if urlsafe => 63,
        _ => return None,
    })
}

fn decode(data: &[u8], strict: bool, urlsafe: bool) -> Result<Bytes> {
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
        let Some(value) = digit(byte, urlsafe) else {
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
    Ok(decoded)
}

pub(crate) fn binascii_encode(data: &[u8], newline: bool) -> Result<Bytes> {
    encode(data, &STANDARD, newline)
}

pub(crate) fn binascii_decode(data: &[u8], strict: bool) -> Result<Bytes> {
    decode(data, strict, false)
}

pub(crate) fn urlsafe_encode(data: &[u8]) -> Result<Bytes> {
    encode(data, &URL_SAFE, false)
}

pub(crate) fn base64_decode(data: &[u8], urlsafe: bool) -> Result<Bytes> {
    decode(data, false, urlsafe)
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
        binascii_encode(unsafe { &*value }, newline).map(owned),
        std::ptr::null(),
    )
}

/// # Safety
/// The caller roots the immutable bytes through the whole decoding operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binascii_decode(value: *const Bytes, strict: bool) -> *const Bytes {
    // SAFETY: the caller supplied an exact bytes owner.
    finish(
        binascii_decode(unsafe { &*value }, strict).map(owned),
        std::ptr::null(),
    )
}

/// # Safety
/// The caller roots the immutable byte input. The result owns its bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_urlsafe_b64encode(value: *const Bytes) -> *const Bytes {
    // SAFETY: the caller supplied an exact bytes owner.
    finish(
        urlsafe_encode(unsafe { &*value }).map(owned),
        std::ptr::null(),
    )
}

/// # Safety
/// `data` and `alphabet` are rooted bytes owners. Translation preserves CPython's
/// last assignment when the two source alphabet bytes are equal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_base64_translate(
    data: *const Bytes,
    alphabet: *const Bytes,
    decode: bool,
) -> *const Bytes {
    // SAFETY: both inputs are exact, immutable bytes throughout the operation.
    let (data, alphabet) = unsafe { (&*data, &*alphabet) };
    let result = (|| {
        let &[first, second] = alphabet.as_slice() else {
            return Err(Error::new(
                Kind::AssertionError,
                super::bytes::repr(alphabet)?,
            ));
        };
        let (from, to) = if decode {
            ([first, second], *b"+/")
        } else {
            (*b"+/", [first, second])
        };
        let mut result = allocate(data.len())?;
        result.extend(data.iter().map(|&c| {
            if c == from[1] {
                to[1]
            } else if c == from[0] {
                to[0]
            } else {
                c
            }
        }));
        Ok(owned(result))
    })();
    finish(result, std::ptr::null())
}
