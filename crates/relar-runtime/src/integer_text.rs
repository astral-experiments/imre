//! Unicode 16.0 text properties for integer conversion, matching CPython 3.14.

use std::fmt::Write;

use crate::Str;

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
fn newer_printable(value: char) -> bool {
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
    let end = RANGES.partition_point(|(start, _)| *start <= codepoint);
    end != 0 && codepoint < RANGES[end - 1].1
}

/// Uses Ruff's quote and escape rules with Unicode 16 printability.
pub(super) fn literal_repr(value: &str) -> Str {
    let escaped = crate::objects::exceptions::string_repr(value);
    if !escaped.chars().any(newer_printable) {
        return escaped;
    }
    let mut result = String::with_capacity(escaped.len());
    for character in escaped.chars() {
        if newer_printable(character) {
            if u32::from(character) <= 0xffff {
                write!(result, "\\u{:04x}", u32::from(character))
            } else {
                write!(result, "\\U{:08x}", u32::from(character))
            }
            .expect("writing to a String cannot fail");
        } else {
            result.push(character);
        }
    }
    Str::from(result)
}
