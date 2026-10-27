//! Boxed collection operations over the same ring used by typed deques.
use super::{Context, Error, Id, NONE, Result, Value, builtins};

pub(super) const CLASS: &str = "collections.deque";
pub(super) const ITERATOR: &str = "collections._deque_iterator";
pub(super) const METHODS: &[&str] = &[
    "append",
    "appendleft",
    "extend",
    "remove",
    "pop",
    "popleft",
    "__len__",
    "__iter__",
];

pub(super) fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [iterable, maxlen] =
        builtins::bind_method_arguments("deque", args, kwargs, ["iterable", "maxlen"], 2)?;
    let maxlen = maxlen
        .filter(|&id| id != NONE)
        .map(|id| {
            let value = ctx
                .as_int(id)
                .map_err(|_| Error::new("TypeError", "an integer is required"))?;
            let bound = value
                .to_i64()
                .and_then(|n| isize::try_from(n).ok())
                .ok_or_else(|| {
                    Error::new(
                        "OverflowError",
                        "Python int too large to convert to C ssize_t",
                    )
                })?;
            usize::try_from(bound)
                .map_err(|_| Error::new("ValueError", "maxlen must be non-negative"))
        })
        .transpose()?;
    let queue = ctx.alloc(Value::Deque(Box::new(crate::objects::DequeRing::new(
        (),
        maxlen,
    ))));
    if let Some(source) = iterable {
        extend(ctx, queue, source)?;
    }
    Ok(queue)
}

/// Consume a live Python iterator one item at a time, retaining any successful prefix.
fn extend(ctx: &mut Context, queue: Id, source: Id) -> Result<()> {
    let source = if queue == source {
        let items = ctx.items(source)?;
        ctx.alloc(Value::Tuple(items))
    } else {
        source
    };
    let iter = ctx.iterator(source)?;
    while let Some(item) = ctx.next_item(iter)? {
        let Value::Deque(ring) = ctx.heap.get_mut(queue) else {
            unreachable!()
        };
        if let Some(removed) = ring.push(false, true, || item)? {
            ctx.heap.weak_edge_removed(removed);
        }
    }
    Ok(())
}

/// A Python equality callback may mutate the queue or raise before a match is removed.
fn remove(ctx: &mut Context, queue: Id, value: Id) -> Result<()> {
    let Value::Deque(ring) = ctx.heap.get(queue) else {
        unreachable!()
    };
    let (state, len) = (ring.state, ring.items.len());
    for index in 0..len {
        let Value::Deque(ring) = ctx.heap.get(queue) else {
            unreachable!()
        };
        let item = ring.items[index];
        let equal = item == value || {
            let mark = ctx.heap.temporary_mark();
            ctx.heap.keep(item);
            let equal = ctx
                .binary("==", item, value)
                .and_then(|result| ctx.truth(result));
            ctx.heap.release_temporaries_since(mark);
            equal?
        };
        let Value::Deque(ring) = ctx.heap.get_mut(queue) else {
            unreachable!()
        };
        if ring.state != state {
            return Err(Error::new("IndexError", "deque mutated during iteration"));
        }
        if equal {
            let removed = ring.items.remove(index).unwrap();
            ring.state = ring.state.wrapping_add(1);
            ctx.heap.weak_edge_removed(removed);
            return Ok(());
        }
    }
    Err(Error::new("ValueError", "deque.remove(x): x not in deque"))
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method deque.{name}() needs an argument"),
        ));
    };
    let Value::Deque(ring) = ctx.heap.get_mut(receiver) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for 'collections.deque' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("deque.{name}() takes no keyword arguments"),
        ));
    }
    if matches!(name, "append" | "appendleft" | "extend" | "remove") {
        let [item] = rest else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "deque.{name}() takes exactly one argument ({} given)",
                    rest.len()
                ),
            ));
        };
        match name {
            "extend" => extend(ctx, receiver, *item)?,
            "remove" => remove(ctx, receiver, *item)?,
            _ => {
                if let Some(removed) = ring.push(name == "appendleft", false, || *item)? {
                    ctx.heap.weak_edge_removed(removed);
                }
            }
        }
        return Ok(NONE);
    }
    if !rest.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("deque.{name}() takes no arguments ({} given)", rest.len()),
        ));
    }
    match name {
        "pop" | "popleft" => {
            let item = ring.pop(name == "popleft")?;
            ctx.heap.weak_edge_removed(item);
            Ok(item)
        }
        "__len__" => {
            let length = ring.items.len();
            Ok(ctx.int(length as i64))
        }
        "__iter__" => ctx.iterator(receiver),
        _ => unreachable!("registered deque method"),
    }
}
