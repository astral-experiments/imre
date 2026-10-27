//! Boxed collection operations over the same ring used by typed deques.
use super::{Context, Error, Id, NONE, Result, Value, builtins};

pub(super) const CLASS: &str = "collections.deque";
pub(super) const ITERATOR: &str = "collections._deque_iterator";
pub(super) const REVERSE_ITERATOR: &str = "collections._deque_reverse_iterator";
pub(super) const METHODS: &[&str] = &[
    "__init__",
    "append",
    "appendleft",
    "clear",
    "copy",
    "__copy__",
    "extend",
    "remove",
    "pop",
    "popleft",
    "reverse",
    "__len__",
    "__iter__",
    "__reversed__",
];

/// Capture the current length and generation for a live cursor in either direction.
pub(super) fn iterator(deque: Id, ring: &crate::objects::DequeRing<Id>, reverse: bool) -> Value {
    Value::DequeIterator {
        deque,
        state: ring.state,
        remaining: ring.items.len(),
        reverse,
    }
}

pub(super) fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let queue = ctx.alloc(Value::Deque(Box::new(crate::objects::DequeRing::new(
        (),
        None,
    ))));
    initialize(ctx, queue, args, kwargs)?;
    Ok(queue)
}

/// Validate the new bound before clearing the deque, then consume the source in order.
fn initialize(ctx: &mut Context, queue: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<()> {
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
    let Value::Deque(ring) = ctx.heap.get_mut(queue) else {
        unreachable!()
    };
    ring.maxlen = maxlen;
    clear(ctx, queue);
    if let Some(source) = iterable {
        extend(ctx, queue, source)?;
    }
    Ok(())
}

/// Remove the ring's traced edges after detaching all its items.
fn clear(ctx: &mut Context, queue: Id) {
    let Value::Deque(ring) = ctx.heap.get_mut(queue) else {
        unreachable!()
    };
    for item in ring.take_items() {
        ctx.heap.weak_edge_removed(item);
    }
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

/// Compare through live cursors so a callback can mutate either deque or raise.
pub(super) fn compare(ctx: &mut Context, op: &str, left: Id, right: Id) -> Result<bool> {
    if left == right {
        return Ok(op == "==");
    }
    let (Value::Deque(a), Value::Deque(b)) = (ctx.heap.get(left), ctx.heap.get(right)) else {
        unreachable!()
    };
    if a.items.len() != b.items.len() {
        return Ok(op == "!=");
    }
    ctx.check_call_depth(0)?;
    ctx.call_depth += 1;
    let roots = ctx.heap.temporary_mark();
    let result = stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
        let a = ctx.iterator(left)?;
        ctx.heap.keep(a);
        let b = ctx.iterator(right)?;
        ctx.heap.keep(b);
        let elements = ctx.heap.temporary_mark();
        loop {
            ctx.heap.release_temporaries_since(elements);
            let x = ctx.next_item(a)?;
            let y = ctx.next_item(b)?;
            let (Some(x), Some(y)) = (x, y) else {
                return Ok(if op == "==" { x == y } else { x != y });
            };
            ctx.heap.keep(x);
            ctx.heap.keep(y);
            if !ctx.checked_equal(x, y)? {
                // The first differing pair answers the originally requested operation.
                let result = ctx.binary(op, x, y)?;
                return ctx.truth(result);
            }
        }
    });
    ctx.heap.release_temporaries_since(roots);
    ctx.call_depth -= 1;
    result
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
            if name == "__init__" {
                "descriptor '__init__' of 'collections.deque' object needs an argument".to_owned()
            } else {
                format!("unbound method deque.{name}() needs an argument")
            },
        ));
    };
    let Value::Deque(ring) = ctx.heap.get_mut(receiver) else {
        let actual = ctx.type_name(receiver);
        return Err(Error::new(
            "TypeError",
            if name == "__init__" {
                format!(
                    "descriptor '__init__' requires a 'collections.deque' object but received a '{actual}'"
                )
            } else {
                format!(
                    "descriptor '{name}' for 'collections.deque' objects doesn't apply to a '{actual}' object"
                )
            },
        ));
    };
    if name == "__init__" {
        initialize(ctx, receiver, rest, kwargs)?;
        return Ok(NONE);
    }
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
        "clear" => {
            clear(ctx, receiver);
            Ok(NONE)
        }
        "copy" | "__copy__" => {
            let copy = ring.shallow_copy()?;
            Ok(ctx.alloc(Value::Deque(Box::new(copy))))
        }
        "reverse" => {
            ring.reverse();
            Ok(NONE)
        }
        "__reversed__" => {
            let iter = iterator(receiver, ring, true);
            Ok(ctx.alloc(iter))
        }
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
