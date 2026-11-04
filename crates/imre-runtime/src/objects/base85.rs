//! Encode each four-byte word directly into the owned bytes result.
use super::Bytes;
use super::bytes::allocate;
use super::exceptions::{Error, Result, finish};
use crate::BytesOwner;

pub(crate) fn encode(
    input: &[u8],
    alphabet: &[u8; 85],
    pad: bool,
    foldnuls: bool,
    foldspaces: bool,
) -> Result<Bytes> {
    let (words, tail) = input.as_chunks::<4>();
    let tail_width = if tail.is_empty() {
        0
    } else if pad {
        5
    } else {
        tail.len() + 1
    };
    let size = words
        .len()
        .checked_mul(5)
        .and_then(|n| n.checked_add(tail_width))
        .ok_or_else(Error::allocation)?;
    let mut output = allocate(size)?;
    let mut write = |mut word: u32, width: usize| {
        if foldnuls && word == 0 && width == 5 {
            output.push(b'z');
        } else if foldspaces && word == 0x2020_2020 {
            output.push(b'y');
        } else {
            let mut digits = [0; 5];
            for digit in digits.iter_mut().rev() {
                *digit = alphabet[(word % 85) as usize];
                word /= 85;
            }
            output.extend_from_slice(&digits[..width]);
        }
    };
    for &word in words {
        write(u32::from_be_bytes(word), 5);
    }
    if !tail.is_empty() {
        let mut word = [0; 4];
        word[..tail.len()].copy_from_slice(tail);
        write(u32::from_be_bytes(word), tail_width);
    }
    Ok(output)
}

/// Encode an exact bytes input with the fixed alphabet of a standard Base85 encoder.
///
/// # Safety
/// The caller roots the bytes owner and supplies 85 readable alphabet bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_base85_encode(
    input: *const Bytes,
    alphabet: *const [u8; 85],
    pad: bool,
    foldnuls: bool,
    foldspaces: bool,
) -> *const Bytes {
    // SAFETY: Both inputs are rooted for the call. The emitted alphabet is static.
    let (input, alphabet) = unsafe { (&*input, &*alphabet) };
    finish(
        encode(input, alphabet, pad, foldnuls, foldspaces)
            .map(|bytes| BytesOwner::into_raw(BytesOwner::new(bytes))),
        std::ptr::null(),
    )
}
