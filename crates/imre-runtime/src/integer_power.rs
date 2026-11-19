//! Python integer exponentiation, including modular inverses for negative powers.

use super::*;
use num_integer::Integer;
use num_traits::Pow;

/// # Safety
/// Both arguments borrow live integers; the compiler has proved a nonnegative exponent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_pow(base: usize, exponent: usize) -> usize {
    // SAFETY: the generated call roots both handles until it returns.
    let (base, exponent) = unsafe { (Int::borrow(base), Int::borrow(exponent)) };
    finish(power(&base, &exponent).map(Int::into_raw), 1)
}

/// Raises an integer to a nonnegative exponent of arbitrary size.
pub(super) fn power(base: &Int, exponent: &Int) -> Result<Int> {
    let (base, exponent) = (base.big(), exponent.big());
    debug_assert!(!exponent.is_negative());
    // Even the lower bound on the bit count must fit the native integer storage.
    // Zero and +/-1 have bounded results for arbitrarily large exponents.
    if base.bits() > 1
        && exponent
            .to_u64()
            .is_none_or(|n| n > (i64::MAX as u64 - 1) / (base.bits() - 1))
    {
        return Err(Error::new(
            Kind::OverflowError,
            "too many digits in integer",
        ));
    }
    Ok(Int::new(Pow::pow(base.as_ref(), exponent.magnitude())))
}

/// # Safety
/// Both arguments borrow live integers; the compiler has proved a negative exponent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_negative_pow(base: usize, exponent: usize) -> f64 {
    // SAFETY: both arguments remain live across the conversion.
    let (base, exponent) = unsafe { (Int::borrow(base), Int::borrow(exponent)) };
    finish(negative_power(&base, &exponent), 0.0)
}

/// Converts integer operands to binary64 for a negative exponent.
pub(super) fn negative_power(base: &Int, exponent: &Int) -> Result<f64> {
    let base = floats::int_to_float(base)?;
    let exponent = floats::int_to_float(exponent)?;
    if base == 0.0 {
        return Err(Error::new(
            Kind::ZeroDivisionError,
            "zero to a negative power",
        ));
    }
    Ok(base.powf(exponent))
}

/// # Safety
/// All arguments borrow live integers. A successful return owns its integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_modpow(base: usize, exponent: usize, modulus: usize) -> usize {
    // SAFETY: the three operands stay rooted by the generated caller.
    let (base, exponent, modulus) = unsafe {
        (
            Int::borrow(base),
            Int::borrow(exponent),
            Int::borrow(modulus),
        )
    };
    finish(
        modular_power(&base, &exponent, &modulus).map(Int::into_raw),
        1,
    )
}

/// Handles either sign of modulus and inverts the base for negative exponents.
pub(super) fn modular_power(base: &Int, exponent: &Int, modulus: &Int) -> Result<Int> {
    let exponent = exponent.big();
    if let Cow::Borrowed(exponent) = &exponent
        && exponent.is_positive()
        && let (Some(base), Some(modulus)) = (base.as_small(), modulus.as_small())
        && (2..=u64::from(u32::MAX)).contains(&modulus.unsigned_abs())
    {
        return Ok(Int::from(word_modular_power(base, exponent, modulus)));
    }
    let (base, modulus) = (base.big(), modulus.big());
    if modulus.is_zero() {
        return Err(Error::new(
            Kind::ValueError,
            "pow() 3rd argument cannot be 0",
        ));
    }
    if modulus.bits() == 1 {
        return Ok(Int::from(0));
    }
    let result = if exponent.is_negative() {
        let Some(inverse) = base.modinv(&modulus) else {
            return Err(Error::new(
                Kind::ValueError,
                "base is not invertible for the given modulus",
            ));
        };
        inverse.modpow(&-exponent.as_ref(), &modulus)
    } else if let Some(power) = exponent.to_u32()
        && base.bits().saturating_mul(u64::from(power)) <= 512
    {
        // At most eight 64-bit limbs are needed for the full power. Reduce it
        // once instead of setting up modular multiplication for a short exponent.
        base.as_ref().pow(power).mod_floor(&modulus)
    } else {
        base.modpow(&exponent, &modulus)
    };
    Ok(Int::new(result))
}

/// Reduces each product in one word when both residues fit in 32 bits.
fn word_modular_power(base: i64, exponent: &BigInt, modulus: i64) -> i64 {
    let magnitude = modulus.unsigned_abs();
    let factor = base.rem_euclid(magnitude as i64) as u64;
    let mut result = 1;
    for bit in (0..exponent.bits()).rev() {
        result = result * result % magnitude;
        if exponent.bit(bit) {
            result = result * factor % magnitude;
        }
    }
    let result = result as i64;
    if modulus < 0 && result != 0 {
        result + modulus
    } else {
        result
    }
}
