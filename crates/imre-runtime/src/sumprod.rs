//! Pairwise sum of products, following CPython's Modules/mathmodule.c.

use crate::Int;
use std::ffi::c_long;

pub(crate) enum Side {
    Left,
    Right,
}

/// Only the exact builtin types participate in CPython's numerical prefixes.
pub(crate) enum Number<'a> {
    Int(&'a Int),
    Float(f64),
    Bool(bool),
    Other,
}

/// Both iterators are acquired left to right before entry. Values yielded or
/// created by these operations must remain live until the end of the pair.
pub(crate) trait Operands {
    type Value;
    type Error;

    fn next(&mut self, side: Side) -> Result<Option<Self::Value>, Self::Error>;
    fn number<'a>(&'a self, value: &'a Self::Value) -> Number<'a>;
    fn integer(&mut self, value: c_long) -> Self::Value;
    fn float(&mut self, value: f64) -> Self::Value;
    fn multiply(
        &mut self,
        left: &Self::Value,
        right: &Self::Value,
    ) -> Result<Self::Value, Self::Error>;
    fn add(&mut self, left: &Self::Value, right: &Self::Value) -> Result<Self::Value, Self::Error>;
    fn value_error(&self, message: &'static str) -> Self::Error;

    /// Release consumed operands before advancing again, keeping the new total.
    fn end_pair(&mut self, _total: &Self::Value) {}
}

pub(crate) fn reduce<O: Operands>(ops: &mut O) -> Result<O::Value, O::Error> {
    let mut prefix = Prefix::Integers(None);
    let mut total = ops.integer(0);
    loop {
        let left = ops.next(Side::Left)?;
        let right = ops.next(Side::Right)?;
        match (left, right) {
            (Some(left), Some(right)) => prefix.add_product(ops, &mut total, &left, &right)?,
            (None, None) => {
                prefix.flush(ops, &mut total)?;
                return Ok(total);
            }
            _ => return Err(ops.value_error("Inputs are not the same length")),
        }
        ops.end_pair(&total);
    }
}

/// Each prefix ends permanently on the first pair it cannot consume. A used
/// subtotal, even zero, is added to the existing Python total on that boundary.
#[derive(Clone, Copy)]
enum Prefix {
    Integers(Option<c_long>),
    Floats(Option<Triple>),
    Generic,
}

impl Prefix {
    fn add_product<O: Operands>(
        &mut self,
        ops: &mut O,
        total: &mut O::Value,
        left: &O::Value,
        right: &O::Value,
    ) -> Result<(), O::Error> {
        if let Self::Integers(subtotal) = *self {
            if let (Number::Int(x), Number::Int(y)) = (ops.number(left), ops.number(right))
                && let Some(product) = as_long(x).and_then(|x| x.checked_mul(as_long(y)?))
                && let Some(sum) = subtotal.unwrap_or(0).checked_add(product)
            {
                *self = Self::Integers(Some(sum));
                return Ok(());
            }
            self.flush(ops, total)?;
        }
        if let Self::Floats(subtotal) = *self {
            if let Some((x, y)) = float_pair(ops.number(left), ops.number(right)) {
                let sum = subtotal.unwrap_or_default().add_product(x, y);
                if sum.hi.is_finite() {
                    *self = Self::Floats(Some(sum));
                    return Ok(());
                }
            }
            self.flush(ops, total)?;
        }
        let product = ops.multiply(left, right)?;
        *total = ops.add(total, &product)?;
        Ok(())
    }

    fn flush<O: Operands>(&mut self, ops: &mut O, total: &mut O::Value) -> Result<(), O::Error> {
        let term = match *self {
            Self::Integers(subtotal) => {
                *self = Self::Floats(None);
                subtotal.map(|sum| ops.integer(sum))
            }
            Self::Floats(subtotal) => {
                *self = Self::Generic;
                subtotal.map(|sum| ops.float(sum.round()))
            }
            Self::Generic => None,
        };
        if let Some(term) = term {
            *total = ops.add(total, &term)?;
        }
        Ok(())
    }
}

fn as_long(value: &Int) -> Option<c_long> {
    c_long::try_from(value.to_i64()?).ok()
}

fn float_pair(left: Number<'_>, right: Number<'_>) -> Option<(f64, f64)> {
    use Number::{Bool, Float, Int};
    match (left, right) {
        (Float(x), Float(y)) => Some((x, y)),
        (Float(x), Int(y)) => Some((x, crate::floats::int_to_float(y).ok()?)),
        (Int(x), Float(y)) => Some((crate::floats::int_to_float(x).ok()?, y)),
        (Float(x), Bool(y)) => Some((x, f64::from(y))),
        (Bool(x), Float(y)) => Some((f64::from(x), y)),
        _ => None,
    }
}

#[derive(Clone, Copy, Default)]
struct Triple {
    hi: f64,
    lo: f64,
    tiny: f64,
}

/// Split the sum into its rounded value and the residual, without ordering the inputs.
fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let sum = a + b;
    let z = sum - a;
    (sum, (a - (sum - z)) + (b - z))
}

impl Triple {
    // CPython's tl_fma retains the product residual as well as the rounding
    // errors from adding it into the high and low components.
    fn add_product(self, x: f64, y: f64) -> Self {
        let product = x * y;
        let product_error = x.mul_add(y, -product);
        let (hi, hi_error) = two_sum(self.hi, product);
        let (low_sum, low_error) = two_sum(self.lo, product_error);
        let (lo, carry_error) = two_sum(low_sum, hi_error);
        Self {
            hi,
            lo,
            tiny: self.tiny + low_error + carry_error,
        }
    }

    fn round(self) -> f64 {
        let (hi, lo) = two_sum(self.lo, self.hi);
        self.tiny + lo + hi
    }
}
