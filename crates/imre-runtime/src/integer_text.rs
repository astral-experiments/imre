//! Unicode 16.0 text properties for integer conversion, matching CPython 3.14.

use std::fmt::{self, Write};

use crate::{Error, Kind, Str};

/// Record the interpreter setting before any user code can change the environment.
#[unsafe(no_mangle)]
pub extern "C" fn imre_integer_digits_init(isolated: u32) {
    let configured = if isolated != 0 {
        DEFAULT_DIGITS
    } else {
        std::env::var_os("PYTHONINTMAXSTRDIGITS")
            .filter(|raw| !raw.is_empty())
            .map_or(DEFAULT_DIGITS, |raw| {
                raw.to_str()
                    .and_then(|s| s.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']).parse::<i32>().ok())
                    .filter(|&n| n == 0 || n >= 640)
                    .map_or_else(
                        || {
                            eprintln!("Fatal Python error: config_init_int_max_str_digits: PYTHONINTMAXSTRDIGITS: invalid limit; must be >= 640 or 0 for unlimited.\nPython runtime state: preinitialized\n");
                            std::process::exit(1);
                        },
                        |n| n as u32,
                    )
            })
    };
    let _ = DIGITS.set(DecimalLimit::new(configured));
}

const DEFAULT_DIGITS: u32 = 4300;
static DIGITS: std::sync::OnceLock<DecimalLimit> = std::sync::OnceLock::new();

struct DecimalLimit {
    digits: u32,
    maximum_exclusive: std::sync::OnceLock<num_bigint::BigUint>,
}

impl DecimalLimit {
    fn new(digits: u32) -> Self {
        Self {
            digits,
            maximum_exclusive: std::sync::OnceLock::new(),
        }
    }
}

fn decimal_limit() -> &'static DecimalLimit {
    // Runtime unit tests may call the formatting ABI without a generated main.
    DIGITS.get_or_init(|| DecimalLimit::new(DEFAULT_DIGITS))
}

pub(super) fn decimal_digit_limit() -> u32 {
    decimal_limit().digits
}

pub(super) fn decimal_digits_allowed(count: usize) -> bool {
    let limit = decimal_digit_limit();
    limit == 0 || count <= limit as usize
}

/// Enforces the limit on a validated, unsigned ASCII digit sequence with optional separators.
pub(super) fn check_input_digits(digits: &str, limit: u32) -> crate::Result<()> {
    if limit == 0 || digits.len() <= limit as usize {
        return Ok(());
    }
    let count = digits.bytes().filter(u8::is_ascii_digit).count();
    if count <= limit as usize {
        return Ok(());
    }
    let (mut limit_buf, mut count_buf) = (itoa::Buffer::new(), itoa::Buffer::new());
    let parts = [
        "Exceeds the limit (",
        limit_buf.format(limit),
        " digits) for integer string conversion: value has ",
        count_buf.format(count),
        " digits; use sys.set_int_max_str_digits() to increase the limit",
    ];
    let mut detail = String::new();
    detail
        .try_reserve_exact(parts.iter().map(|part| part.len()).sum())
        .map_err(|_| Error::allocation())?;
    for part in parts {
        detail.push_str(part);
    }
    Err(Error::new(Kind::ValueError, detail))
}

/// A broad bit-length bound avoids allocating a configured power of ten for small inputs.
pub(super) fn check_decimal(value: &num_bigint::BigInt) -> crate::Result<()> {
    let limit = decimal_limit();
    let digits = u64::from(limit.digits);
    let bits = value.bits();
    if digits == 0 || bits <= digits * 3 {
        return Ok(());
    }
    if bits > digits * 4
        || value.magnitude()
            >= limit
                .maximum_exclusive
                .get_or_init(|| num_bigint::BigUint::from(10u8).pow(limit.digits))
    {
        Err(Error::new(
            Kind::ValueError,
            format!(
                "Exceeds the limit ({} digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit",
                limit.digits
            ),
        ))
    } else {
        Ok(())
    }
}

/// Format a selected integer only after all Python call arguments have been evaluated.
pub(super) fn decimal_string(value: &crate::Int) -> crate::Result<Str> {
    if let Some(value) = value.as_small() {
        return Ok(Str::new(itoa::Buffer::new().format(value)));
    }
    let big = value.big();
    check_decimal(&big)?;
    Ok(Str::from(big.to_string()))
}

/// Builds Python's base-ten conversion diagnostic from the original literal.
/// CPython's `%.200R` limit truncates the rendered repr by code points.
#[cold]
pub(super) fn invalid_literal(value: &str) -> Error {
    invalid_literal_in_base(value, 10)
}

/// Render an invalid integer using the base supplied by the caller.
#[cold]
pub(super) fn invalid_literal_in_base(value: &str, base: u32) -> Error {
    let literal = literal_repr_prefix(value);
    Error::new(
        Kind::ValueError,
        compact_str::format_compact!("invalid literal for int() with base {base}: {literal}"),
    )
}

/// Returns the decimal value of a Unicode 16.0 digit.
///
/// Each entry is a zero followed by nine consecutive decimal digits. The table
/// is reproducible with CPython 3.14's `unicodedata.decimal(chr(cp), None) == 0`
/// over `range(0x110000)`. Pinning it excludes digits added by newer ICU data.
pub(super) fn decimal_digit(value: char) -> Option<u8> {
    #[rustfmt::skip]
    const ZEROES: &[u32] = &[
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6,
        0xb66, 0xbe6, 0xc66, 0xce6, 0xd66, 0xde6, 0xe50, 0xed0,
        0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80,
        0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900,
        0xa9d0, 0xa9f0, 0xaa50, 0xabf0, 0xff10, 0x104a0, 0x10d30, 0x10d40,
        0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0, 0x11650,
        0x116c0, 0x116d0, 0x116da, 0x11730, 0x118e0, 0x11950, 0x11bf0, 0x11c50,
        0x11d50, 0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70,
        0x1ccf0, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0,
        0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
    ];
    let codepoint = u32::from(value);
    let end = ZEROES.partition_point(|zero| *zero <= codepoint);
    let digit = codepoint.checked_sub(*ZEROES.get(end.checked_sub(1)?)?)?;
    (digit < 10).then_some(digit as u8)
}

/// Identifies characters printable in locked ICU 2.3 data but not Unicode 16.
///
/// These sorted half-open ranges are the complete difference between Ruff's
/// ICU general-category printability predicate and CPython 3.14 `isprintable()`
/// over all Unicode scalar values. There are no differences in the other
/// direction. Recheck this table when updating ICU or the Python target version.
pub(super) fn newer_printable(value: char) -> bool {
    #[rustfmt::skip]
    const RANGES: &[(u32, u32)] = &[
        (0x88f, 0x890), (0xc5c, 0xc5d), (0xcdc, 0xcdd),
        (0x1acf, 0x1ade), (0x1ae0, 0x1aec), (0x20c1, 0x20c2),
        (0x2b96, 0x2b97), (0xa7ce, 0xa7d0), (0xa7d2, 0xa7d3),
        (0xa7d4, 0xa7d5), (0xa7f1, 0xa7f2), (0xfbc3, 0xfbd3),
        (0xfd90, 0xfd92), (0xfdc8, 0xfdcf), (0x10940, 0x1095a),
        (0x10ec5, 0x10ec8), (0x10ed0, 0x10ed9), (0x10efa, 0x10efc),
        (0x11b60, 0x11b68), (0x11db0, 0x11ddc), (0x11de0, 0x11dea),
        (0x16ea0, 0x16eb9), (0x16ebb, 0x16ed4), (0x16ff2, 0x16ff7),
        (0x187f8, 0x18800), (0x18d09, 0x18d1f), (0x18d80, 0x18df3),
        (0x1ccfa, 0x1ccfd), (0x1ceba, 0x1ced1), (0x1cee0, 0x1cef1),
        (0x1e6c0, 0x1e6df), (0x1e6e0, 0x1e6f6), (0x1e6fe, 0x1e700),
        (0x1f6d8, 0x1f6d9), (0x1f777, 0x1f77b), (0x1f8d0, 0x1f8d9),
        (0x1fa54, 0x1fa58), (0x1fa8a, 0x1fa8b), (0x1fa8e, 0x1fa8f),
        (0x1fac8, 0x1fac9), (0x1facd, 0x1face), (0x1faea, 0x1faeb),
        (0x1faef, 0x1faf0), (0x1fbfa, 0x1fbfb), (0x2b73a, 0x2b740),
        (0x2cea2, 0x2ceae), (0x323b0, 0x3347a),
    ];
    let codepoint = u32::from(value);
    if codepoint < RANGES[0].0 {
        return false;
    }
    let end = RANGES.partition_point(|(start, _)| *start <= codepoint);
    end != 0 && codepoint < RANGES[end - 1].1
}

/// Stop the repr writer after the diagnostic's 200 Unicode code points.
fn literal_repr_prefix(value: &str) -> Str {
    struct Prefix {
        text: Str,
        remaining: usize,
    }

    impl Write for Prefix {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            for character in text.chars() {
                if self.remaining == 0 {
                    return Err(fmt::Error);
                }
                self.text.push(character);
                self.remaining -= 1;
            }
            Ok(())
        }
    }

    let mut prefix = Prefix {
        text: Str::default(),
        remaining: 200,
    };
    // Quote selection still examines the whole source. Only rendering is bounded.
    let _ = crate::objects::exceptions::write_string_repr(value, &mut prefix);
    prefix.text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_literal_prefix_preserves_late_quotes_and_partial_escapes() {
        let nul = "\0".repeat(16_384) + "'";
        let expected = format!("\"{}", "\\x00".repeat(50));
        assert_eq!(literal_repr_prefix(&nul), &expected[..200]);

        let unicode = "\u{1ccfa}".repeat(1_024);
        let expected = format!("'{}", "\\U0001ccfa".repeat(20));
        assert_eq!(literal_repr_prefix(&unicode), &expected[..200]);
    }
}
