//! CPython's hexadecimal float grammar and single round-to-nearest-even conversion.

use crate::{Error, Kind, Result};

fn invalid() -> Error {
    Error::new(
        Kind::ValueError,
        "invalid hexadecimal floating-point string",
    )
}

fn overflow() -> Error {
    Error::new(
        Kind::OverflowError,
        "hexadecimal value too large to represent as a float",
    )
}

/// Round the exact coefficient times 2**exponent to the binary64 grid.
/// The coefficient holds its most significant bits. A nonzero discarded tail
/// distinguishes a midpoint from a value above it.
fn rounded(high: u64, exponent: i128, sticky: bool) -> Result<f64> {
    if high == 0 {
        return Ok(0.0);
    }
    let top = exponent + i128::from(high.ilog2());
    if top < -1075 {
        return Ok(0.0);
    }
    if top > 1023 {
        return Err(overflow());
    }
    let unit = (top - 52).max(-1074);
    let shift = unit - exponent;
    let high = u128::from(high);
    let rounded = if shift > 0 {
        let half = 1_u128 << (shift - 1);
        let rest = high & (2 * half - 1);
        let retained = high >> shift;
        retained + u128::from(rest > half || rest == half && (sticky || retained & 1 != 0))
    } else {
        high << -shift
    };
    // For normal values the leading significand bit supplies one exponent
    // unit. Adding also carries rounding into the next binade when necessary.
    let bits = (((unit + 1074) as u64) << 52) + rounded as u64;
    let value = f64::from_bits(bits);
    if value.is_infinite() {
        Err(overflow())
    } else {
        Ok(value)
    }
}

pub(crate) fn parse_hex(text: &str) -> Result<f64> {
    let text = text.trim_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    let negative = text.starts_with('-');
    if unsigned.eq_ignore_ascii_case("inf") || unsigned.eq_ignore_ascii_case("infinity") {
        return Ok(if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    if unsigned.eq_ignore_ascii_case("nan") {
        return Ok(f64::NAN.copysign(if negative { -1.0 } else { 1.0 }));
    }
    let mut input = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
        .unwrap_or(unsigned)
        .as_bytes();
    let mut high = 0_u64;
    let mut exponent = 0_i128;
    let mut sticky = false;
    let mut fraction = false;
    let mut saw_digit = false;
    while let Some((&byte, tail)) = input.split_first() {
        if let Some(digit) = crate::objects::hexadecimal(byte) {
            saw_digit = true;
            if fraction {
                exponent -= 4;
            }
            if high <= u64::MAX >> 4 {
                high = (high << 4) | u64::from(digit);
            } else {
                // At least 61 leading bits are already available for rounding.
                sticky |= digit != 0;
                exponent += 4;
            }
        } else if byte == b'.' && !fraction {
            fraction = true;
        } else {
            break;
        }
        input = tail;
    }
    if !saw_digit {
        return Err(invalid());
    }
    if matches!(input.first(), Some(b'p' | b'P')) {
        input = &input[1..];
        let minus = input.first() == Some(&b'-');
        if minus || input.first() == Some(&b'+') {
            input = &input[1..];
        }
        if !input.first().is_some_and(u8::is_ascii_digit) {
            return Err(invalid());
        }
        let mut power = 0_i64;
        while let Some((&digit @ b'0'..=b'9', tail)) = input.split_first() {
            power = power
                .saturating_mul(10)
                .saturating_add(i64::from(digit - b'0'));
            input = tail;
        }
        exponent += i128::from(if minus { -power } else { power });
    }
    let value = rounded(high, exponent, sticky)?;
    if !input.is_empty() {
        return Err(invalid());
    }
    Ok(if negative { -value } else { value })
}
