//! Real-number and integer operations for boxed math callables.

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, ToPrimitive, Zero};
use smallvec::SmallVec;

use super::{Context, Error, Id, Result, Value};
use crate::dynamic::builtins;
use crate::sumprod::{self, Number, Operands, Side};

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
    if operation == "prod" {
        return prod(ctx, args, kwargs);
    }
    if operation == "nextafter" {
        return nextafter(ctx, args, kwargs);
    }
    if operation == "isclose" {
        return isclose(ctx, args, kwargs);
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("math.{operation}() takes no keyword arguments"),
        ));
    }
    if operation == "factorial" {
        let &[n] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "math.factorial() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        let n = ctx.index_integer(n)?;
        return Ok(ctx.alloc(Value::Int(crate::integer_factorial(&n)?)));
    }
    if operation == "perm" && matches!(args, [_] | [_, super::NONE]) {
        let n = ctx.index_integer(args[0])?;
        return Ok(ctx.alloc(Value::Int(crate::integer_factorial(&n)?)));
    }
    if matches!(operation, "comb" | "perm") {
        let &[n, k] = args else {
            let expected = match operation {
                "comb" => "2 arguments",
                _ if args.is_empty() => "at least 1 argument",
                _ => "at most 2 arguments",
            };
            return Err(Error::new(
                "TypeError",
                format!("{operation} expected {expected}, got {}", args.len()),
            ));
        };
        let n = ctx.index_integer(n)?;
        let k = ctx.index_integer(k)?;
        let result = if operation == "comb" {
            crate::integer_comb(&n, &k)?
        } else {
            crate::integer_perm(&n, &k)?
        };
        return Ok(ctx.alloc(Value::Int(result)));
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
    if matches!(
        operation,
        "atan2" | "copysign" | "fmod" | "ldexp" | "pow" | "remainder"
    ) {
        let &[first, second] = args else {
            return Err(Error::new(
                "TypeError",
                format!("{operation} expected 2 arguments, got {}", args.len()),
            ));
        };
        let first = as_double(ctx, first)?;
        let result = match operation {
            "atan2" => first.atan2(as_double(ctx, second)?),
            "copysign" => first.copysign(as_double(ctx, second)?),
            "fmod" => crate::floats::fmod(first, as_double(ctx, second)?)?,
            "pow" => real_power(first, as_double(ctx, second)?)?,
            "remainder" => ieee_remainder(first, as_double(ctx, second)?)?,
            _ => scale_by_power_of_two(ctx, first, second)?,
        };
        return Ok(ctx.alloc(Value::Float(result)));
    }
    if operation == "sumprod" {
        let &[left, right] = args else {
            return Err(Error::new(
                "TypeError",
                format!("sumprod expected 2 arguments, got {}", args.len()),
            ));
        };
        return sumprod(ctx, left, right);
    }
    if operation == "dist" {
        let &[p, q] = args else {
            return Err(Error::new(
                "TypeError",
                format!("dist expected 2 arguments, got {}", args.len()),
            ));
        };
        let p = point_coordinates(ctx, p)?;
        let q = point_coordinates(ctx, q)?;
        if p.len() != q.len() {
            return Err(Error::new(
                "ValueError",
                "both points must have the same number of dimensions",
            ));
        }
        let mut differences = SmallVec::<[f64; 16]>::new();
        differences
            .try_reserve_exact(p.len())
            .map_err(|_| Error::new("MemoryError", "math.dist coordinates"))?;
        for (px, qx) in p.into_iter().zip(q) {
            let px = as_double(ctx, px)?;
            let qx = as_double(ctx, qx)?;
            differences.push((px - qx).abs());
        }
        return Ok(ctx.alloc(Value::Float(vector_norm(&differences))));
    }
    if operation == "hypot" {
        let mut coordinates = SmallVec::<[f64; 16]>::new();
        coordinates
            .try_reserve_exact(args.len())
            .map_err(|_| Error::new("MemoryError", "math.hypot coordinates"))?;
        for &arg in args {
            coordinates.push(as_double(ctx, arg)?.abs());
        }
        return Ok(ctx.alloc(Value::Float(vector_norm(&coordinates))));
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
        let mut result = logarithm(ctx, *value, f64::ln)?;
        if let Some(base) = base {
            let denominator = logarithm(ctx, *base, f64::ln)?;
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
    if matches!(operation, "log2" | "log10") {
        let log = if operation == "log10" {
            crate::floats::log10
        } else {
            f64::log2
        };
        let result = logarithm(ctx, value, log)?;
        return Ok(ctx.alloc(Value::Float(result)));
    }
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
    let checked_unary: Option<fn(f64) -> crate::Result<f64>> = match operation {
        "acos" => Some(crate::floats::acos),
        "asin" => Some(crate::floats::asin),
        "degrees" => Some(|x| Ok(crate::floats::imre_float_degrees(x))),
        "radians" => Some(|x| Ok(crate::floats::imre_float_radians(x))),
        "log1p" => Some(crate::floats::log1p),
        _ => None,
    };
    if let Some(function) = checked_unary {
        let result = function(as_double(ctx, value)?)?;
        return Ok(ctx.alloc(Value::Float(result)));
    }
    if operation == "ulp" {
        let result = ulp(as_double(ctx, value)?);
        return Ok(ctx.alloc(Value::Float(result)));
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
    let unary: Option<fn(f64) -> f64> = match operation {
        "asinh" => Some(f64::asinh),
        "atan" => Some(|x| atan(x)),
        // The platform libm retains precision for inputs just above -1.
        "atanh" => Some(|x| atanh(x)),
        "cos" => Some(f64::cos),
        "cosh" => Some(f64::cosh),
        "erfc" => Some(|x| erfc(x)),
        "exp" => Some(f64::exp),
        "fabs" => Some(f64::abs),
        "sin" => Some(f64::sin),
        "sinh" => Some(f64::sinh),
        "tan" => Some(f64::tan),
        "tanh" => Some(f64::tanh),
        _ => None,
    };
    if let Some(unary) = unary {
        let x = as_double(ctx, value)?;
        if x.is_infinite() && matches!(operation, "cos" | "sin" | "tan") {
            return Err(Error::new(
                "ValueError",
                format!("expected a finite input, got {}", crate::floats::render(x)),
            ));
        }
        if operation == "atanh" && x.abs() >= 1.0 {
            return Err(Error::new(
                "ValueError",
                format!(
                    "expected a number between -1 and 1, got {}",
                    crate::floats::render(x)
                ),
            ));
        }
        let result = unary(x);
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
    if operation == "frexp" {
        return frexp(ctx, value);
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
    builtins::round_float_to_integer(ctx, number, slot)
}

/// Read tuple storage directly, including for tuple subclasses. Other points
/// must be fully consumed and kept alive before any coordinate is converted.
fn point_coordinates(ctx: &mut Context, point: Id) -> Result<Vec<Id>> {
    if let Some(items) = ctx.tuple_items(point)? {
        return Ok(items);
    }
    let snapshot = ctx.collect_iterable(point, true)?;
    Ok(ctx.tuple_items(snapshot)?.expect("collected a tuple"))
}

/// Compute the Euclidean norm of absolute coordinates using CPython's compensated
/// sum of squares and its correction for rounding the square root.
fn vector_norm(coordinates: &[f64]) -> f64 {
    let max = coordinates.iter().copied().fold(0.0, f64::max);
    if max.is_infinite() {
        return max;
    }
    if coordinates.iter().any(|x| x.is_nan()) {
        return f64::NAN;
    }
    if coordinates.len() <= 1 || max == 0.0 {
        return max;
    }

    let mut exponent = 0;
    // SAFETY: frexp writes one exponent to the supplied C int.
    unsafe { c_frexp(max, &mut exponent) };
    // Scaling each coordinate with ldexp also works when a subnormal maximum
    // would require a scale factor too large to represent as a double.
    let mut sum = 1.0;
    let mut product_errors = 0.0;
    let mut sum_errors = 0.0;
    let mut add_product = |x: f64, y: f64| {
        let product = x * y;
        product_errors += x.mul_add(y, -product);
        let next = sum + product;
        sum_errors += (sum - next) + product;
        sum = next;
        (sum - 1.0) + (product_errors + sum_errors)
    };
    let mut squared_norm = 0.0;
    for &x in coordinates {
        let scaled = ldexp(x, -exponent);
        squared_norm = add_product(scaled, scaled);
    }
    let h = squared_norm.sqrt();
    let residual = add_product(-h, h);
    ldexp(h + residual / (2.0 * h), exponent)
}

/// Multiply exact numeric factors without boxing each floating-point intermediate.
fn float_product(ctx: &Context, items: &[Id], start: Id) -> Option<(usize, f64)> {
    let mut total = match ctx.heap.get(start) {
        Value::Float(value) => *value,
        Value::Int(value) if matches!(ctx.heap.get(*items.first()?), Value::Float(_)) => {
            value.as_small()? as f64
        }
        _ => return None,
    };
    let mut consumed = 0;
    for &item in items {
        let factor = match ctx.heap.get(item) {
            Value::Float(value) => *value,
            Value::Int(value) => match value.as_small() {
                Some(n) => n as f64,
                None => break,
            },
            _ => break,
        };
        total *= factor;
        consumed += 1;
    }
    Some((consumed, total))
}

fn prod(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let count = args.len() + kwargs.len();
    if count > 2 {
        let kind = if args.is_empty() { "keyword " } else { "" };
        return Err(Error::new(
            "TypeError",
            format!("prod() takes at most 2 {kind}arguments ({count} given)"),
        ));
    }
    let &[values] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "prod() takes exactly 1 positional argument ({} given)",
                args.len()
            ),
        ));
    };
    let mut product = match kwargs.first() {
        None => ctx.int(1),
        Some((key, start)) if key == "start" => *start,
        Some((key, _)) => {
            return Err(crate::dynamic::calls::unexpected_keyword_error(
                "prod",
                key,
                &["start"],
            ));
        }
    };

    let iterator = ctx.iterator(values)?;
    ctx.heap.keep(iterator);
    // CPython unboxes these exact starts even for empty and user-defined iterators.
    if !kwargs.is_empty() {
        match ctx.heap.get(product) {
            Value::Float(f) => product = ctx.alloc(Value::Float(*f)),
            Value::Int(n) => {
                if let Some(n) = n.to_i64()
                    && std::ffi::c_long::try_from(i128::from(n)).is_ok()
                {
                    product = ctx.int(n);
                }
            }
            _ => {}
        }
    }
    let mut consumed = 0;
    // Exact factors cannot run Python code. Consume them from the backing slice and
    // hand the live cursor to generic multiplication at the first unsupported factor.
    if let Value::List(items) | Value::Tuple(items) = ctx.heap.get(values)
        && let Value::Int(initial) = ctx.heap.get(product)
        && let Some(mut total) = initial.as_small()
    {
        for &item in items {
            let Value::Int(factor) = ctx.heap.get(item) else {
                break;
            };
            let Some(next) = factor.as_small().and_then(|n| total.checked_mul(n)) else {
                break;
            };
            total = next;
            consumed += 1;
        }
        let complete = consumed == items.len();
        if consumed != 0 {
            product = ctx.int(total);
        }
        if complete {
            return Ok(product);
        }
    }
    if let Value::List(items) | Value::Tuple(items) = ctx.heap.get(values) {
        let remaining = &items[consumed..];
        if let Some((count, total)) = float_product(ctx, remaining, product) {
            let complete = count == remaining.len();
            if count != 0 {
                product = ctx.alloc(Value::Float(total));
            }
            if complete {
                return Ok(product);
            }
            consumed += count;
        }
        let Value::Iterator { offset, .. } = ctx.heap.get_mut(iterator) else {
            unreachable!("exact list or tuple iterator")
        };
        *offset = consumed;
    }
    let mark = ctx.heap.temporary_mark();
    ctx.heap.keep(product);
    let mut limit = ctx.heap.streaming_collection_limit();
    while let Some(item) = ctx.next_item(iterator)? {
        ctx.heap.keep(item);
        product = ctx.binary("*", product, item)?;
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(product);
        ctx.collect_streaming_garbage(&mut limit);
    }
    Ok(product)
}

/// Release each owner separately because its destructor can observe the remaining owners.
fn release_sumprod_owners<const N: usize>(
    ctx: &mut Context,
    mark: usize,
    owners: [Option<Id>; N],
    result: &Result<Id>,
) {
    let mut mark = mark;
    let mut protect = |value| {
        ctx.heap.keep_at(mark, value);
        mark += 1;
    };
    match result {
        Ok(value) => protect(*value),
        Err(error) => error.roots().for_each(protect),
    }
    let mut end = mark;
    for owner in owners.into_iter().flatten() {
        ctx.heap.keep_at(mark, owner);
        end += 1;
    }
    // First clear call temporaries while every owner and the result are rooted.
    for root in (mark..=end).rev() {
        ctx.discard_temporaries(root, None);
    }
}

struct BoxedSumprod<'a> {
    ctx: &'a mut Context,
    iterators: [Id; 2],
    items: [Option<Id>; 2],
    product: Option<Id>,
    total: Option<Id>,
    pair_mark: usize,
    collection_limit: usize,
}

impl Operands for BoxedSumprod<'_> {
    type Value = Id;
    type Error = Error;

    fn next(&mut self, side: Side) -> Result<Option<Id>> {
        let index = match side {
            Side::Left => 0,
            Side::Right => 1,
        };
        let mark = self.ctx.heap.temporary_mark();
        let item = self.ctx.next_item(self.iterators[index])?;
        self.ctx.discard_temporaries(mark, item);
        self.items[index] = item;
        Ok(item)
    }

    fn number<'a>(&'a self, value: &'a Id) -> Number<'a> {
        match self.ctx.heap.get(*value) {
            Value::Int(n) => Number::Int(n),
            Value::Float(x) => Number::Float(*x),
            Value::Bool(b) => Number::Bool(*b),
            _ => Number::Other,
        }
    }

    fn integer(&mut self, value: i64) -> Id {
        self.ctx.int(value)
    }

    fn float(&mut self, value: f64) -> Id {
        self.ctx.alloc(Value::Float(value))
    }

    fn multiply(&mut self, left: &Id, right: &Id) -> Result<Id> {
        let mark = self.ctx.heap.temporary_mark();
        let term = self.ctx.binary("*", *left, *right)?;
        self.ctx.discard_temporaries(mark, Some(term));
        self.product = Some(term);
        Ok(term)
    }

    fn add(&mut self, left: &Id, right: &Id) -> Result<Id> {
        self.ctx.binary("+", *left, *right)
    }

    fn value_error(&self, message: &'static str) -> Error {
        Error::new("ValueError", message)
    }

    fn end_pair(&mut self, total: &Id) {
        let [left, right] = std::mem::take(&mut self.items);
        let previous = self.total.replace(*total);
        if let Some(product) = self.product.take() {
            let owners = [previous, left, right, Some(product)];
            release_sumprod_owners(self.ctx, self.pair_mark, owners, &Ok(*total));
        } else {
            // The fast lanes consumed exact scalars without observable destructors.
            self.ctx.discard_temporaries(self.pair_mark, Some(*total));
        }
        self.ctx
            .collect_streaming_garbage(&mut self.collection_limit);
    }
}

fn sumprod(ctx: &mut Context, left: Id, right: Id) -> Result<Id> {
    let mark = ctx.heap.temporary_mark();
    let left = ctx.iterator(left)?;
    ctx.discard_temporaries(mark, Some(left));
    let right_mark = ctx.heap.temporary_mark();
    let right = match ctx.iterator(right) {
        Ok(iterator) => iterator,
        Err(error) => return ctx.release_with_result(mark, Err(error)),
    };
    ctx.discard_temporaries(right_mark, Some(right));
    let mut operands = BoxedSumprod {
        iterators: [left, right],
        items: [None; 2],
        product: None,
        total: None,
        pair_mark: ctx.heap.temporary_mark(),
        collection_limit: ctx.heap.streaming_collection_limit(),
        ctx,
    };
    let result = sumprod::reduce(&mut operands);
    let [left_item, right_item] = operands.items;
    let owners = [
        Some(left),
        Some(right),
        operands.total,
        left_item,
        right_item,
        operands.product,
    ];
    release_sumprod_owners(operands.ctx, mark, owners, &result);
    result
}

fn nextafter(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let count = args.len() + kwargs.len();
    if count > 3 {
        let kind = if args.is_empty() { "keyword " } else { "" };
        return Err(Error::new(
            "TypeError",
            format!("nextafter() takes at most 3 {kind}arguments ({count} given)"),
        ));
    }
    let &[x, y] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "nextafter() takes exactly 2 positional arguments ({} given)",
                args.len()
            ),
        ));
    };
    let steps = match kwargs.first() {
        None => super::NONE,
        Some((key, steps)) if key == "steps" => *steps,
        Some((key, _)) => {
            return Err(crate::dynamic::calls::unexpected_keyword_error(
                "nextafter",
                key,
                &["steps"],
            ));
        }
    };
    let x = as_double(ctx, x)?;
    let y = as_double(ctx, y)?;
    let result = if steps == super::NONE {
        c_nextafter(x, y)
    } else {
        let steps = ctx.index_integer(steps)?;
        let steps = steps.big();
        if steps.sign() == num_bigint::Sign::Minus {
            return Err(Error::new(
                "ValueError",
                "steps must be a non-negative integer",
            ));
        }
        nextafter_steps(x, y, steps.to_u64().unwrap_or(u64::MAX))
    };
    Ok(ctx.alloc(Value::Float(result)))
}

/// Advance by binary64 encodings, counting the two signed zeros as one point.
fn nextafter_steps(x: f64, y: f64, steps: u64) -> f64 {
    if steps == 0 || x.is_nan() {
        return x;
    }
    if y.is_nan() {
        return y;
    }
    let from = x.to_bits();
    let to = y.to_bits();
    if x.is_sign_negative() == y.is_sign_negative() {
        let bits = if from < to {
            from + steps.min(to - from)
        } else {
            from - steps.min(from - to)
        };
        return f64::from_bits(bits);
    }
    let to_zero = x.abs().to_bits();
    let from_zero = y.abs().to_bits();
    if steps >= to_zero + from_zero {
        y
    } else if steps <= to_zero {
        // Approaching zero retains the starting sign until we cross it.
        f64::from_bits(from - steps)
    } else {
        f64::from_bits(steps - to_zero).copysign(y)
    }
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
fn logarithm(ctx: &mut Context, value: Id, log: fn(f64) -> f64) -> Result<f64> {
    let result = match ctx.heap.get(ctx.scalar_value(value)) {
        Value::Int(n) => crate::floats::int_logarithm(n, log),
        Value::Bool(b) => crate::floats::int_logarithm(&crate::Int::from(i64::from(*b)), log),
        _ => crate::floats::float_logarithm(as_double(ctx, value)?, log),
    };
    Ok(result?)
}

/// Add non-finite inputs in sequence so the target quiets signaling NaNs and
/// selects their payloads using the same operation as CPython.
#[cold]
#[inline(never)]
fn fsum_add_special(sum: f64, value: f64) -> f64 {
    sum + value
}

/// Accumulate non-overlapping partial sums, as in CPython's math_fsum.
fn fsum(ctx: &mut Context, values: Id) -> Result<Id> {
    let iterator = ctx.iterator(values)?;
    ctx.heap.keep(iterator);
    let mark = ctx.heap.temporary_mark();
    let mut limit = ctx.heap.streaming_collection_limit();
    let mut partials = SmallVec::<[f64; 32]>::new();
    let mut special_sum = 0.0_f64;
    let mut inf_sum = 0.0_f64;

    while let Some(item) = ctx.next_item(iterator)? {
        ctx.heap.keep(item);
        let mut x = as_double(ctx, item)?;
        ctx.heap.release_temporaries_since(mark);
        ctx.collect_streaming_garbage(&mut limit);

        if !x.is_finite() {
            if x.is_infinite() {
                inf_sum += x;
            }
            special_sum = fsum_add_special(special_sum, x);
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

/// Raise two real numbers to a floating-point power with Python's domain and range errors.
fn real_power(base: f64, exponent: f64) -> Result<f64> {
    let result = base.powf(exponent);
    if base.is_finite() && exponent.is_finite() {
        if result.is_nan() || (base == 0.0 && exponent < 0.0) {
            return Err(Error::new("ValueError", "math domain error"));
        }
        if result.is_infinite() {
            return Err(Error::new("OverflowError", "math range error"));
        }
    }
    Ok(result)
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

/// Return the spacing above the magnitude, or below it at the largest finite float.
fn ulp(value: f64) -> f64 {
    if value.is_nan() {
        return value;
    }
    let magnitude = value.abs();
    if magnitude.is_infinite() {
        return magnitude;
    }
    if magnitude == f64::MAX {
        magnitude - magnitude.next_down()
    } else {
        magnitude.next_up() - magnitude
    }
}

/// Scale by a power of two with a single rounding, including for subnormal results.
fn scale_by_power_of_two(ctx: &Context, x: f64, exponent: Id) -> Result<f64> {
    let exponent = ctx
        .as_int(exponent)
        .map_err(|_| Error::new("TypeError", "Expected an int as second argument to ldexp."))?;
    if x == 0.0 || !x.is_finite() {
        return Ok(x);
    }
    let exponent = exponent.big();
    let exponent = exponent.to_i32().unwrap_or_else(|| {
        if exponent.sign() == num_bigint::Sign::Minus {
            i32::MIN
        } else {
            i32::MAX
        }
    });
    let result = ldexp(x, exponent);
    if result.is_infinite() {
        return Err(Error::new("OverflowError", "math range error"));
    }
    Ok(result)
}

/// Reduce by the nearest multiple of the divisor, choosing the even multiple on ties.
fn ieee_remainder(x: f64, y: f64) -> Result<f64> {
    // CPython returns the first NaN operand, including its sign and payload.
    if x.is_nan() {
        return Ok(x);
    }
    if y.is_nan() {
        return Ok(y);
    }
    if x.is_infinite() || y == 0.0 {
        return Err(Error::new("ValueError", "math domain error"));
    }
    Ok(remainder(x, y))
}

unsafe extern "C" {
    safe fn atan(x: f64) -> f64;
    safe fn atanh(x: f64) -> f64;
    safe fn erfc(x: f64) -> f64;
    safe fn remainder(x: f64, y: f64) -> f64;
    safe fn ldexp(x: f64, exponent: std::ffi::c_int) -> f64;
    #[link_name = "nextafter"]
    safe fn c_nextafter(x: f64, y: f64) -> f64;
    #[link_name = "frexp"]
    fn c_frexp(x: f64, exponent: *mut std::ffi::c_int) -> f64;
}

/// Split a binary float into its signed mantissa and base-two exponent.
fn frexp(ctx: &mut Context, value: Id) -> Result<Id> {
    let x = as_double(ctx, value)?;
    let mut exponent = 0;
    let mantissa = if x.is_finite() && x != 0.0 {
        // SAFETY: frexp writes a single C int to the supplied exponent.
        unsafe { c_frexp(x, &mut exponent) }
    } else {
        // CPython preserves the input bits and returns exponent zero for these cases.
        x
    };
    let mantissa = ctx.alloc(Value::Float(mantissa));
    let exponent = ctx.int(i64::from(exponent));
    Ok(ctx.alloc(Value::Tuple(vec![mantissa, exponent])))
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
