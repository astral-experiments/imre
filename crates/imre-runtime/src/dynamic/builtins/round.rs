use num_bigint::{BigInt, BigUint, Sign};
use num_traits::{Euclid, Pow};

use super::{Context, Error, Id, NONE, Result, Value, bind_builtin_arguments, type_error};
use crate::Int;

/// Call the operand's original __round__, or its built-in numeric implementation.
pub(super) fn call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(value), digits] =
        bind_builtin_arguments("round", args, kwargs, ["number", "ndigits"], 1)?
    else {
        unreachable!("the required operand was checked above")
    };
    let digits = digits.filter(|&id| id != NONE);
    if let Some(method) = ctx.bound_numeric_slot(value, "__round__")? {
        return ctx.call(method, digits.into_iter().collect(), Vec::new());
    }
    let scalar = ctx.scalar_value(value);
    if !matches!(
        ctx.heap.get(scalar),
        Value::Int(_) | Value::Bool(_) | Value::Float(_)
    ) {
        return Err(type_error(format!(
            "type {} doesn't define __round__ method",
            ctx.type_name(value)
        )));
    }
    let places = digits
        .map(|id| ctx.index_integer(id))
        .transpose()?
        .map(|n| {
            n.to_i64().unwrap_or_else(|| {
                if n.big().sign() == Sign::Minus {
                    i64::MIN
                } else {
                    i64::MAX
                }
            })
        });
    match *ctx.heap.get(scalar) {
        Value::Float(n) => match places {
            None => Ok(
                ctx.alloc(Value::Int(Int::new(crate::floats::float_to_integer(
                    n.round_ties_even(),
                )?))),
            ),
            Some(places) => Ok(ctx.alloc(Value::Float(round_float(n, places)?))),
        },
        Value::Bool(b) => Ok(ctx.int(if places.unwrap_or(0) < 0 {
            0
        } else {
            i64::from(b)
        })),
        Value::Int(ref n) => {
            if let Some(places @ ..0) = places {
                let rounded = round_multiple(&n.big(), places.unsigned_abs(), false);
                Ok(ctx.alloc(Value::Int(Int::new(rounded))))
            } else if scalar == value {
                Ok(value)
            } else {
                Ok(ctx.alloc(Value::Int(n.clone())))
            }
        }
        _ => unreachable!(),
    }
}

/// Round a magnitude to a decimal multiple. A discarded fraction breaks an integer midpoint.
fn round_multiple(n: &BigInt, places: u64, fractional: bool) -> BigInt {
    if places > n.bits() {
        return BigInt::ZERO;
    }
    let scale = BigUint::from(10u8).pow(places);
    let (mut quotient, remainder) = n.magnitude().div_rem_euclid(&scale);
    let twice = remainder << 1;
    if twice > scale || (twice == scale && (fractional || quotient.bit(0))) {
        quotient += 1u8;
    }
    BigInt::from_biguint(n.sign(), quotient * scale)
}

/// Round the exact binary64 value to decimal places, preserving signed zero.
fn round_float(n: f64, places: i64) -> Result<f64> {
    if !n.is_finite() || places > 323 {
        return Ok(n);
    }
    if places < -308 {
        return Ok(0.0_f64.copysign(n));
    }
    if places == 0 {
        return Ok(n.round_ties_even());
    }
    if places > 0 {
        let decimal = crate::objects::format_fixed_float(n, places as u32, false, 0);
        return Ok(decimal.parse().expect("fixed-point decimal"));
    }
    let integer = crate::floats::float_to_integer(n)?;
    let rounded = Int::new(round_multiple(
        &integer,
        places.unsigned_abs(),
        n.fract() != 0.0,
    ));
    crate::floats::int_to_float(&rounded)
        .map(|value| value.copysign(n))
        .map_err(|_| Error::new("OverflowError", "rounded value too large to represent"))
}
