//! Base32 uses five input bytes per eight alphabet digits.
use super::Bytes;
use super::bytes::allocate;
use super::exceptions::{Error, Kind, Result, finish};
use crate::{BytesOwner, Str};

pub(crate) const ALPHABETS: [[u8; 32]; 2] = [
    *b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567",
    *b"0123456789ABCDEFGHIJKLMNOPQRSTUV",
];
const LOWER: u8 = 32;
const INVALID: u8 = 128;

/// The low five bits hold the digit; the high bits distinguish lowercase and invalid input.
const fn digits(alphabet: &[u8; 32]) -> [u8; 256] {
    let mut table = [INVALID; 256];
    let mut i = 0;
    while i < alphabet.len() {
        let byte = alphabet[i];
        table[byte as usize] = i as u8;
        if byte.is_ascii_uppercase() {
            table[byte.to_ascii_lowercase() as usize] = i as u8 | LOWER;
        }
        i += 1;
    }
    table
}
const DIGITS: [[u8; 256]; 2] = [digits(&ALPHABETS[0]), digits(&ALPHABETS[1])];

pub(crate) fn encode(data: &[u8], hex: bool) -> Result<Bytes> {
    let alphabet = &ALPHABETS[usize::from(hex)];
    let size = data
        .len()
        .div_ceil(5)
        .checked_mul(8)
        .ok_or_else(Error::allocation)?;
    let mut output = allocate(size)?;
    let word = |[a, b, c, d, e]: [u8; 5]| {
        let mut value = u64::from_be_bytes([0, 0, 0, a, b, c, d, e]);
        let mut encoded = [0; 8];
        for digit in encoded.iter_mut().rev() {
            *digit = alphabet[(value & 31) as usize];
            value >>= 5;
        }
        encoded
    };
    let (words, tail) = data.as_chunks::<5>();
    for &chunk in words {
        output.extend_from_slice(&word(chunk));
    }
    if !tail.is_empty() {
        let mut chunk = [0; 5];
        chunk[..tail.len()].copy_from_slice(tail);
        let mut encoded = word(chunk);
        encoded[(tail.len() * 8).div_ceil(5)..].fill(b'=');
        output.extend_from_slice(&encoded);
    }
    Ok(output)
}

pub(crate) fn decode(input: &[u8], hex: bool, casefold: bool) -> Result<Bytes> {
    let bad_padding = || Error::new(Kind::BinasciiError, "Incorrect padding");
    if !input.len().is_multiple_of(8) {
        return Err(bad_padding());
    }
    let end = input.iter().rposition(|&b| b != b'=').map_or(0, |i| i + 1);
    let (words, tail) = input[..end].as_chunks::<8>();
    let mut output = allocate(words.len() * 5 + tail.len() * 5 / 8)?;
    let table = &DIGITS[usize::from(hex)];
    let reject = if casefold { INVALID } else { INVALID | LOWER };
    let mut write = |digits: &[u8]| -> Result<()> {
        let mut word = 0u64;
        let mut flags = 0;
        for &byte in digits {
            let digit = table[byte as usize];
            flags |= digit;
            word = (word << 5) | u64::from(digit & 31);
        }
        if flags & reject != 0 {
            return Err(Error::new(Kind::BinasciiError, "Non-base32 digit found"));
        }
        let aligned = word << (5 * (8 - digits.len()));
        output.extend_from_slice(&aligned.to_be_bytes()[3..3 + digits.len() * 5 / 8]);
        Ok(())
    };
    for word in words {
        write(word)?;
    }
    if !tail.is_empty() {
        write(tail)?;
    }
    // CPython checks all digits before rejecting an invalid trailing pad count.
    if !matches!(input.len() - end, 0 | 1 | 3 | 4 | 6) {
        return Err(bad_padding());
    }
    Ok(output)
}

fn result(bytes: Result<Bytes>) -> *const Bytes {
    finish(
        bytes.map(|b| BytesOwner::into_raw(BytesOwner::new(b))),
        std::ptr::null(),
    )
}

/// Encode a rooted immutable bytes buffer with the standard or extended hex alphabet.
///
/// # Safety
/// The caller roots the input owner for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_base32_encode(data: *const Bytes, hex: bool) -> *const Bytes {
    // SAFETY: the caller keeps the bytes owner rooted.
    result(encode(unsafe { &*data }, hex))
}

/// Decode a rooted immutable bytes buffer, preserving digit and padding errors.
///
/// # Safety
/// The caller roots the input owner for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_base32_decode(
    data: *const Bytes,
    hex: bool,
    casefold: bool,
) -> *const Bytes {
    // SAFETY: the caller keeps the bytes owner rooted.
    result(decode(unsafe { &*data }, hex, casefold))
}

/// Decode an exact string after validating that its borrowed storage is ASCII.
///
/// # Safety
/// The caller roots the string owner throughout validation and decoding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_base32_decode_text(
    data: *const Str,
    hex: bool,
    casefold: bool,
) -> *const Bytes {
    // SAFETY: the caller keeps the string owner rooted.
    let data = super::binascii::ascii_bytes(unsafe { &*data });
    result(data.and_then(|data| decode(data, hex, casefold)))
}
