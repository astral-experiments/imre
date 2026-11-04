//! Real-number and integer operations for boxed math callables.

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, ToPrimitive, Zero};
use smallvec::SmallVec;

use super::{Context, Error, Id, Result, Value};
use crate::dynamic::builtins;

/// Retain the original function's argument binding and live arithmetic hooks.
pub(in crate::dynamic) fn fraction_hash(
    ctx: &mut Context,
    fallback: Id,
    args: Vec<Id>,
    kwargs: Vec<(String, Id)>,
) -> Result<Id> {
    if let &[n, d] = args.as_slice()
        && kwargs.is_empty()
        && let Value::Function { scope, .. } = *ctx.heap.get(fallback)
        && let Some(p) = ctx.scope_name(scope, "_PyHASH_MODULUS")
        && matches!(ctx.heap.get(p), Value::Int(p) if p.as_small() == Some(crate::python_hash::MODULUS))
        && ["pow", "hash", "abs"].into_iter().all(|name| {
            ctx.lookup(scope, name).is_ok_and(
                |id| matches!(ctx.heap.get(id), Value::Native(current) if current == name),
            )
        })
        && let (Value::Int(n), Value::Int(d)) = (ctx.heap.get(n), ctx.heap.get(d))
        && let Some(hash) = crate::python_hash::fraction_hash(n, d)
    {
        return Ok(ctx.int(hash));
    }
    // Subclass operations, replaced builtins and a noninvertible denominator
    // must use the module's live Python operations and infinity constant.
    ctx.call(fallback, args, kwargs)
}

/// Compute the bounded candidates and the exact distance comparison for Fraction.
pub(super) fn fraction_limit_candidates(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let &[n, d, limit] = args else {
        return Err(Error::new(
            "TypeError",
            "_limit_candidates requires three integers",
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "_limit_candidates takes no keyword arguments",
        ));
    }
    let integer = |id| match ctx.heap.get(id) {
        Value::Int(value) => Some(value.big().into_owned()),
        _ => None,
    };
    let (Some(mut n), Some(mut d), Some(limit)) = (integer(n), integer(d), integer(limit)) else {
        return Ok(super::NONE);
    };
    if limit < BigInt::one() || d <= limit {
        return Ok(super::NONE);
    }
    let (mut p0, mut q0) = (BigInt::zero(), BigInt::one());
    let (mut p1, mut q1) = (BigInt::one(), BigInt::zero());
    while !d.is_zero() {
        let (a, remainder) = n.div_mod_floor(&d);
        let q2 = &q0 + &a * &q1;
        if q2 > limit {
            let k = (limit - &q0) / &q1;
            let (p2, q2) = (p0 + &k * &p1, q0 + k * &q1);
            let distance = (d * &q2) << 1;
            let values =
                [p1, q1, p2, q2, distance].map(|n| ctx.alloc(Value::Int(crate::Int::new(n))));
            return Ok(ctx.alloc(Value::Tuple(values.into())));
        }
        let p2 = p0 + a * &p1;
        (p0, q0, p1, q1) = (p1, q1, p2, q2);
        (n, d) = (d, remainder);
    }
    // A mutated, non-reduced Fraction can exhaust the divisions before the bound.
    Ok(super::NONE)
}

/// Dispatch the boxed math functions.
pub(super) fn call(
    ctx: &mut Context,
    operation: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if operation == "isclose" {
        return isclose(ctx, args, kwargs);
    }
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
    if matches!(operation, "atan2" | "copysign") {
        let &[first, second] = args else {
            return Err(Error::new(
                "TypeError",
                format!("{operation} expected 2 arguments, got {}", args.len()),
            ));
        };
        let first = as_double(ctx, first)?;
        let second = as_double(ctx, second)?;
        let result = if operation == "atan2" {
            first.atan2(second)
        } else {
            first.copysign(second)
        };
        return Ok(ctx.alloc(Value::Float(result)));
    }
    if operation == "fma" {
        let &[x, y, z] = args else {
            return Err(Error::new(
                "TypeError",
                format!("fma expected 3 arguments, got {}", args.len()),
            ));
        };
        let x = as_double(ctx, x)?;
        let y = as_double(ctx, y)?;
        let z = as_double(ctx, z)?;
        let result = x.mul_add(y, z);
        if result.is_nan() && !x.is_nan() && !y.is_nan() && !z.is_nan() {
            return Err(Error::new("ValueError", "invalid operation in fma"));
        }
        if result.is_infinite() && x.is_finite() && y.is_finite() && z.is_finite() {
            return Err(Error::new("OverflowError", "overflow in fma"));
        }
        return Ok(ctx.alloc(Value::Float(result)));
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
    if operation == "fsum" {
        return fsum(ctx, value);
    }
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
    if operation == "acosh" {
        let x = as_double(ctx, value)?;
        let result = x.acosh();
        if result.is_nan() && !x.is_nan() {
            return Err(Error::new(
                "ValueError",
                format!(
                    "expected argument value not less than 1, got {}",
                    crate::floats::render(x)
                ),
            ));
        }
        return Ok(ctx.alloc(Value::Float(result)));
    }
    let trig: Option<fn(f64) -> f64> = match operation {
        "cos" => Some(f64::cos),
        "sin" => Some(f64::sin),
        "tan" => Some(f64::tan),
        _ => None,
    };
    if let Some(trig) = trig {
        let x = as_double(ctx, value)?;
        if x.is_infinite() {
            return Err(Error::new(
                "ValueError",
                format!("expected a finite input, got {}", crate::floats::render(x)),
            ));
        }
        return Ok(ctx.alloc(Value::Float(trig(x))));
    }
    if operation == "exp" {
        let x = as_double(ctx, value)?;
        let result = x.exp();
        if x.is_finite() && result.is_infinite() {
            return Err(Error::new("OverflowError", "math range error"));
        }
        return Ok(ctx.alloc(Value::Float(result)));
    }
    if operation == "isfinite" {
        return Ok(builtins::boolean(as_double(ctx, value)?.is_finite()));
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
    if operation == "sqrt" {
        let x = as_double(ctx, value)?;
        if x < 0.0 {
            return Err(Error::new(
                "ValueError",
                format!(
                    "expected a nonnegative input, got {}",
                    crate::floats::render(x)
                ),
            ));
        }
        return Ok(ctx.alloc(Value::Float(x.sqrt())));
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

fn isclose(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(a), Some(b), rel_tol, abs_tol] =
        builtins::bind_two_positional("isclose", args, kwargs, ["a", "b", "rel_tol", "abs_tol"])?
    else {
        unreachable!("required isclose operands were checked");
    };
    let a = as_double(ctx, a)?;
    let b = as_double(ctx, b)?;
    let rel_tol = rel_tol.map_or(Ok(1e-9), |value| as_double(ctx, value))?;
    let abs_tol = abs_tol.map_or(Ok(0.0), |value| as_double(ctx, value))?;
    if rel_tol < 0.0 || abs_tol < 0.0 {
        return Err(Error::new("ValueError", "tolerances must be non-negative"));
    }
    if a == b {
        return Ok(builtins::boolean(true));
    }
    if a.is_infinite() || b.is_infinite() {
        return Ok(builtins::boolean(false));
    }
    let diff = (b - a).abs();
    Ok(builtins::boolean(
        diff <= (rel_tol * b).abs() || diff <= (rel_tol * a).abs() || diff <= abs_tol,
    ))
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

/// Accumulate non-overlapping partial sums, as in CPython's math_fsum.
fn fsum(ctx: &mut Context, values: Id) -> Result<Id> {
    let iterator = ctx.iterator(values)?;
    ctx.heap.keep(iterator);
    let mark = ctx.heap.temporary_mark();
    let mut limit = ctx.heap.streaming_collection_limit();
    let mut partials = SmallVec::<[f64; 32]>::new();
    let mut last_nan = None;
    let mut inf_sum = 0.0_f64;

    while let Some(item) = ctx.next_item(iterator)? {
        ctx.heap.keep(item);
        let mut x = as_double(ctx, item)?;
        ctx.heap.release_temporaries_since(mark);
        ctx.collect_streaming_garbage(&mut limit);

        if !x.is_finite() {
            if x.is_nan() {
                // Quiet a signaling NaN while preserving its sign and payload.
                last_nan = Some(x + 0.0);
            } else {
                inf_sum += x;
            }
            partials.clear();
            continue;
        }
        partials.retain_mut(|partial| {
            let mut y = *partial;
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            // With |x| >= |y|, this recovers the exact rounding error.
            *partial = y - (hi - x);
            x = hi;
            *partial != 0.0
        });
        if !x.is_finite() {
            return Err(Error::new("OverflowError", "intermediate overflow in fsum"));
        }
        if x != 0.0 {
            partials
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", "math.fsum partials"))?;
            partials.push(x);
        }
    }
    if inf_sum.is_nan() {
        return Err(Error::new("ValueError", "-inf + inf in fsum"));
    }
    let special_sum = last_nan.unwrap_or(inf_sum);
    if special_sum != 0.0 {
        return Ok(ctx.alloc(Value::Float(special_sum)));
    }

    let mut hi = partials.pop().unwrap_or(0.0);
    while let Some(y) = partials.pop() {
        let x = hi;
        hi = x + y;
        let lo = y - (hi - x);
        if lo != 0.0 {
            // A smaller partial of the same sign can tip a halfway tie.
            if let Some(&next) = partials.last()
                && lo.is_sign_positive() == next.is_sign_positive()
            {
                let twice_lo = lo * 2.0;
                let adjusted = hi + twice_lo;
                if adjusted - hi == twice_lo {
                    hi = adjusted;
                }
            }
            break;
        }
    }
    Ok(ctx.alloc(Value::Float(hi)))
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
