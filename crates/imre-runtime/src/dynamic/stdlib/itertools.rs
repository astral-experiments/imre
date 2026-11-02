use super::{Context, Error, Id, Result, Value};

/// The input's iterator is released on exhaustion. Only yielded objects enter the replay buffer.
#[derive(Clone)]
pub(in crate::dynamic) struct Cycle {
    source: Option<Id>,
    saved: Vec<Id>,
    index: usize,
}

impl Cycle {
    pub(in crate::dynamic) fn children(&self, visit: &mut crate::dynamic::value::MarkQueue<'_>) {
        visit.extend(self.source);
        visit.extend(&self.saved);
    }
}

pub(super) fn cycle_allocate(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [base, class, iterable] = args else {
        return Err(Error::new(
            "TypeError",
            "invalid cycle allocation arguments",
        ));
    };
    if !kwargs.is_empty()
        || !matches!(ctx.heap.get(*class), Value::Class { ancestors, .. }
        if *class == *base || ancestors.contains(base))
    {
        return Err(Error::new("TypeError", "expected a cycle subclass"));
    }
    let source = ctx.iterator(*iterable)?;
    ctx.heap.keep(source);
    let result = ctx.instance(*class, None);
    let Value::Instance { cycle, .. } = ctx.heap.get_mut(result) else {
        unreachable!("fresh cycle instance")
    };
    *cycle = Some(Box::new(Cycle {
        source: Some(source),
        saved: Vec::new(),
        index: 0,
    }));
    Ok(result)
}

pub(super) fn cycle_next(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [receiver] = args else {
        return Err(Error::new("TypeError", "expected a cycle instance"));
    };
    let Value::Instance {
        cycle: Some(state), ..
    } = ctx.heap.get(*receiver)
    else {
        return Err(Error::new("TypeError", "expected a cycle instance"));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "cycle.__next__() takes no keyword arguments",
        ));
    }
    // Advancing the source can call Python and reenter this same cycle.
    let item = match state.source {
        Some(source) => ctx.next_item(source)?,
        None => None,
    };
    let Value::Instance {
        cycle: Some(state), ..
    } = ctx.heap.get_mut(*receiver)
    else {
        unreachable!("live cycle instance")
    };
    if let Some(item) = item {
        state
            .saved
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        state.saved.push(item);
        return Ok(item);
    }
    state.source = None;
    let item = state
        .saved
        .get(state.index)
        .copied()
        .ok_or_else(|| Error::new("StopIteration", ""))?;
    state.index = (state.index + 1) % state.saved.len();
    Ok(item)
}

pub(super) fn filterfalse_allocate(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [base, class, predicate, iterable] = args else {
        return Err(Error::new(
            "TypeError",
            "invalid filterfalse allocation arguments",
        ));
    };
    if !kwargs.is_empty()
        || !matches!(ctx.heap.get(*class), Value::Class { ancestors, .. }
        if *class == *base || ancestors.contains(base))
    {
        return Err(Error::new("TypeError", "expected a filterfalse subclass"));
    }
    let source = ctx.iterator(*iterable)?;
    ctx.heap.keep(source);
    let state = ctx.alloc(Value::Tuple(vec![source, *predicate]));
    ctx.heap.keep(state);
    let result = ctx.instance(*class, None);
    let Value::Instance { member_values, .. } = ctx.heap.get_mut(result) else {
        unreachable!("fresh filterfalse instance")
    };
    // The declaring class keys this private slot, so subclass descriptors cannot intercept it.
    member_values
        .get_or_insert_with(Default::default)
        .insert(*base, state);
    Ok(result)
}

pub(super) fn filterfalse_state(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if let [base, receiver] = args
        && kwargs.is_empty()
        && matches!(ctx.heap.get(*base), Value::Class { .. })
        && let Value::Instance {
            member_values: Some(values),
            ..
        } = ctx.heap.get(*receiver)
        && let Some(&state) = values.get(base)
    {
        return Ok(state);
    }
    Err(Error::new("TypeError", "expected a filterfalse instance"))
}

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

fn number(ctx: &Context, id: Id) -> Result<()> {
    if matches!(
        ctx.heap.get(ctx.scalar_value(id)),
        Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Complex(_)
    ) || ["__index__", "__int__", "__float__"]
        .iter()
        .any(|name| ctx.slot(id, name).is_some())
    {
        Ok(())
    } else {
        Err(Error::new("TypeError", "a number is required"))
    }
}

/// Retain both Python operands without performing the first addition.
pub(super) fn count(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [start, step] =
        super::super::builtins::bind_method_arguments("count", args, kwargs, ["start", "step"], 2)?;
    let mut current = start.unwrap_or_else(|| ctx.int(0));
    let step = step.unwrap_or_else(|| ctx.int(1));
    number(ctx, current)?;
    number(ctx, step)?;
    if let Value::Bool(b) = *ctx.heap.get(current)
        && (matches!(ctx.heap.get(step), Value::Bool(true))
            || matches!(ctx.heap.get(step), Value::Int(n) if n.as_small() == Some(1)))
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
    let following = ctx.binary("+", current, step)?;
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
