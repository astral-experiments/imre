//! Real-number and integer operations for boxed math callables.

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, ToPrimitive, Zero};

use super::{Context, Error, Id, Result, Value};
use crate::dynamic::builtins;

/// Dispatch the positional-only boxed math functions.
pub(super) fn call(
    ctx: &mut Context,
    operation: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("math.{operation}() takes no keyword arguments"),
        ));
    }
    if matches!(operation, "gcd" | "lcm") {
        let mut result = if operation == "gcd" {
            BigInt::ZERO
        } else {
            BigInt::one()
        };
        for &arg in args {
            // Later arguments still run __index__ after GCD reaches one or LCM reaches zero.
            let integer = ctx.index_integer(arg)?;
            match operation {
                "gcd" if !result.is_one() => result = result.gcd(&integer.big()),
                "lcm" if !result.is_zero() => result = result.lcm(&integer.big()),
                _ => {}
            }
        }
        return Ok(ctx.alloc(Value::Int(crate::Int::new(result))));
    }
    if operation == "copysign" {
        let &[x, y] = args else {
            return Err(Error::new(
                "TypeError",
                format!("copysign expected 2 arguments, got {}", args.len()),
            ));
        };
        let x = as_double(ctx, x)?;
        let y = as_double(ctx, y)?;
        return Ok(ctx.alloc(Value::Float(x.copysign(y))));
    }
    if operation == "log" {
        let (value, base) = match args {
            [value] => (value, None),
            [value, base] => (value, Some(base)),
            [] => {
                return Err(Error::new(
                    "TypeError",
                    "log expected at least 1 argument, got 0",
                ));
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("log expected at most 2 arguments, got {}", args.len()),
                ));
            }
        };
        let mut result = natural_log(ctx, *value)?;
        if let Some(base) = base {
            let denominator = natural_log(ctx, *base)?;
            if denominator == 0.0 {
                return Err(Error::new("ZeroDivisionError", "division by zero"));
            }
            result /= denominator;
        }
        return Ok(ctx.alloc(Value::Float(result)));
    }
    let &[value] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "math.{operation}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    };
    if operation == "isqrt" {
        let n = ctx.index_integer(value)?;
        if n.big().sign() == num_bigint::Sign::Minus {
            return Err(Error::new(
                "ValueError",
                "isqrt() argument must be nonnegative",
            ));
        }
        return Ok(ctx.alloc(Value::Int(crate::Int::new(n.big().sqrt()))));
    }
    if operation == "cos" {
        let x = as_double(ctx, value)?;
        if x.is_infinite() {
            return Err(Error::new(
                "ValueError",
                format!("expected a finite input, got {}", crate::floats::render(x)),
            ));
        }
        return Ok(ctx.alloc(Value::Float(x.cos())));
    }
    if operation == "isinf" {
        return Ok(builtins::boolean(as_double(ctx, value)?.is_infinite()));
    }
    if operation == "isnan" {
        return Ok(builtins::boolean(as_double(ctx, value)?.is_nan()));
    }
    if operation == "modf" {
        return modf(ctx, value);
    }
    let slot = match operation {
        "trunc" => "__trunc__",
        "floor" => "__floor__",
        "ceil" => "__ceil__",
        _ => unreachable!(),
    };
    if let Some(method) = ctx.bound_numeric_slot(value, slot)? {
        return ctx.call(method, Vec::new(), Vec::new());
    }
    let scalar = ctx.scalar_value(value);
    let number = match *ctx.heap.get(scalar) {
        Value::Int(_) if scalar == value => return Ok(value),
        Value::Int(ref integer) => return Ok(ctx.alloc(Value::Int(integer.clone()))),
        Value::Bool(boolean) => return Ok(ctx.int(i64::from(boolean))),
        Value::Float(number) => number,
        _ if operation == "trunc" => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "type {:.100} doesn't define __trunc__ method",
                    ctx.type_name(value)
                ),
            ));
        }
        _ => {
            let converted = builtins::from_numeric(ctx, value)?.ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!("must be real number, not {}", ctx.type_name(value)),
                )
            })?;
            let Value::Float(number) = *ctx.heap.get(converted) else {
                unreachable!()
            };
            number
        }
    };
    let rounded = match operation {
        "floor" => number.floor(),
        "ceil" => number.ceil(),
        _ => number,
    };
    Ok(ctx.alloc(Value::Int(crate::Int::new(
        crate::floats::float_to_integer(rounded)?,
    ))))
}

/// Take the log of an int's stored value, including those too large for a double.
/// Other numbers go through the same real conversion as the unary math functions.
fn natural_log(ctx: &mut Context, value: Id) -> Result<f64> {
    match ctx.heap.get(ctx.scalar_value(value)) {
        Value::Bool(true) => return Ok(0.0),
        Value::Bool(false) => return Err(Error::new("ValueError", "expected a positive input")),
        Value::Int(integer) => {
            let n = integer.big();
            if n.sign() != num_bigint::Sign::Plus {
                return Err(Error::new("ValueError", "expected a positive input"));
            }
            if let Ok(x) = crate::floats::int_to_float(integer) {
                return Ok(x.ln());
            }
            // CPython's loghelper uses a rounded mantissa and a separate exponent
            // on overflow. Keep one sticky bit so the cast rounds ties correctly.
            let magnitude = n.magnitude();
            let mut exponent = magnitude.bits();
            let shift = exponent - u64::from(u64::BITS);
            let high = (magnitude >> shift).to_u64().unwrap();
            let sticky = magnitude.trailing_zeros().unwrap() < shift;
            let mut mantissa = (high | u64::from(sticky)) as f64 * 2.0_f64.powi(-64);
            if mantissa == 1.0 {
                mantissa = 0.5;
                exponent += 1;
            }
            return Ok(mantissa.ln() + std::f64::consts::LN_2 * exponent as f64);
        }
        _ => {}
    }
    let x = as_double(ctx, value)?;
    if x <= 0.0 {
        return Err(Error::new(
            "ValueError",
            format!(
                "expected a positive input, got {}",
                crate::floats::render(x)
            ),
        ));
    }
    Ok(x.ln())
}

/// Read a float subclass's stored double before trying the numeric conversion protocol.
pub(super) fn as_double(ctx: &mut Context, value: Id) -> Result<f64> {
    match *ctx.heap.get(ctx.scalar_value(value)) {
        Value::Float(x) => Ok(x),
        _ => {
            let converted = builtins::from_numeric(ctx, value)?.ok_or_else(|| {
                let name = ctx.type_name(value);
                let name = &name[..name.floor_char_boundary(50)];
                Error::new("TypeError", format!("must be real number, not {name}"))
            })?;
            let Value::Float(x) = *ctx.heap.get(converted) else {
                unreachable!()
            };
            Ok(x)
        }
    }
}

/// Split at the integer toward zero, preserving the sign and payload of either nonfinite case.
fn modf(ctx: &mut Context, value: Id) -> Result<Id> {
    let x = as_double(ctx, value)?;
    let (fraction, integral) = if x.is_nan() {
        (x, x)
    } else if x.is_infinite() {
        (0.0_f64.copysign(x), x)
    } else {
        let integral = x.trunc();
        ((x - integral).copysign(x), integral)
    };
    let fraction = ctx.alloc(Value::Float(fraction));
    let integral = ctx.alloc(Value::Float(integral));
    Ok(ctx.alloc(Value::Tuple(vec![fraction, integral])))
}
