//! Real-number and integer operations for boxed math callables.

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::One;

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
    if operation == "gcd" {
        let mut gcd = BigInt::default();
        for &arg in args {
            let integer = ctx.index_integer(arg)?;
            if !gcd.is_one() {
                gcd = gcd.gcd(&integer.big());
            }
        }
        return Ok(ctx.alloc(Value::Int(crate::Int::new(gcd))));
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

/// Read a float subclass's stored double before trying the numeric conversion protocol.
fn as_double(ctx: &mut Context, value: Id) -> Result<f64> {
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
