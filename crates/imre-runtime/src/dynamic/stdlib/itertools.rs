use super::{Context, Error, Id, Result, Value};
use crate::objects::CountValue;

/// Build the shared repeat cursor, converting a supplied count exactly once.
pub(super) fn repeat(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let missing = || {
        Error::new(
            "TypeError",
            "repeat() missing required argument 'object' (pos 1)",
        )
    };
    if args.is_empty() && kwargs.len() <= 2 && !kwargs.iter().any(|(key, _)| key == "object") {
        return Err(missing());
    }
    let [value, times] = super::super::builtins::bind_method_arguments(
        "repeat",
        args,
        kwargs,
        ["object", "times"],
        2,
    )?;
    let times = times.map(|id| ctx.index_integer(id)).transpose()?;
    let cursor = crate::objects::Repeat::new(value.ok_or_else(missing)?, times.as_ref())?;
    Ok(ctx.alloc(Value::Repeat(cursor)))
}

fn number(ctx: &Context, id: Id) -> Result<CountValue> {
    match ctx.heap.get(id) {
        Value::Int(n) => Ok(CountValue::Int(n.clone())),
        Value::Float(n) => Ok(CountValue::Float(*n)),
        Value::Bool(b) => Ok(CountValue::Int(crate::Int::from(i64::from(*b)))),
        _ => Err(Error::new("TypeError", "a number is required")),
    }
}

/// Retain both Python operands without performing the first addition.
pub(super) fn count(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [start, step] =
        super::super::builtins::bind_method_arguments("count", args, kwargs, ["start", "step"], 2)?;
    let mut current = start.unwrap_or_else(|| ctx.int(0));
    let step = step.unwrap_or_else(|| ctx.int(1));
    number(ctx, current)?;
    let delta = number(ctx, step)?;
    if let Value::Bool(b) = *ctx.heap.get(current)
        && matches!(delta, CountValue::Int(n) if n.as_small() == Some(1))
    {
        current = ctx.int(i64::from(b));
    }
    Ok(ctx.alloc(Value::Count { current, step }))
}

/// Commit the next value only after the shared numeric progression succeeds.
pub(in crate::dynamic) fn advance(ctx: &mut Context, id: Id) -> Result<Id> {
    let Value::Count { current, step } = *ctx.heap.get(id) else {
        unreachable!("count cursor")
    };
    let following = number(ctx, current)?.successor(&number(ctx, step)?)?;
    let following = ctx.alloc(match following {
        CountValue::Int(n) => Value::Int(n),
        CountValue::Float(n) => Value::Float(n),
    });
    let Value::Count { current, .. } = ctx.heap.get_mut(id) else {
        unreachable!("count cursor")
    };
    Ok(std::mem::replace(current, following))
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (receiver, rest) = args.split_first().expect("bound itertools method");
    if name == "__length_hint__" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "repeat.__length_hint__() takes no keyword arguments",
            ));
        }
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "repeat.__length_hint__() takes no arguments ({} given)",
                    rest.len()
                ),
            ));
        }
        let Value::Repeat(repeat) = ctx.heap.get(*receiver) else {
            unreachable!("repeat method")
        };
        let n = repeat
            .remaining
            .ok_or_else(|| Error::new("TypeError", "len() of unsized object"))?;
        return Ok(ctx.int(n as i64));
    }
    if !rest.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("expected 0 arguments, got {}", rest.len()),
        ));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("wrapper {name}() takes no keyword arguments"),
        ));
    }
    match name {
        "__iter__" => Ok(*receiver),
        "__next__" => ctx
            .next_result(*receiver)?
            .ok_or_else(|| Error::new("StopIteration", "")),
        _ => unreachable!("itertools method"),
    }
}
