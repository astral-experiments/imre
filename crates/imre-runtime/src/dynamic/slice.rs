//! First-class slice objects for runtime-loaded Python.
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};
use crate::Int;

pub(super) fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "slice() takes no keyword arguments",
        ));
    }
    let parts = match args {
        [stop] => [NONE, *stop, NONE],
        [start, stop] => [*start, *stop, NONE],
        [start, stop, step] => [*start, *stop, *step],
        [] => {
            return Err(Error::new(
                "TypeError",
                "slice expected at least 1 argument, got 0",
            ));
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                format!("slice expected at most 3 arguments, got {}", args.len()),
            ));
        }
    };
    Ok(ctx.alloc(Value::Slice(parts)))
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new("TypeError", "slice method needs a receiver"));
    };
    let Value::Slice(parts) = *ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected a slice"));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("slice.{name}() takes no keyword arguments"),
        ));
    }
    if name == "__reduce__" {
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "slice.__reduce__() takes no arguments ({} given)",
                    rest.len()
                ),
            ));
        }
        let class = ctx.builtin("slice");
        let fields = ctx.alloc(Value::Tuple(parts.to_vec()));
        return Ok(ctx.alloc(Value::Tuple(vec![class, fields])));
    }
    let [length] = rest else {
        return Err(Error::new(
            "TypeError",
            format!(
                "slice.indices() takes exactly one argument ({} given)",
                rest.len()
            ),
        ));
    };
    let length = ctx.index_integer(*length)?;
    if length.big().is_negative() {
        return Err(Error::new("ValueError", "length should not be negative"));
    }
    let values =
        indices(ctx, parts, length.big().into_owned())?.map(|n| ctx.alloc(Value::Int(Int::new(n))));
    Ok(ctx.alloc(Value::Tuple(values.to_vec())))
}

/// Compare bounds in order using their actual Python comparison methods.
pub(super) fn compare(ctx: &mut Context, op: &str, left: [Id; 3], right: [Id; 3]) -> Result<Id> {
    for (a, b) in left.into_iter().zip(right) {
        if a != b {
            let equal = ctx.binary("==", a, b)?;
            if !ctx.truth(equal)? {
                return match op {
                    "==" => Ok(FALSE),
                    "!=" => Ok(TRUE),
                    _ => ctx.binary(op, a, b),
                };
            }
        }
    }
    Ok(if matches!(op, "==" | "<=" | ">=") {
        TRUE
    } else {
        FALSE
    })
}

fn components(ctx: &mut Context, [start, stop, step]: [Id; 3]) -> Result<[Option<Int>; 3]> {
    let mut part = |v| {
        if v == NONE {
            Ok(None)
        } else {
            ctx.index_value(v)?.map(Some).ok_or_else(|| {
                Error::new(
                    "TypeError",
                    "slice indices must be integers or None or have an __index__ method",
                )
            })
        }
    };
    // Python invokes __index__ on the step before it inspects either bound.
    let step = part(step)?;
    if step.as_ref().is_some_and(|v| v.as_small() == Some(0)) {
        return Err(Error::new("ValueError", "slice step cannot be zero"));
    }
    Ok([part(start)?, part(stop)?, step])
}

/// Clamp converted slice components to the current length.
pub(super) fn normalize<T: Signed + Ord + Clone>(
    [start, stop, step]: [Option<T>; 3],
    length: T,
) -> [T; 3] {
    let step = step.unwrap_or_else(T::one);
    let reverse = step.is_negative();
    let (low, high) = if reverse {
        (-T::one(), length.clone() - T::one())
    } else {
        (T::zero(), length.clone())
    };
    let bound = |value: Option<T>, default: &T| match value {
        None => default.clone(),
        Some(v) => (if v.is_negative() {
            v + length.clone()
        } else {
            v
        })
        .clamp(low.clone(), high.clone()),
    };
    [
        bound(start, if reverse { &high } else { &low }),
        bound(stop, if reverse { &low } else { &high }),
        step,
    ]
}

fn indices(ctx: &mut Context, parts: [Id; 3], length: BigInt) -> Result<[BigInt; 3]> {
    let bounds = components(ctx, parts)?.map(|v| v.map(|n| n.big().into_owned()));
    Ok(normalize(bounds, length))
}

/// Slicing a range returns its actual new endpoints and step, even for an empty range.
pub(super) fn range(
    ctx: &mut Context,
    parts: [Id; 3],
    start: i64,
    stop: i64,
    step: i64,
) -> Result<Id> {
    let len = super::builtins::range_len(start, stop, step)?;
    let [first, last, stride] = indices(ctx, parts, BigInt::from(len))?;
    let narrow = |n: BigInt| {
        n.to_i64()
            .ok_or_else(|| Error::new("OverflowError", "range bound is too large"))
    };
    Ok(ctx.alloc(Value::Range {
        start: narrow(first * step + start)?,
        stop: narrow(last * step + start)?,
        step: narrow(stride * step)?,
    }))
}

/// Clip bounds only when addressing storage whose actual length fits a machine word.
pub(super) fn sequence_components(ctx: &mut Context, parts: [Id; 3]) -> Result<[Option<i64>; 3]> {
    Ok(components(ctx, parts)?.map(|v| {
        v.map(|n| {
            n.to_i64().unwrap_or_else(|| {
                if n.big().is_negative() {
                    i64::MIN
                } else {
                    i64::MAX
                }
            })
        })
    }))
}

pub(super) fn sequence_bounds(ctx: &mut Context, parts: [Id; 3], len: usize) -> Result<[i64; 3]> {
    Ok(normalize(sequence_components(ctx, parts)?, len as i64))
}
