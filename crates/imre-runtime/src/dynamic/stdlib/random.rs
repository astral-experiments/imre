use num_bigint::Sign;

use super::super::range::Range;
use super::{Context, Error, Id, NONE, Result, Value, builtins};
use crate::{Int, objects};

/// Bind positional and keyword arguments after the implicit module generator.
fn arguments<const N: usize>(
    name: &str,
    names: [&str; N],
    positional: usize,
    expected: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<[Option<Id>; N]> {
    let mut values = [None; N];
    for (target, value) in values[..positional].iter_mut().zip(args) {
        *target = Some(*value);
    }
    for (key, value) in kwargs {
        if key == "self" {
            return Err(Error::new(
                "TypeError",
                format!("{name}() got multiple values for argument 'self'"),
            ));
        }
        let Some(index) = names.iter().position(|&param| param == key) else {
            let mut message = format!("{name}() got an unexpected keyword argument '{key}'");
            let parameters = std::iter::once("self").chain(names).collect::<Vec<_>>();
            if let Some(hint) = objects::exceptions::keyword_suggestion(key, &parameters) {
                message.push_str(&format!(". Did you mean '{hint}'?"));
            }
            return Err(Error::new("TypeError", message));
        };
        if values[index].replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() got multiple values for argument '{key}'"),
            ));
        }
    }
    if args.len() > positional {
        let mut given = (args.len() + 1).to_string();
        let keywords = values[positional..].iter().flatten().count();
        if keywords != 0 {
            let suffix = if keywords == 1 { "" } else { "s" };
            given.push_str(&format!(
                " positional arguments (and {keywords} keyword-only argument{suffix})"
            ));
        }
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes {expected} positional arguments but {given} were given"),
        ));
    }
    Ok(values)
}

pub(super) fn randint(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let bounds = arguments("Random.randint", ["a", "b"], 2, "3", args, kwargs)?;
    let [Some(start), Some(stop)] = bounds else {
        let missing: Vec<_> = ["a", "b"]
            .into_iter()
            .zip(bounds)
            .filter_map(|(name, value)| value.is_none().then_some(name))
            .collect();
        return Err(super::super::calls::missing_arguments_error(
            "Random.randint",
            "positional",
            &missing,
        ));
    };
    let start = ctx.index_integer(start)?;
    let stop = ctx.index_integer(stop)?;
    let draw = objects::randint(&start.big(), &stop.big())?;
    Ok(ctx.alloc(Value::Int(Int::new(draw))))
}

pub(super) fn randbytes(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [length] = arguments("Random.randbytes", ["n"], 1, "2", args, kwargs)?;
    let length = length.ok_or_else(|| {
        super::super::calls::missing_arguments_error("Random.randbytes", "positional", &["n"])
    })?;
    // Random.randbytes calls getrandbits(n * 8) before to_bytes indexes n.
    let eight = ctx.int(8);
    let count = ctx.binary("*", length, eight)?;
    let count = ctx.index_integer(count)?;
    let draw = objects::random_bits(&count)?;
    let draw = ctx.alloc(Value::Int(Int::new(draw)));
    let little = ctx.text("little");
    builtins::call(ctx, "int.to_bytes", &[draw, length, little], &[])
}

/// Select the original element through Python's length and subscription protocols.
pub(super) fn choice(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    secure: bool,
) -> Result<Id> {
    let [seq] = arguments("Random.choice", ["seq"], 1, "2", args, kwargs)?;
    let seq = seq.ok_or_else(|| {
        Error::new(
            "TypeError",
            "Random.choice() missing 1 required positional argument: 'seq'",
        )
    })?;
    // Random.choice checks for emptiness and then reads the sampling bound separately.
    let first = builtins::call(ctx, "len", &[seq], &[])?;
    let bound = if ctx.truth(first)? {
        builtins::call(ctx, "len", &[seq], &[])?
    } else {
        first
    };
    let index = crate::objects::choice_index(&ctx.as_int(bound)?, secure)?;
    let key = ctx.alloc(Value::Int(crate::Int::new(index)));
    ctx.get_item(seq, key)
}

fn append(ctx: &mut Context, list: Id, value: Id) -> Result<()> {
    let Value::List(items) = ctx.heap.get_mut(list) else {
        unreachable!()
    };
    items
        .try_reserve(1)
        .map_err(|_| Error::new("MemoryError", ""))?;
    items.push(value);
    Ok(())
}

fn cumulative_weights(ctx: &mut Context, weights: Id) -> Result<Id> {
    let iterator = ctx.iterator(weights)?;
    ctx.heap.keep(iterator);
    let cumulative = ctx.alloc(Value::List(Vec::new()));
    let mark = ctx.heap.temporary_mark();
    let mut limit = ctx.heap.streaming_collection_limit();
    let mut previous = None;
    while let Some(item) = ctx.next_item(iterator)? {
        ctx.heap.keep(item);
        let total = match previous {
            Some(value) => match ctx.binary("+", value, item) {
                Err(error) if ctx.is_stop_iteration(&error) => break,
                result => result?,
            },
            None => item,
        };
        append(ctx, cumulative, total)?;
        previous = Some(total);
        ctx.heap.release_temporaries_since(mark);
        ctx.collect_streaming_garbage(&mut limit);
    }
    Ok(cumulative)
}

/// Run the integer-weights diagnostic with the original iteration exception active.
fn weights_type_error(ctx: &mut Context, weights: Id, error: Error) -> Error {
    let original = ctx.raised_instance(error.clone());
    let previous = ctx.heap.replace_active(original);
    ctx.heap.keep(previous);
    let diagnostic = (|| -> Result<Error> {
        let int = ctx.builtin("int");
        if !ctx.instance_of(weights, int) {
            // isinstance also recognizes an unrelated object's reported __class__.
            match ctx.lookup_attr(weights, "__class__") {
                Ok(class) if ctx.direct_subclass(class, int) => {}
                Ok(_) => return Ok(error),
                Err(missing) if ctx.is_attribute_error(&missing) => return Ok(error),
                Err(failure) => return Err(failure),
            }
        }
        let k = ctx.checked_format(weights, true)?;
        let mut replacement = Error::new(
            "TypeError",
            format!("The number of choices must be a keyword argument: k={k}"),
        );
        replacement.context = original;
        replacement.tracing().cause = Some(NONE);
        Ok(replacement)
    })();
    ctx.heap.replace_active(previous);
    diagnostic.unwrap_or_else(|mut failure| {
        failure.context = original;
        failure
    })
}

/// Bisect on the original sequence, excluding its last boundary as in Random.choices.
fn weighted_index(ctx: &mut Context, cumulative: Id, needle: Id, mut hi: i64) -> Result<i64> {
    if let Some(kind @ ("dict" | "collections.defaultdict" | "mappingproxy")) =
        ctx.builtin_type_name(cumulative)
    {
        // _bisect reads the sequence item slot, which exact native mappings do not supply.
        return Err(Error::new("TypeError", format!("{kind} is not a sequence")));
    }
    if hi == -1 {
        let length = builtins::call(ctx, "len", &[cumulative], &[])?;
        hi = ctx.as_integer(length)?;
    }
    let mut lo = 0;
    let mark = ctx.heap.temporary_mark();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let key = ctx.int(mid);
        let weight = ctx.get_item(cumulative, key)?;
        ctx.heap.keep(weight);
        let less = ctx.binary("<", needle, weight)?;
        if ctx.truth(less)? {
            hi = mid;
        } else {
            lo = mid + 1;
        }
        ctx.heap.release_temporaries_since(mark);
    }
    Ok(lo)
}

pub(super) fn choices(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [population, weights, cumulative, k] = arguments(
        "Random.choices",
        ["population", "weights", "cum_weights", "k"],
        2,
        "from 2 to 3",
        args,
        kwargs,
    )?;
    let population = population.ok_or_else(|| {
        Error::new(
            "TypeError",
            "Random.choices() missing 1 required positional argument: 'population'",
        )
    })?;
    let length = builtins::call(ctx, "len", &[population], &[])?;
    let n = ctx.as_integer(length)?;
    let preparation = ctx.heap.temporary_mark();
    let cumulative = match (
        weights.filter(|&id| id != NONE),
        cumulative.filter(|&id| id != NONE),
    ) {
        (Some(_), Some(_)) => {
            return Err(Error::new(
                "TypeError",
                "Cannot specify both weights and cumulative weights",
            ));
        }
        (Some(weights), None) => Some(match cumulative_weights(ctx, weights) {
            Err(error) if ctx.is_builtin_error(&error, "TypeError") => {
                ctx.heap.release_temporaries_since(preparation);
                for root in error.roots() {
                    ctx.heap.keep(root);
                }
                return Err(weights_type_error(ctx, weights, error));
            }
            result => result?,
        }),
        (None, supplied) => supplied,
    };
    ctx.heap.release_temporaries_since(preparation);
    let weighted = if let Some(cumulative) = cumulative {
        ctx.heap.keep(cumulative);
        let operands = ctx.heap.temporary_mark();
        let length = builtins::call(ctx, "len", &[cumulative], &[])?;
        if ctx.as_integer(length)? != n {
            return Err(Error::new(
                "ValueError",
                "The number of weights does not match the population",
            ));
        }
        let last = ctx.int(-1);
        let last = ctx.get_item(cumulative, last)?;
        ctx.heap.keep(last);
        let zero = ctx.alloc(Value::Float(0.0));
        let total = ctx.binary("+", last, zero)?;
        ctx.heap.release_temporaries_since(operands);
        ctx.heap.keep(total);
        let validation = ctx.heap.temporary_mark();
        let zero = ctx.alloc(Value::Float(0.0));
        let nonpositive = ctx.binary("<=", total, zero)?;
        if ctx.truth(nonpositive)? {
            return Err(Error::new(
                "ValueError",
                "Total of weights must be greater than zero",
            ));
        }
        ctx.heap.release_temporaries_since(validation);
        if !super::math::as_double(ctx, total)?.is_finite() {
            return Err(Error::new("ValueError", "Total of weights must be finite"));
        }
        ctx.heap.release_temporaries_since(validation);
        Some((cumulative, total))
    } else {
        None
    };
    let k = k.unwrap_or_else(|| ctx.int(1));
    let k = ctx.index_integer(k)?;
    let mut repeat = objects::Repeat::new(NONE, Some(&k))?;
    let result = ctx.alloc(Value::List(Vec::new()));
    let mark = ctx.heap.temporary_mark();
    let mut limit = ctx.heap.streaming_collection_limit();
    while repeat.next().is_some() {
        let draw = objects::imre_random_float();
        let index = if let Some((cumulative, total)) = weighted {
            let draw = ctx.alloc(Value::Float(draw));
            let needle = ctx.binary("*", draw, total)?;
            ctx.heap.keep(needle);
            let index = weighted_index(ctx, cumulative, needle, n - 1)?;
            ctx.heap.release_temporaries_since(mark);
            index
        } else {
            (draw * n as f64).floor() as i64
        };
        let index = ctx.int(index);
        let item = ctx.get_item(population, index)?;
        append(ctx, result, item)?;
        ctx.heap.release_temporaries_since(mark);
        ctx.collect_streaming_garbage(&mut limit);
    }
    Ok(result)
}

/// Shuffle by subscription, preserving the read and assignment order of Python's tuple swap.
pub(super) fn shuffle(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [x] = arguments("Random.shuffle", ["x"], 1, "2", args, kwargs)?;
    let x = x.ok_or_else(|| {
        Error::new(
            "TypeError",
            "Random.shuffle() missing 1 required positional argument: 'x'",
        )
    })?;
    let roots = ctx.heap.temporary_mark();
    ctx.heap.keep(x);
    let step = ctx.heap.temporary_mark();
    let result: Result<Id> = (|| {
        let length = builtins::call(ctx, "len", &[x], &[])?;
        let length = ctx.as_integer(length)? as usize;
        for indices in objects::shuffle_swaps(length) {
            ctx.heap.release_temporaries_since(step);
            let (i, j) = indices?;
            let i = ctx.int(i as i64);
            let j = ctx.int(j as i64);
            let right = ctx.get_item(x, j)?;
            ctx.heap.keep(right);
            let left = ctx.get_item(x, i)?;
            ctx.heap.keep(left);
            ctx.put_item(x, i, right)?;
            ctx.put_item(x, j, left)?;
        }
        Ok(NONE)
    })();
    ctx.heap.release_temporaries_since(roots);
    if let Err(error) = &result {
        for root in error.roots() {
            ctx.heap.keep(root);
        }
    }
    result
}

/// Format the original Python arguments, including user-defined __format__ methods.
fn empty_range(ctx: &mut Context, operands: &[Id]) -> Result<Id> {
    let mut message = ctx.text("empty range in randrange(");
    let separator = ctx.text(", ");
    for (i, &operand) in operands.iter().enumerate() {
        if i != 0 {
            message = ctx.concat_strings(message, separator)?;
        }
        let field = builtins::format_value(ctx, operand, None)?;
        message = ctx.concat_strings(message, field)?;
    }
    let closing = ctx.text(")");
    let message = ctx.concat_strings(message, closing)?;
    Err(ctx.exception("ValueError", &[message]))
}

pub(super) fn randrange(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [start, stop, step] = arguments(
        "Random.randrange",
        ["start", "stop", "step"],
        3,
        "from 2 to 4",
        args,
        kwargs,
    )?;
    let start = start.ok_or_else(|| {
        Error::new(
            "TypeError",
            "Random.randrange() missing 1 required positional argument: 'start'",
        )
    })?;
    let istart = ctx.index_integer(start)?;
    let step = step.unwrap_or_else(|| ctx.int(1));
    let stop = stop.unwrap_or(NONE);
    if stop == NONE {
        // CPython checks identity with the exact integer 1 before coercing step.
        if !matches!(ctx.heap.get(step), Value::Int(n) if n.as_small() == Some(1)) {
            return Err(Error::new("TypeError", "Missing a non-None stop argument"));
        }
        let bound = istart.big();
        if bound.sign() != Sign::Plus {
            return Err(Error::new("ValueError", "empty range for randrange()"));
        }
        let selected = objects::random_below(bound.magnitude())?;
        return Ok(ctx.alloc(Value::Int(Int::new(selected.into()))));
    }
    let istop = ctx.index_integer(stop)?;
    let istep = ctx.index_integer(step)?;
    if istep.as_small() == Some(0) {
        return Err(Error::new("ValueError", "zero step for randrange()"));
    }
    let range = Range::new(istart, istop, istep)?;
    if range.len.as_small() == Some(0) {
        return if range.step.as_small() == Some(1) {
            empty_range(ctx, &[start, stop])
        } else {
            empty_range(ctx, &[start, stop, step])
        };
    }
    let index = objects::random_below(range.len.big().magnitude())?;
    Ok(ctx.alloc(Value::Int(range.at(&index.into()))))
}
