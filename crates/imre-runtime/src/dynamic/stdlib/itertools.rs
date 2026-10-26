use super::{Context, Error, Id, Result, Value};
use crate::objects::CountValue;

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
    let (receiver, rest) = args.split_first().expect("bound count method");
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
        "__next__" => advance(ctx, *receiver),
        _ => unreachable!("count method"),
    }
}
