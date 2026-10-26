//! Binary64 operations that need Python's checks or arbitrary-precision integers.
//! Ordinary float values are unboxed; only present `float | None` values own an
//! `Rc<f64>`. A nullable pointer preserves every bit pattern without reserving a
//! NaN payload or conflating signed zero with absence.

use crate::StrOwner;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::rc::Rc;

use num_bigint::BigInt;
use num_traits::{FromPrimitive, Signed, ToPrimitive, Zero};

use super::{Bytes, Error, Int, Kind, Result, Str, finish, output_text};
use crate::integer_text::decimal_digit;

/// Returns a nullable-word owner. Bare float storage never calls this helper.
#[unsafe(no_mangle)]
pub extern "C" fn imre_float_box(value: f64) -> usize {
    Rc::into_raw(Rc::new(value)) as usize
}

/// # Safety
/// `value` is a rooted, present optional-float word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_float_unbox(value: usize) -> f64 {
    // SAFETY: the caller supplies a live allocation of the exact payload type.
    unsafe { *(value as *const f64) }
}

/// # Safety
/// `value` is a rooted optional-float word, or zero for None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_float_box_retain(value: usize) {
    if value != 0 {
        // SAFETY: the caller roots a present optional-float allocation.
        unsafe { Rc::increment_strong_count(value as *const f64) };
    }
}

/// # Safety
/// Transfers one optional-float reference, or accepts zero for None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_float_box_release(value: usize) {
    if value != 0 {
        // SAFETY: the caller transfers the reference being released.
        unsafe { Rc::decrement_strong_count(value as *const f64) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_float_str(value: f64) -> *const Str {
    StrOwner::into_raw(StrOwner::new(render(value)))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_print_float(value: f64) {
    output_text(&render(value));
}

/// Ryu chooses shortest digits with ties to even; Rust's formatter, currently
/// used by Ruff for finite values, instead prints 900719925474099.25 as a final
/// `.3` where Python chooses `.2`. Keep Ruff's nonfinite spelling and adapt Ryu's
/// finite exponent notation: Python switches at 1e-5 and always writes its sign
/// and at least two exponent digits.
pub(crate) fn render(value: f64) -> Str {
    if !value.is_finite() {
        return ruff_python_literal::float::to_string(value).into();
    }
    let mut buffer = ryu::Buffer::new();
    let raw = buffer.format_finite(value);
    if let Some((significand, exponent)) = raw.split_once('e') {
        let exponent = exponent.parse::<i32>().unwrap();
        return compact_str::format_compact!("{significand}e{exponent:+03}");
    }
    let unsigned = raw.strip_prefix('-').unwrap_or(raw);
    if let Some(digits) = unsigned.strip_prefix("0.0000") {
        let mut result = Str::new(if value.is_sign_negative() { "-" } else { "" });
        result.push(char::from(digits.as_bytes()[0]));
        if digits.len() > 1 {
            result.push('.');
            result.push_str(&digits[1..]);
        }
        result.push_str("e-05");
        return result;
    }
    raw.into()
}

/// # Safety
/// `value` is a rooted canonical Int handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_float(value: usize) -> f64 {
    // SAFETY: this helper borrows the caller's rooted Int.
    let value = unsafe { Int::borrow(value) };
    crate::finish(int_to_float(&value), 0.0)
}

pub(super) fn int_to_float(value: &Int) -> Result<f64> {
    if let Some(value) = value.as_small() {
        return Ok(value as f64);
    }
    let value = value.big();
    let mut digits = value.magnitude().iter_u64_digits().rev();
    let magnitude = match digits.len() {
        0 | 1 => digits.next().unwrap_or(0) as f64,
        2..=16 => {
            let high = u128::from(digits.next().unwrap());
            let low = u128::from(digits.next().unwrap());
            let shift = 64 * digits.len() as i32;
            // The top two words contain more than 53 bits. A nonzero tail
            // distinguishes an exact midpoint from a value just above it.
            let sticky = u128::from(digits.any(|digit| digit != 0));
            ((high << 64 | low | sticky) as f64) * 2.0_f64.powi(shift)
        }
        // More than 1024 magnitude bits always exceed the finite binary64 range.
        _ => f64::INFINITY,
    };
    if magnitude.is_infinite() {
        Err(Error::new(
            Kind::OverflowError,
            "int too large to convert to float",
        ))
    } else {
        Ok(if value.is_negative() {
            -magnitude
        } else {
            magnitude
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_float_int(value: f64) -> usize {
    finish(float_to_integer(value).map(|n| Int::new(n).into_raw()), 1)
}

/// Truncate a finite float and preserve Python's distinct NaN and infinity errors.
pub(super) fn float_to_integer(value: f64) -> Result<BigInt> {
    if value.is_nan() {
        return Err(Error::new(
            Kind::ValueError,
            "cannot convert float NaN to integer",
        ));
    }
    if value.is_infinite() {
        return Err(Error::new(
            Kind::OverflowError,
            "cannot convert float infinity to integer",
        ));
    }
    Ok(BigInt::from_f64(value).unwrap())
}

/// Validates Python's decimal grammar before Rust performs correctly rounded
/// conversion. Rust alone also accepts forms outside Python's grammar. Underscores
/// may separate digits only, including in the exponent; Unicode decimal digits
/// and surrounding Unicode whitespace are normalized before validation.
pub(crate) fn parse(source: &str) -> Option<f64> {
    let source = source.trim();
    let mut normalized = if source.is_ascii() {
        Cow::Borrowed(source)
    } else {
        let mut normalized = String::with_capacity(source.len());
        for character in source.chars() {
            if character.is_ascii() {
                normalized.push(character);
            } else {
                let digit = decimal_digit(character)?;
                normalized.push(char::from(b'0' + digit));
            }
        }
        Cow::Owned(normalized)
    };
    let bytes = normalized.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        if byte == b'_'
            && (index == 0
                || !bytes[index - 1].is_ascii_digit()
                || !bytes.get(index + 1).is_some_and(u8::is_ascii_digit))
        {
            return None;
        }
    }
    if normalized.contains('_') {
        normalized = Cow::Owned(normalized.replace('_', ""));
    }
    let unsigned = normalized.strip_prefix(['+', '-']).unwrap_or(&normalized);
    let negative = normalized.starts_with('-');
    if unsigned.eq_ignore_ascii_case("nan") {
        return Some(f64::NAN.copysign(if negative { -1.0 } else { 1.0 }));
    }
    if unsigned.eq_ignore_ascii_case("inf") || unsigned.eq_ignore_ascii_case("infinity") {
        return Some(if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    let bytes = unsigned.as_bytes();
    let mut index = 0;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let mut digits = index;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        digits += index - start;
    }
    if digits == 0 {
        return None;
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == start {
            return None;
        }
    }
    (index == bytes.len())
        .then(|| normalized.parse().ok())
        .flatten()
}

/// # Safety
/// `value` is a rooted string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_float_parse(value: *const Str) -> f64 {
    // SAFETY: the caller keeps this immutable string alive.
    finish(
        parse(unsafe { &*value })
            .ok_or_else(|| Error::new(Kind::ValueError, "could not convert string to float")),
        0.0,
    )
}

/// Parses the ASCII byte grammar without applying Unicode text normalization.
///
/// # Safety
/// `value` is a rooted immutable bytes handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_float(value: *const Bytes) -> f64 {
    // SAFETY: the caller keeps this immutable bytes allocation alive.
    let bytes = unsafe { &*value };
    let parsed = bytes
        .is_ascii()
        .then(|| std::str::from_utf8(bytes).expect("ASCII is valid UTF-8"))
        .and_then(parse);
    finish(
        parsed.ok_or_else(|| Error::new(Kind::ValueError, "could not convert bytes to float")),
        0.0,
    )
}

/// True division, floor division, and modulo use opcodes 0, 1, and 2.
/// Python's floor quotient follows its corrected remainder, rather than simply
/// flooring a rounded `left / right` (for example, `1.0 // 0.1 == 9.0`).
#[unsafe(no_mangle)]
pub extern "C" fn imre_float_divide(left: f64, right: f64, operation: u32) -> f64 {
    if right == 0.0 {
        return Error::new(Kind::ZeroDivisionError, "division by zero").returning(0.0);
    }
    if operation == 0 {
        return left / right;
    }
    let remainder = left % right;
    let opposite = (remainder < 0.0) != (right < 0.0);
    if operation == 2 {
        return if remainder == 0.0 {
            0.0_f64.copysign(right)
        } else if opposite {
            remainder + right
        } else {
            remainder
        };
    }
    let mut quotient = (left - remainder) / right;
    if remainder != 0.0 && opposite {
        quotient -= 1.0;
    }
    if quotient == 0.0 {
        0.0_f64.copysign(left / right)
    } else {
        let floor = quotient.floor();
        if quotient - floor > 0.5 {
            floor + 1.0
        } else {
            floor
        }
    }
}

/// Compares exactly by truncating the finite float to an integer, then resolving
/// an equal truncation using its fractional sign. No integer is rounded to f64.
/// The fourth result, 2, denotes unordered comparison with NaN.
pub(crate) fn compare(integer: &Int, float: f64) -> i32 {
    if float.is_nan() {
        return 2;
    }
    if float.is_infinite() {
        return if float.is_sign_positive() { -1 } else { 1 };
    }
    let truncated = BigInt::from_f64(float).unwrap();
    match integer.big().as_ref().cmp(&truncated) {
        Ordering::Less => -1,
        Ordering::Greater => 1,
        Ordering::Equal => {
            let fraction = float.fract();
            if fraction > 0.0 {
                -1
            } else {
                i32::from(fraction < 0.0)
            }
        }
    }
}

/// # Safety
/// `integer` is a rooted canonical Int handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_float_compare(integer: usize, float: f64) -> i32 {
    // SAFETY: the caller roots the canonical Int for this nonreentrant operation.
    let integer = unsafe { Int::borrow(integer) };
    compare(&integer, float)
}

/// Rounds a positive rational directly to binary64, with one exact division and
/// ties to even. Scaling the integers, rather than converting both operands,
/// permits finite results such as `10**400 / 10**400` and avoids double rounding
/// at the subnormal boundary. Only the final sign bit is applied afterward.
pub(super) fn ratio(left: &Int, right: &Int) -> Result<f64> {
    // Both integer conversions are exact in this range, so hardware division
    // has the same single rounding as the arbitrary-precision path below.
    if let (Some(left), Some(right)) = (left.as_small(), right.as_small())
        && left.unsigned_abs() <= 1_u64 << 53
        && right.unsigned_abs() <= 1_u64 << 53
    {
        return if right == 0 {
            Err(Error::new(Kind::ZeroDivisionError, "division by zero"))
        } else {
            Ok(left as f64 / right as f64)
        };
    }
    let left = left.big();
    let right = right.big();
    if right.is_zero() {
        return Err(Error::new(Kind::ZeroDivisionError, "division by zero"));
    }
    let sign = u64::from(left.is_negative() != right.is_negative()) << 63;
    let left = left.magnitude();
    let right = right.magnitude();
    if left.is_zero() {
        return Ok(f64::from_bits(sign));
    }
    let difference = i128::from(left.bits()) - i128::from(right.bits());
    if difference > 1024 {
        return Err(Error::new(
            Kind::OverflowError,
            "integer division result too large for a float",
        ));
    }
    if difference < -1075 {
        return Ok(f64::from_bits(sign));
    }
    let mut exponent = difference as i32;
    let below_power = if exponent >= 0 {
        left < &(right << exponent as usize)
    } else {
        &(left << (-exponent) as usize) < right
    };
    exponent -= i32::from(below_power);
    if exponent > 1023 {
        return Err(Error::new(
            Kind::OverflowError,
            "integer division result too large for a float",
        ));
    }
    let subnormal = exponent < -1022;
    let shift = if subnormal { 1074 } else { 52 - exponent };
    let (numerator, denominator) = if shift >= 0 {
        (left << shift as usize, right.clone())
    } else {
        (left.clone(), right << (-shift) as usize)
    };
    let quotient = &numerator / &denominator;
    let remainder = numerator - &quotient * &denominator;
    let mut significand = quotient.to_u64().unwrap();
    match (remainder << 1_usize).cmp(&denominator) {
        Ordering::Greater => significand += 1,
        Ordering::Equal => significand += significand & 1,
        Ordering::Less => {}
    }
    let bits = if subnormal {
        significand
    } else {
        if significand == 1_u64 << 53 {
            significand >>= 1;
            exponent += 1;
        }
        if exponent > 1023 {
            return Err(Error::new(
                Kind::OverflowError,
                "integer division result too large for a float",
            ));
        }
        (((exponent + 1023) as u64) << 52) | (significand - (1_u64 << 52))
    };
    Ok(f64::from_bits(sign | bits))
}

/// # Safety
/// Both arguments are rooted canonical Int handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_true_divide(left: usize, right: usize) -> f64 {
    // SAFETY: both handles remain rooted throughout this operation.
    let (left, right) = unsafe { (Int::borrow(left), Int::borrow(right)) };
    finish(ratio(&left, &right), 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quotient(left: BigInt, right: BigInt) -> f64 {
        ratio(&Int::new(left), &Int::new(right))
            .unwrap_or_else(|_| panic!("unexpected ratio error"))
    }

    #[test]
    fn ratios_round_once_at_normal_subnormal_and_overflow_boundaries() {
        let huge = BigInt::from(10).pow(400);
        assert_eq!(quotient(huge.clone(), huge), 1.0);
        assert_eq!(quotient(1.into(), 10.into()), 0.1);
        assert_eq!(quotient(1.into(), BigInt::from(1) << 1075), 0.0);
        assert_eq!(
            quotient((-1).into(), BigInt::from(1) << 1075).to_bits(),
            (-0.0_f64).to_bits()
        );
        assert_eq!(quotient(3.into(), BigInt::from(1) << 1075).to_bits(), 2);
        let max: BigInt = (BigInt::from(1) << 1024_usize) - (BigInt::from(1) << 971_usize);
        assert_eq!(quotient(max.clone(), 1.into()), f64::MAX);
        assert!(
            ratio(
                &Int::new(max + (BigInt::from(1) << 970)),
                &Int::new(1.into())
            )
            .is_err()
        );
        assert_eq!(
            quotient(
                BigInt::from(1) << 53_usize,
                (BigInt::from(1) << 53_usize) + 1
            ),
            0.9999999999999999
        );
    }

    #[test]
    fn mixed_comparisons_keep_integer_precision() {
        let rounded = Int::new(BigInt::from(9_007_199_254_740_993_u64));
        assert_eq!(compare(&rounded, 9_007_199_254_740_992.0), 1);
        let enormous = Int::new(BigInt::from(1) << 4096);
        assert_eq!(compare(&enormous, f64::MAX), 1);
        assert_eq!(compare(&enormous, f64::INFINITY), -1);
        assert_eq!(compare(&enormous, f64::NAN), 2);
        assert_eq!(compare(&Int::new(0.into()), -0.5), 1);
        assert_eq!(compare(&Int::new(0.into()), 0.5), -1);
        assert_eq!(compare(&Int::new(0.into()), -0.0), 0);
    }

    #[test]
    fn decimal_parser_checks_separators_unicode_and_special_values() {
        assert_eq!(parse(" \u{2003}-١_٢.٥e+٢\u{a0}"), Some(-1250.0));
        assert_eq!(parse("1_0.5_0e-0_1"), Some(1.05));
        assert_eq!(parse("-1e-9999").unwrap().to_bits(), (-0.0_f64).to_bits());
        assert_eq!(parse("+INFINITY"), Some(f64::INFINITY));
        assert!(parse("-NaN").unwrap().is_nan());
        for invalid in [
            "", "_1", "1_", "1__2", "1_.0", "1._0", "1e_2", "1e+_2", ".", "1e", "0x1", "nan(1)",
            "1\0", "1 2", "²", "\u{1c}1",
        ] {
            assert!(parse(invalid).is_none(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn floor_and_modulo_preserve_python_corrections_and_zero_signs() {
        assert_eq!(imre_float_divide(1.0, 0.1, 1), 9.0);
        assert_eq!(imre_float_divide(-1.0, 0.1, 1), -10.0);
        assert_eq!(imre_float_divide(1.0, -2.0, 2), -1.0);
        assert_eq!(
            imre_float_divide(0.0, -2.0, 1).to_bits(),
            (-0.0_f64).to_bits()
        );
        assert_eq!(
            imre_float_divide(2.0, -1.0, 2).to_bits(),
            (-0.0_f64).to_bits()
        );
        assert!(imre_float_divide(f64::INFINITY, 1.0, 1).is_nan());
    }

    #[test]
    fn shortest_formatting_uses_python_decimal_ties_and_exponents() {
        for (value, expected) in [
            (f64::from_bits(0x4309_9999_9999_999a), "900719925474099.2"),
            (-0.0, "-0.0"),
            (1e-5, "1e-05"),
            (-1.25e-5, "-1.25e-05"),
            (1e-4, "0.0001"),
            (1e16, "1e+16"),
            (5e-324, "5e-324"),
            (f64::NAN, "nan"),
        ] {
            assert_eq!(render(value), expected);
        }
    }

    #[test]
    fn optional_storage_preserves_all_bits_and_owns_independent_references() {
        for bits in [
            0,
            1_u64 << 63,
            f64::INFINITY.to_bits(),
            0x7ff8_0000_0000_0042,
        ] {
            let value = Rc::new(f64::from_bits(bits));
            let weak = Rc::downgrade(&value);
            let word = Rc::into_raw(value) as usize;
            // SAFETY: these exact rooted handles transfer one reference per release.
            unsafe {
                imre_float_box_retain(word);
                assert_eq!(imre_float_unbox(word).to_bits(), bits);
                imre_float_box_release(word);
                assert_eq!(weak.strong_count(), 1);
                imre_float_box_release(word);
                assert_eq!(weak.strong_count(), 0);
                imre_float_box_retain(0);
                imre_float_box_release(0);
            }
        }
    }
}
