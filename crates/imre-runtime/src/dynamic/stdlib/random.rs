use num_bigint::Sign;

use super::super::range::Range;
use super::{Context, Error, Id, NONE, Result, Value, builtins};
use crate::{Int, objects};

/// Bind positional and keyword arguments after the implicit module generator.
fn arguments<const N: usize>(
    name: &str,
    names: [&str; N],
    expected: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<[Option<Id>; N]> {
    let mut values = [None; N];
    for (target, value) in values.iter_mut().zip(args) {
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
    if args.len() > N {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes {expected} positional arguments but {} were given",
                args.len() + 1
            ),
        ));
    }
    Ok(values)
}

/// Select the original element through Python's length and subscription protocols.
pub(super) fn choice(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    secure: bool,
) -> Result<Id> {
    let [seq] = arguments("Random.choice", ["seq"], "2", args, kwargs)?;
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
