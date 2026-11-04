//! The public Random state wraps the MT words and offset with a version and the cached Gaussian.
use std::ffi::{c_long, c_ulong};

use num_traits::{Signed, ToPrimitive};

use super::instance::{RandomState, state};
use super::{Context, Error, Id, NONE, Result, Value, objects};
use objects::Generator;

pub(super) fn getstate(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let version = ctx.lookup_attr(receiver, "VERSION")?;
    ctx.heap.keep(version);
    let (words, index) = match state(ctx, receiver) {
        RandomState::Local(local) => local.borrow().state(),
        RandomState::Shared => objects::shared_random_state(),
    };
    let mut items = Vec::with_capacity(words.len() + 1);
    items.extend(words.into_iter().map(|word| ctx.int(i64::from(word))));
    items.push(ctx.int(index as i64));
    let internal = ctx.alloc(Value::Tuple(items));
    ctx.heap.keep(internal);
    let cached = ctx.lookup_attr(receiver, "gauss_next")?;
    Ok(ctx.alloc(Value::Tuple(vec![version, internal, cached])))
}

pub(super) fn setstate(ctx: &mut Context, receiver: Id, outer: Id) -> Result<Id> {
    let zero = ctx.int(0);
    let version = ctx.get_item(outer, zero)?;
    ctx.heap.keep(version);
    let three = ctx.int(3);
    let legacy = if ctx.checked_equal(version, three)? {
        false
    } else {
        let two = ctx.int(2);
        if !ctx.checked_equal(version, two)? {
            let current = ctx.lookup_attr(receiver, "VERSION")?;
            let versions = ctx.alloc(Value::Tuple(vec![version, current]));
            let template =
                ctx.text("state with version %s passed to Random.setstate() of version %s");
            let message = ctx.binary("%", template, versions)?;
            return Err(Error::new("ValueError", ctx.str_value(message)?));
        }
        true
    };
    let fields = ctx.unpack_items(outer, 3, None)?;
    // Python assigns gauss_next during unpacking, even if later MT validation fails.
    ctx.set_attr(receiver, "gauss_next", fields[2])?;
    let internal = if legacy {
        legacy_words(ctx, fields[1]).map_err(|original| {
            if !ctx.is_builtin_error(&original, "ValueError") {
                return original;
            }
            chained(ctx, Error::new("TypeError", ""), original)
        })?
    } else {
        fields[1]
    };
    let generator = restore_generator(ctx, internal)?;
    match state(ctx, receiver) {
        RandomState::Local(local) => *local.borrow_mut() = generator,
        RandomState::Shared => objects::seed_shared(generator),
    }
    Ok(NONE)
}

fn chained(ctx: &mut Context, mut error: Error, original: Error) -> Error {
    let cause = ctx.raised_instance(original);
    error.tracing().cause = Some(cause);
    error.context = cause;
    error
}

fn legacy_words(ctx: &mut Context, values: Id) -> Result<Id> {
    let iter = ctx.iterator(values)?;
    ctx.heap.keep(iter);
    let modulus = ctx.int(1 << 32);
    ctx.heap.keep(modulus);
    let buffer = ctx.alloc(Value::List(Vec::new()));
    let step = ctx.heap.temporary_mark();
    while let Some(value) = ctx.next_item(iter)? {
        ctx.heap.keep(value);
        let word = ctx.binary("%", value, modulus).map_err(|original| {
            if !ctx.is_stop_iteration(&original) {
                return original;
            }
            chained(
                ctx,
                Error::new("RuntimeError", "generator raised StopIteration"),
                original,
            )
        })?;
        super::append(ctx, buffer, word)?;
        ctx.comprehension_step(step);
    }
    let Value::List(items) = ctx.heap.get_mut(buffer) else {
        unreachable!("legacy state buffer")
    };
    let items = std::mem::take(items);
    Ok(ctx.alloc(Value::Tuple(items)))
}

fn restore_generator(ctx: &mut Context, internal: Id) -> Result<Generator> {
    let values = match ctx.heap.get(ctx.scalar_value(internal)) {
        Value::Tuple(values) | Value::NamedTuple { values, .. } => values,
        _ => return Err(Error::new("TypeError", "state vector must be a tuple")),
    };
    if values.len() != Generator::STATE_WORDS + 1 {
        return Err(Error::new("ValueError", "state vector is the wrong size"));
    }
    let mut words = [0; Generator::STATE_WORDS];
    for (word, &id) in words.iter_mut().zip(values) {
        // PyLong_AsUnsignedLong accepts actual integers, then the MT core truncates to u32.
        let integer = ctx
            .as_int(id)
            .map_err(|_| Error::new("TypeError", "an integer is required"))?;
        let n = integer.big();
        if n.is_negative() {
            return Err(Error::new(
                "OverflowError",
                "can't convert negative value to unsigned int",
            ));
        }
        let native = n
            .to_u64()
            .and_then(|n| c_ulong::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C unsigned long",
                )
            })?;
        *word = native as u32;
    }
    let index = ctx.index_c_long(values[Generator::STATE_WORDS])?;
    c_long::try_from(index)
        .map_err(|_| Error::new("OverflowError", "Python int too large to convert to C long"))?;
    Ok(Generator::from_state(words, index)?)
}
