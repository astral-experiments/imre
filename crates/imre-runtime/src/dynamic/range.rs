//! Exact bounds and lazy integer cursors for Python's boxed range.

use std::ffi::c_long;

use num_bigint::BigInt;
use num_traits::{Signed, Zero};

use super::{Context, Error, Id, Result, Value};
use crate::Int;

pub(super) const METHODS: &[&str] = &[
    "__bool__",
    "__contains__",
    "__eq__",
    "__ge__",
    "__getitem__",
    "__gt__",
    "__hash__",
    "__iter__",
    "__le__",
    "__len__",
    "__lt__",
    "__ne__",
    "__repr__",
    "__reversed__",
    "count",
    "index",
];

#[derive(Clone)]
pub(super) struct Range {
    pub(super) start: Int,
    pub(super) stop: Int,
    pub(super) step: Int,
    pub(super) len: Int,
}

impl Range {
    /// Keep the mathematical length even when Python's `len(range(...))` would overflow.
    pub(super) fn new(start: Int, stop: Int, step: Int) -> Result<Self> {
        if step.as_small() == Some(0) {
            return Err(Error::new("ValueError", "range() arg 3 must not be zero"));
        }
        let delta = stop.big().as_ref() - start.big().as_ref();
        let stride = step.big();
        let len = if delta.is_zero() || delta.sign() != stride.sign() {
            Int::from(0)
        } else {
            Int::new((delta.abs() - 1) / stride.abs() + 1)
        };
        Ok(Self {
            start,
            stop,
            step,
            len,
        })
    }

    /// Compare the represented sequences without visiting their elements.
    pub(super) fn equal(&self, other: &Self) -> bool {
        if !self.len.cmp(&other.len).is_eq() {
            return false;
        }
        match self.len.as_small() {
            Some(0) => true,
            len => {
                self.start.cmp(&other.start).is_eq()
                    && (len == Some(1) || self.step.cmp(&other.step).is_eq())
            }
        }
    }

    pub(super) fn at(&self, index: &BigInt) -> Int {
        Int::new(self.start.big().as_ref() + self.step.big().as_ref() * index)
    }

    /// Return the position of an exact integer without traversing the range.
    pub(super) fn index_of(&self, value: &Int) -> Option<Int> {
        let value = value.big();
        let start = self.start.big();
        let stop = self.stop.big();
        let step = self.step.big();
        let in_bounds = if step.is_positive() {
            start <= value && value < stop
        } else {
            stop < value && value <= start
        };
        if !in_bounds {
            return None;
        }
        let delta = value.as_ref() - start.as_ref();
        (&delta % step.as_ref())
            .is_zero()
            .then(|| Int::new(delta / step.as_ref()))
    }

    /// The reverse cursor starts at the last reachable element, even for an unaligned stop.
    pub(super) fn iter(&self, reverse: bool) -> Cursor {
        let (current, step) = if reverse {
            (
                self.at(&(self.len.big().as_ref() - 1)),
                Int::new(-self.step.big().as_ref()),
            )
        } else {
            (self.start.clone(), self.step.clone())
        };
        let kind = if self.fits_c_long(reverse) {
            "range_iterator"
        } else {
            "longrange_iterator"
        };
        Cursor {
            current,
            step,
            remaining: self.len.clone(),
            kind,
        }
    }

    /// CPython exposes distinct iterator types when a C long could overflow on the final increment.
    fn fits_c_long(&self, reverse: bool) -> bool {
        let narrow = |n: &Int| n.to_i64().and_then(|v| c_long::try_from(v).ok());
        let (Some(start), Some(stop), Some(step), Some(len)) = (
            narrow(&self.start),
            narrow(&self.stop),
            narrow(&self.step),
            narrow(&self.len),
        ) else {
            return false;
        };
        if reverse {
            step.checked_neg().is_some() && start.checked_sub(step).is_some()
        } else {
            len == 0
                || stop
                    .checked_add(if step > 0 { step - 1 } else { step + 1 })
                    .is_some()
        }
    }
}

#[derive(Clone)]
pub(super) struct Cursor {
    current: Int,
    step: Int,
    pub(super) remaining: Int,
    pub(super) kind: &'static str,
}

impl Cursor {
    pub(super) fn next(&mut self) -> Result<Option<Int>> {
        if self.remaining.as_small() == Some(0) {
            return Ok(None);
        }
        let next = crate::integer_binary(&self.current, &self.step, 0)?;
        self.remaining = crate::integer_binary(&self.remaining, &Int::from(1), 1)?;
        Ok(Some(std::mem::replace(&mut self.current, next)))
    }
}

/// Dispatch descriptors for ranges and their iterators.
pub(super) fn method(
    ctx: &mut Context,
    owner: &str,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let search = matches!(name, "count" | "index");
    let wrapper = !matches!(name, "count" | "index" | "__reversed__" | "__length_hint__");
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            if !wrapper {
                format!("unbound method {owner}.{name}() needs an argument")
            } else {
                format!("descriptor '{name}' of '{owner}' object needs an argument")
            },
        ));
    };
    let applies = match ctx.heap.get(receiver) {
        Value::Range(_) => owner == "range",
        Value::RangeIterator(cursor) => owner == cursor.kind,
        _ => false,
    };
    if !applies {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for '{owner}' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    }
    if !kwargs.is_empty() {
        let label = if wrapper {
            format!("wrapper {name}()")
        } else {
            format!("{owner}.{name}()")
        };
        return Err(Error::new(
            "TypeError",
            format!("{label} takes no keyword arguments"),
        ));
    }
    if search {
        let [item] = rest else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "range.{name}() takes exactly one argument ({} given)",
                    rest.len()
                ),
            ));
        };
        let Value::Range(range) = ctx.heap.get(receiver) else {
            unreachable!()
        };
        return search_range(ctx, range.clone(), *item, name == "index");
    }
    if matches!(
        name,
        "__eq__"
            | "__ne__"
            | "__lt__"
            | "__le__"
            | "__gt__"
            | "__ge__"
            | "__contains__"
            | "__getitem__"
    ) {
        let [other] = rest else {
            return Err(Error::new(
                "TypeError",
                format!("expected 1 argument, got {}", rest.len()),
            ));
        };
        return match name {
            "__contains__" => ctx.contains(*other, receiver).map(super::builtins::boolean),
            "__getitem__" => ctx.get_item(receiver, *other),
            "__eq__" | "__ne__" if matches!(ctx.heap.get(*other), Value::Range(_)) => Ok(
                super::builtins::boolean(ctx.equal(receiver, *other) == (name == "__eq__")),
            ),
            _ => Ok(super::value::NOT_IMPLEMENTED),
        };
    }
    if !rest.is_empty() {
        let n = rest.len();
        let message = if wrapper {
            format!("expected 0 arguments, got {n}")
        } else {
            format!("{owner}.{name}() takes no arguments ({n} given)")
        };
        return Err(Error::new("TypeError", message));
    }
    match (ctx.heap.get(receiver), name) {
        (_, "__len__") => super::builtins::call(ctx, "len", &[receiver], &[]),
        (_, "__bool__") => ctx.truth(receiver).map(super::builtins::boolean),
        (_, "__hash__") => {
            let hash = ctx.dictionary_hash(receiver)?;
            Ok(ctx.int(hash))
        }
        (_, "__repr__") => {
            let repr = ctx.checked_format(receiver, true)?;
            Ok(ctx.text(repr))
        }
        (Value::Range(range), "__reversed__") => {
            Ok(ctx.alloc(Value::RangeIterator(range.iter(true))))
        }
        (_, "__iter__") => ctx.iterator(receiver),
        (Value::RangeIterator(iter), "__length_hint__") => {
            Ok(ctx.alloc(Value::Int(iter.remaining.clone())))
        }
        (_, "__next__") => ctx
            .next_result(receiver)?
            .ok_or_else(|| Error::new("StopIteration", "")),
        _ => unreachable!("range protocol method"),
    }
}

/// Compare non-integer needles element by element so their equality hooks can run.
fn search_range(ctx: &mut Context, range: Range, item: Id, want_index: bool) -> Result<Id> {
    if matches!(ctx.heap.get(item), Value::Int(_) | Value::Bool(_)) {
        let index = range.index_of(&ctx.as_int(item)?);
        return if want_index {
            index
                .map(|index| ctx.alloc(Value::Int(index)))
                .ok_or_else(|| Error::new("ValueError", "range.index(x): x not in range"))
        } else {
            Ok(ctx.int(i64::from(index.is_some())))
        };
    }

    let mut cursor = range.iter(false);
    let mut count = BigInt::zero();
    while let Some(value) = cursor.next()? {
        let mark = ctx.heap.temporary_mark();
        let candidate = ctx.alloc(Value::Int(value));
        let equal = ctx.checked_equal(candidate, item)?;
        ctx.heap.release_temporaries_since(mark);
        if equal {
            if want_index {
                let index = range.len.big().as_ref() - cursor.remaining.big().as_ref() - 1;
                return Ok(ctx.alloc(Value::Int(Int::new(index))));
            }
            count += 1;
        }
    }
    if want_index {
        Err(Error::new(
            "ValueError",
            "sequence.index(x): x not in sequence",
        ))
    } else {
        Ok(ctx.alloc(Value::Int(Int::new(count))))
    }
}
