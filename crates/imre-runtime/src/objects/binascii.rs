//! Binary-to-text codecs and checksums over byte slices.

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

/// Return the low 32 bits of a Python integer checksum seed.
pub(crate) fn checksum_seed(seed: &crate::Int) -> u32 {
    if let Some(small) = seed.as_small() {
        small as u32
    } else {
        let big = seed.big();
        let low = big.magnitude().iter_u32_digits().next().unwrap_or(0);
        if big.sign() == num_bigint::Sign::Minus {
            low.wrapping_neg()
        } else {
            low
        }
    }
}

/// Compute the unsigned ZIP-compatible CRC over a borrowed buffer.
pub(crate) fn crc32(data: &[u8], seed: &crate::Int) -> u32 {
    zlib_rs::crc32::crc32(checksum_seed(seed), data)
}

/// Compute zlib's unsigned Adler-32 over a borrowed buffer and an arbitrary Python integer seed.
pub(crate) fn adler32(data: &[u8], seed: &crate::Int) -> u32 {
    let seed = checksum_seed(seed);
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

const fn decode_digits(urlsafe: bool) -> [u8; 256] {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut digits = [u8::MAX; 256];
    let mut i = 0;
    while i < alphabet.len() {
        digits[alphabet[i] as usize] = i as u8;
        i += 1;
    }
    if urlsafe {
        // Python accepts both spellings of the last two digits in URL-safe input.
        digits[b'-' as usize] = 62;
        digits[b'_' as usize] = 63;
    }
    digits
}

fn unpack_digits([a, b, c, d]: [u8; 4]) -> [u8; 3] {
    [a << 2 | b >> 4, b << 4 | c >> 2, c << 6 | d]
}

fn decode(data: &[u8], strict: bool, urlsafe: bool) -> Result<Bytes> {
    const STANDARD_DIGITS: [u8; 256] = decode_digits(false);
    const URLSAFE_DIGITS: [u8; 256] = decode_digits(true);
    let digits = if urlsafe {
        &URLSAFE_DIGITS
    } else {
        &STANDARD_DIGITS
    };
    if strict {
        decode_groups::<true>(data, digits)
    } else {
        decode_groups::<false>(data, digits)
    }
}

fn decode_groups<const STRICT: bool>(data: &[u8], digits: &[u8; 256]) -> Result<Bytes> {
    let mut decoded = allocate(data.len().div_ceil(4) * 3)?;
    let mut input = data;
    while !input.is_empty() {
        while let Some((raw, rest)) = input.split_first_chunk::<4>() {
            let group @ [a, b, c, d] = raw.map(|byte| digits[usize::from(byte)]);
            if a | b | c | d == u8::MAX {
                if rest.is_empty() && raw[3] == b'=' && a | b != u8::MAX {
                    let len = if raw[2] == b'=' {
                        1
                    } else if c != u8::MAX {
                        2
                    } else {
                        break;
                    };
                    decoded.extend_from_slice(&unpack_digits(group)[..len]);
                    return Ok(decoded);
                }
                break;
            }
            decoded.extend_from_slice(&unpack_digits(group));
            input = rest;
        }

        // Gather a group across ignored bytes. Resume contiguous decoding at its end.
        let mut group = [0; 4];
        let mut used: usize = 0;
        let mut pads = 0;
        while used < group.len() {
            let Some((&byte, rest)) = input.split_first() else {
                break;
            };
            let first = input.len() == data.len();
            input = rest;
            let value = digits[usize::from(byte)];
            if value != u8::MAX {
                if STRICT && pads != 0 {
                    let message = if used + pads == 4 {
                        "Excess data after padding"
                    } else {
                        "Discontinuous padding not allowed"
                    };
                    return Err(Error::new(Kind::BinasciiError, message));
                }
                group[used] = value;
                used += 1;
                pads = 0;
                continue;
            }
            if byte == b'=' {
                pads += 1;
                if STRICT && (used < 2 || used + pads > 4) {
                    if used == 1 {
                        break; // The length error takes precedence over bad padding.
                    }
                    let message = if first {
                        "Leading padding not allowed"
                    } else {
                        "Excess padding not allowed"
                    };
                    return Err(Error::new(Kind::BinasciiError, message));
                }
            } else if STRICT {
                return Err(Error::new(
                    Kind::BinasciiError,
                    "Only base64 data is allowed",
                ));
            }
        }
        if used == 1 {
            let count = decoded.len() / 3 * 4 + 1;
            return Err(Error::new(
                Kind::BinasciiError,
                format!(
                    "Invalid base64-encoded string: number of data characters ({count}) cannot be 1 more than a multiple of 4"
                ),
            ));
        }
        if used != 0 && used + pads < 4 {
            return Err(Error::new(Kind::BinasciiError, "Incorrect padding"));
        }
        decoded.extend_from_slice(&unpack_digits(group)[..used.saturating_sub(1)]);
    }
    Ok(decoded)
}

/// Decode quoted-printable bytes, including the permissive escapes accepted by binascii.
pub(crate) fn decode_quoted_printable(mut input: &[u8], header: bool) -> Result<Bytes> {
    use super::text::hexadecimal;

    let mut output = allocate(input.len())?;
    loop {
        match input {
            [] | [b'='] => break,
            [b'=', b'\n', tail @ ..] => input = tail,
            // A soft break starting with CR consumes the rest of the line, even without LF.
            [b'=', b'\r', tail @ ..] => {
                let Some(end) = tail.iter().position(|&byte| byte == b'\n') else {
                    break;
                };
                input = &tail[end + 1..];
            }
            [b'=', b'=', tail @ ..] => {
                output.push(b'=');
                input = tail;
            }
            [b'=', high, low, tail @ ..]
                if let (Some(high), Some(low)) = (hexadecimal(*high), hexadecimal(*low)) =>
            {
                output.push(high << 4 | low);
                input = tail;
            }
            [byte, tail @ ..] => {
                output.push(if header && *byte == b'_' { b' ' } else { *byte });
                input = tail;
            }
        }
    }
    Ok(output)
}

/// Encode bytes using binascii's 76-column quoted-printable line rules.
pub(crate) fn encode_quoted_printable(
    mut input: &[u8],
    quotetabs: bool,
    istext: bool,
    header: bool,
) -> Result<Bytes> {
    // The first LF selects the ending for all hard and soft breaks in this input.
    let newline: &[u8] = match input.iter().position(|&byte| byte == b'\n') {
        Some(i) if i > 0 && input[i - 1] == b'\r' => b"\r\n",
        _ => b"\n",
    };
    let escape = |byte: u8| {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        [
            b'=',
            HEX[usize::from(byte >> 4)],
            HEX[usize::from(byte & 15)],
        ]
    };
    let mut output = allocate(input.len())?;
    // A literal trailing tab or space in a full 76-column line can grow by two bytes.
    let mut line = [0; 78];
    let mut column = 0;
    let plain = istext && !quotetabs && !header;
    while let Some((&byte, rest)) = input.split_first() {
        if plain && column <= 3 && input.len() > 8 && (column != 0 || byte != b'.') {
            // Leave the wrapping column and EOF to the scalar rules.
            let limit = (75 - column).min(input.len() - 1);
            let run = input[..limit]
                .iter()
                .position(|&b| !(b' '..=b'~').contains(&b) || b == b'=')
                .unwrap_or(limit);
            if run >= 8 {
                line[column..column + run].copy_from_slice(&input[..run]);
                column += run;
                input = &input[run..];
                continue;
            }
        }
        input = rest;
        let next = rest.first().copied();
        if istext && (byte == b'\n' || (byte == b'\r' && next == Some(b'\n'))) {
            if byte == b'\r' {
                input = &rest[1..];
            }
            // Header spaces have already become underscores. Only a literal trailing
            // space or tab in the output needs protection before the hard break.
            if let Some(&last @ (b' ' | b'\t')) = line[..column].last() {
                line[column - 1..column + 2].copy_from_slice(&escape(last));
                column += 2;
            }
            append_quoted_line(&mut output, &line[..column], newline)?;
            column = 0;
            continue;
        }

        let quote = match byte {
            b'=' => true,
            b'_' => header,
            b'.' => column == 0 && matches!(next, None | Some(0 | b'\r' | b'\n')),
            b' ' | b'\t' => quotetabs || next.is_none(),
            b'\r' | b'\n' => !istext,
            _ => !(b' '..=b'~').contains(&byte),
        };
        let width = if quote { 3 } else { 1 };
        let wrap = column + width >= 76 && (quote || !matches!(next, None | Some(b'\n')));
        if wrap {
            line[column] = b'=';
            append_quoted_line(&mut output, &line[..column + 1], newline)?;
            column = 0;
        }
        if quote {
            line[column..column + width].copy_from_slice(&escape(byte));
        } else {
            line[column] = if header && byte == b' ' { b'_' } else { byte };
        }
        column += width;
    }
    append_quoted_line(&mut output, &line[..column], &[])?;
    Ok(output)
}

/// Append a complete encoded line and its selected line ending with one capacity check.
fn append_quoted_line(output: &mut Bytes, line: &[u8], ending: &[u8]) -> Result<()> {
    output
        .try_reserve(line.len() + ending.len())
        .map_err(|_| Error::allocation())?;
    output.extend_from_slice(line);
    output.extend_from_slice(ending);
    Ok(())
}

/// Decode ASCII hex pairs with binascii's strict length and alphabet rules.
pub(crate) fn unhexlify(data: &[u8]) -> Result<Bytes> {
    let (pairs, remainder) = data.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(Error::new(Kind::BinasciiError, "Odd-length string"));
    }
    let mut decoded = allocate(pairs.len())?;
    let (blocks, tail) = pairs.as_chunks::<16>();
    for block in blocks {
        // Classify consecutive input bytes before combining adjacent nibbles.
        let mut nibbles = [0; 32];
        let mut seen = 0;
        for (nibble, &byte) in nibbles.iter_mut().zip(block.as_flattened()) {
            *nibble = super::text::hexadecimal(byte).unwrap_or(u8::MAX);
            seen |= *nibble;
        }
        check_hex_digits(seen)?;
        let mut bytes = [0; 16];
        for (byte, &[high, low]) in bytes.iter_mut().zip(nibbles.as_chunks::<2>().0) {
            *byte = high << 4 | low;
        }
        decoded.extend_from_slice(&bytes);
    }

    // A partial block needs at most fifteen scalar pairs.
    const DIGITS: [u8; 256] = {
        let mut values = [u8::MAX; 256];
        let mut byte = 0;
        while byte < values.len() {
            if let Some(value) = super::text::hexadecimal(byte as u8) {
                values[byte] = value;
            }
            byte += 1;
        }
        values
    };
    let mut bytes = [0; 16];
    let mut seen = 0;
    for (byte, &[high, low]) in bytes.iter_mut().zip(tail) {
        let high = DIGITS[usize::from(high)];
        let low = DIGITS[usize::from(low)];
        seen |= high | low;
        *byte = high << 4 | low;
    }
    check_hex_digits(seen)?;
    decoded.extend_from_slice(&bytes[..tail.len()]);
    Ok(decoded)
}

fn check_hex_digits(seen: u8) -> Result<()> {
    if seen == u8::MAX {
        return Err(Error::new(
            Kind::BinasciiError,
            "Non-hexadecimal digit found",
        ));
    }
    Ok(())
}

/// Decode the hexadecimal data in a rooted immutable bytes object.
///
/// # Safety
/// The caller roots an exact bytes owner throughout the operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binascii_unhexlify(value: *const Bytes) -> *const Bytes {
    // SAFETY: the caller supplied a rooted bytes owner.
    finish(unhexlify(unsafe { &*value }).map(owned), std::ptr::null())
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

/// Borrow the ASCII input accepted by Python's public binary-to-text decoders.
pub(super) fn ascii_bytes(text: &str) -> Result<&[u8]> {
    if text.is_ascii() {
        Ok(text.as_bytes())
    } else {
        Err(Error::new(
            Kind::ValueError,
            "string argument should contain only ASCII characters",
        ))
    }
}

/// # Safety
/// The caller roots the immutable string throughout validation and copying.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binascii_ascii_input(value: *const Str) -> *const Bytes {
    // SAFETY: the caller supplied an exact str owner.
    let data = ascii_bytes(unsafe { &*value });
    let result = data.and_then(|data| {
        let mut bytes = allocate(data.len())?;
        bytes.extend_from_slice(data);
        Ok(owned(bytes))
    });
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
