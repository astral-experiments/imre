//! Integer literal decoding and Python numeric representations.
use compact_str::CompactString;
use num_bigint::{BigInt, BigUint};
use num_traits::FromPrimitive;
use std::cell::RefCell;

pub fn parse_integer_literal(literal: &str) -> Option<BigInt> {
    let literal = literal.replace('_', "");
    let (literal, negative) = literal
        .strip_prefix('-')
        .map_or((literal.as_str(), false), |s| (s, true));
    let (digits, radix) = if literal.starts_with("0x") || literal.starts_with("0X") {
        (&literal[2..], 16)
    } else if literal.starts_with("0o") || literal.starts_with("0O") {
        (&literal[2..], 8)
    } else if literal.starts_with("0b") || literal.starts_with("0B") {
        (&literal[2..], 2)
    } else {
        (literal, 10)
    };
    BigInt::parse_bytes(digits.as_bytes(), radix).map(|value| if negative { -value } else { value })
}

/// Ryu chooses shortest digits with ties to even; Rust's formatter, currently
/// used by Ruff for finite values, instead prints 900719925474099.25 as a final
/// `.3` where Python chooses `.2`. Keep Ruff's nonfinite spelling and adapt Ryu's
/// finite exponent notation: Python switches at 1e-5 and always writes its sign
/// and at least two exponent digits.
pub fn float_repr(value: f64) -> CompactString {
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
        let mut result = CompactString::new(if value.is_sign_negative() { "-" } else { "" });
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

pub fn complex_repr(real: f64, imag: f64) -> String {
    let imag = float_repr(imag);
    let imag = imag.strip_suffix(".0").unwrap_or(&imag);
    if real.to_bits() == 0 {
        return format!("{imag}j");
    }
    let real = float_repr(real);
    let real = real.strip_suffix(".0").unwrap_or(&real);
    let sign = if imag.starts_with('-') { "" } else { "+" };
    format!("({real}{sign}{imag}j)")
}

/// Default Python limit when rendering an integer in decimal.
pub const DEFAULT_DIGITS: u32 = 4300;

thread_local! {
    // Cache only the threshold for the most recently checked nonzero limit on this thread.
    static MAXIMUM_EXCLUSIVE: RefCell<(u32, BigUint)> = const { RefCell::new((0, BigUint::ZERO)) };
}

/// Test the actual integer before incurring a possibly unbounded decimal conversion.
pub fn decimal_fits(value: &BigInt, limit: u32) -> bool {
    let digits = u64::from(limit);
    let bits = value.bits();
    if limit == 0 || bits <= digits * 3 {
        return true;
    }
    bits <= digits * 4
        && MAXIMUM_EXCLUSIVE.with_borrow_mut(|cached| {
            if cached.0 != limit {
                *cached = (limit, BigUint::from(10u8).pow(limit));
            }
            value.magnitude() < &cached.1
        })
}

/// The exact integer key of a binary64 value when it compares equal to an integer.
pub fn integral_float(value: f64) -> Option<BigInt> {
    if value.fract() == 0.0 {
        BigInt::from_f64(value)
    } else {
        None
    }
}
