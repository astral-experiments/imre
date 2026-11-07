//! Exact combinations for checked and runtime-imported calls.
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};

use super::{Error, Int, Kind, Result};

/// Batch word-sized numerator and denominator factors before touching wide digits.
fn word_comb(n: usize, steps: usize) -> Int {
    let mut wide = None;
    let (mut numerator, mut denominator) = (1_usize, 1_usize);
    for divisor in 1..=steps {
        let factor = n - divisor + 1;
        if let Some(product) = numerator.checked_mul(factor) {
            numerator = product;
            // Symmetry bounds steps by n / 2, so each divisor fits its factor.
            denominator *= divisor;
        } else {
            // At each group boundary the running value is exactly C(n, divisor - 1).
            let result = wide.get_or_insert_with(|| BigInt::from(1));
            *result *= numerator;
            *result /= denominator;
            numerator = factor;
            denominator = divisor;
        }
    }
    match wide {
        Some(result) => Int::new(result * numerator / denominator),
        None => Int::from_usize(numerator / denominator),
    }
}

/// Compute math.comb for either entry point after both arguments have been indexed.
pub(super) fn integer_comb(n: &Int, k: &Int) -> Result<Int> {
    if let (Some(n), Some(k)) = (n.as_small(), k.as_small())
        && n >= 0
        && k >= 0
    {
        return Ok(if k > n {
            Int::from(0)
        } else {
            word_comb(n as usize, k.min(n - k) as usize)
        });
    }
    let (value, k) = (n.big(), k.big());
    if value.is_negative() {
        return Err(Error::new(
            Kind::ValueError,
            "n must be a non-negative integer",
        ));
    }
    if k.is_negative() {
        return Err(Error::new(
            Kind::ValueError,
            "k must be a non-negative integer",
        ));
    }
    if k > value {
        return Ok(Int::from(0));
    }
    let complement = value.as_ref() - k.as_ref();
    let steps = k.as_ref().min(&complement).to_i64().ok_or_else(|| {
        Error::new(
            Kind::OverflowError,
            "min(n - k, k) must not exceed 9223372036854775807",
        )
    })?;
    // C(n, d) = C(n, d - 1) * (n - d + 1) / d. The division is exact,
    // and the unbounded product does not need a GCD to prevent overflow.
    match steps {
        0 => return Ok(Int::from(1)),
        1 => return Ok(n.clone()),
        _ => {}
    }
    let mut factor = value.into_owned();
    let mut result = factor.clone();
    for divisor in 2..=steps {
        factor -= 1;
        result *= &factor;
        result /= divisor;
    }
    Ok(Int::new(result))
}
