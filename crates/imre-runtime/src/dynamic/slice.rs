//! First-class slice objects for runtime-loaded Python.
use num_bigint::BigInt;
use num_traits::Signed;

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value, value::NOT_IMPLEMENTED};
use crate::Int;

pub(super) const METHODS: &[&str] = &[
    "__eq__",
    "__ge__",
    "__gt__",
    "__hash__",
    "__le__",
    "__lt__",
    "__ne__",
    "__reduce__",
    "__repr__",
    "indices",
];

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
    let wrapper = !matches!(name, "indices" | "__reduce__");
    let Some((&receiver, rest)) = args.split_first() else {
        let message = if wrapper {
            format!("descriptor '{name}' of 'slice' object needs an argument")
        } else {
            format!("unbound method slice.{name}() needs an argument")
        };
        return Err(Error::new("TypeError", message));
    };
    let Value::Slice(parts) = *ctx.heap.get(receiver) else {
        let kind = ctx.type_name(receiver);
        let message = if wrapper {
            format!("descriptor '{name}' requires a 'slice' object but received a '{kind}'")
        } else {
            format!("descriptor '{name}' for 'slice' objects doesn't apply to a '{kind}' object")
        };
        return Err(Error::new("TypeError", message));
    };
    if !kwargs.is_empty() {
        let label = if wrapper {
            format!("wrapper {name}")
        } else {
            format!("slice.{name}")
        };
        return Err(Error::new(
            "TypeError",
            format!("{label}() takes no keyword arguments"),
        ));
    }
    if matches!(name, "__hash__" | "__repr__") {
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("expected 0 arguments, got {}", rest.len()),
            ));
        }
        if name == "__hash__" {
            let hash = ctx.dictionary_hash(receiver)?;
            return Ok(ctx.int(hash));
        }
        let text = ctx.checked_format(receiver, true)?;
        return Ok(ctx.text(text));
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
    let [other] = rest else {
        let message = if wrapper {
            format!("expected 1 argument, got {}", rest.len())
        } else {
            format!(
                "slice.indices() takes exactly one argument ({} given)",
                rest.len()
            )
        };
        return Err(Error::new("TypeError", message));
    };
    if wrapper {
        let Value::Slice(right) = *ctx.heap.get(*other) else {
            return Ok(NOT_IMPLEMENTED);
        };
        let op = match name {
            "__eq__" => "==",
            "__ne__" => "!=",
            "__lt__" => "<",
            "__le__" => "<=",
            "__gt__" => ">",
            "__ge__" => ">=",
            _ => unreachable!("slice comparison"),
        };
        return compare(ctx, op, parts, right);
    }
    let length = ctx.index_integer(*other)?;
    if length.big().is_negative() {
        return Err(Error::new("ValueError", "length should not be negative"));
    }
    let values =
        indices(ctx, parts, length.big().into_owned())?.map(|n| ctx.alloc(Value::Int(Int::new(n))));
    Ok(ctx.alloc(Value::Tuple(values.to_vec())))
}

/// Compare bounds in order using their actual Python comparison methods.
pub(super) fn compare(
    ctx: &mut Context,
    op: &str,
    mut left: [Id; 3],
    mut right: [Id; 3],
) -> Result<Id> {
    let mut pending = smallvec::SmallVec::new();
    'compare: loop {
        for (a, b) in left.into_iter().zip(right) {
            if equal(ctx, a, b, &mut pending)? {
                continue;
            }
            match op {
                "==" => return Ok(FALSE),
                "!=" => return Ok(TRUE),
                _ => match (ctx.heap.get(a), ctx.heap.get(b)) {
                    (Value::Slice(x), Value::Slice(y)) => {
                        (left, right) = (*x, *y);
                        continue 'compare;
                    }
                    _ => return ctx.binary(op, a, b),
                },
            }
        }
        return Ok(if matches!(op, "==" | "<=" | ">=") {
            TRUE
        } else {
            FALSE
        });
    }
}

/// Visit nested slice fields in tuple comparison order without growing the native stack.
fn equal(
    ctx: &mut Context,
    a: Id,
    b: Id,
    pending: &mut smallvec::SmallVec<[(Id, Id); 3]>,
) -> Result<bool> {
    let mut pair = Some((a, b));
    while let Some((a, b)) = pair {
        if a != b {
            if let (Value::Slice(left), Value::Slice(right)) = (ctx.heap.get(a), ctx.heap.get(b)) {
                pending
                    .try_reserve(3)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                pending.extend(
                    left.iter()
                        .copied()
                        .zip(right.iter().copied())
                        .rev()
                        .filter(|(x, y)| x != y),
                );
            } else {
                let equal = ctx.binary("==", a, b)?;
                if !ctx.truth(equal)? {
                    pending.clear();
                    return Ok(false);
                }
            }
        }
        pair = pending.pop();
    }
    Ok(true)
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
pub(super) fn range(ctx: &mut Context, parts: [Id; 3], range: super::range::Range) -> Result<Id> {
    let [first, last, stride] = indices(ctx, parts, range.len.big().into_owned())?;
    let step = Int::new(stride * range.step.big().as_ref());
    Ok(ctx.alloc(Value::Range(super::range::Range::new(
        range.at(&first),
        range.at(&last),
        step,
    )?)))
}

/// Convert Python slice indices before a caller measures potentially mutable storage.
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
