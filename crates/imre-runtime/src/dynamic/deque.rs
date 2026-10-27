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
    "extendleft",
    "remove",
    "pop",
    "popleft",
    "reverse",
    "__len__",
    "__iter__",
    "__reversed__",
    "__getitem__",
    "__setitem__",
    "__delitem__",
];

pub(super) enum IndexOp {
    Get,
    Set(Id),
    Delete,
}

impl IndexOp {
    /// Resolve the index before borrowing the ring so an index hook can mutate the deque.
    /// Explicit slot calls report overflow differently from bracket syntax.
    pub(super) fn apply(self, ctx: &mut Context, queue: Id, key: Id, explicit: bool) -> Result<Id> {
        let index = ctx.index_value(key)?.ok_or_else(|| {
            let kind = ctx.type_name(key);
            let message = if explicit {
                format!("'{kind}' object cannot be interpreted as an integer")
            } else {
                format!("sequence index must be integer, not '{kind}'")
            };
            Error::new("TypeError", message)
        })?;
        let index = crate::signed_index(&index).ok_or_else(|| {
            Error::new(
                if explicit {
                    "OverflowError"
                } else {
                    "IndexError"
                },
                format!(
                    "cannot fit '{}' into an index-sized integer",
                    ctx.type_name(key)
                ),
            )
        })?;
        let Value::Deque(ring) = ctx.heap.get_mut(queue) else {
            unreachable!()
        };
        let position = crate::normalized_index(index, ring.items.len())
            .ok_or_else(|| Error::new("IndexError", "deque index out of range"))?;
        let removed = match self {
            Self::Get => return Ok(ring.items[position]),
            Self::Set(value) => std::mem::replace(&mut ring.items[position], value),
            Self::Delete => ring.remove_at(position).expect("validated deque position"),
        };
        ctx.heap.weak_edge_removed(removed);
        Ok(NONE)
    }
}

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
        extend(ctx, queue, source, false)?;
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
fn extend(ctx: &mut Context, queue: Id, source: Id, left: bool) -> Result<()> {
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
        if let Some(removed) = ring.push(left, true, || item)? {
            ctx.heap.weak_edge_removed(removed);
        }
    }
    Ok(())
}

/// Compare through live cursors so a callback can mutate either deque or raise.
pub(super) fn compare(ctx: &mut Context, op: &str, left: Id, right: Id) -> Result<bool> {
    use std::collections::HashMap;

    if left == right {
        return Ok(op == "==");
    }

    enum Cursor {
        Deque(Id, Id),
        Sequence(usize),
    }
    struct Frame {
        left: Id,
        right: Id,
        cursor: Cursor,
        equal_op: bool,
        roots: usize,
        element_roots: usize,
        pending: Option<(Id, Id, bool)>,
        repeated: bool,
    }

    ctx.check_call_depth(0)?;
    ctx.call_depth += 1;
    let roots = ctx.heap.temporary_mark();
    let mut repeated_depth = 0;
    let result = stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
        let mut active: HashMap<(Id, Id), usize> = HashMap::new();
        let mut frames: Vec<Frame> = Vec::new();
        let mut request = Some((left, right, op == "=="));
        let mut answer: Option<bool> = None;
        loop {
            if let Some((a, b, equal_op)) = request.take() {
                if a == b {
                    answer = Some(equal_op);
                    continue;
                }
                let is_deque = matches!(
                    (ctx.heap.get(a), ctx.heap.get(b)),
                    (Value::Deque(_), Value::Deque(_))
                );
                let unequal = match (ctx.heap.get(a), ctx.heap.get(b)) {
                    (Value::Deque(x), Value::Deque(y)) => x.items.len() != y.items.len(),
                    (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
                        x.len() != y.len()
                    }
                    _ => unreachable!(),
                };
                if unequal {
                    answer = Some(!equal_op);
                    continue;
                }
                active
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                let repeats = active.get(&(a, b)).copied().unwrap_or(0);
                // Bound repeated pairs separately from acyclic nesting. A callback can
                // terminate a cycle, so do not reject its first repeated list pair.
                if repeats > 0 && ctx.deque_comparison_repeats >= 16_384 {
                    return Err(Error::new(
                        "RecursionError",
                        "maximum comparison depth exceeded",
                    ));
                }
                active.insert((a, b), repeats + 1);
                if repeats > 0 {
                    repeated_depth += 1;
                    ctx.deque_comparison_repeats += 1;
                }
                frames
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                let mark = ctx.heap.temporary_mark();
                ctx.heap.keep(a);
                ctx.heap.keep(b);
                let cursor = if is_deque {
                    let left_iter = ctx.iterator(a)?;
                    ctx.heap.keep(left_iter);
                    let right_iter = ctx.iterator(b)?;
                    ctx.heap.keep(right_iter);
                    Cursor::Deque(left_iter, right_iter)
                } else {
                    Cursor::Sequence(0)
                };
                frames.push(Frame {
                    left: a,
                    right: b,
                    cursor,
                    equal_op,
                    roots: mark,
                    element_roots: ctx.heap.temporary_mark(),
                    pending: None,
                    repeated: repeats > 0,
                });
                continue;
            }
            if let Some(value) = answer.take() {
                let Some(parent) = frames.last_mut() else {
                    return Ok(value);
                };
                let (x, y, final_pass) = parent.pending.take().expect("waiting parent");
                if !final_pass && !value {
                    if matches!(parent.cursor, Cursor::Sequence(_)) {
                        let value = !parent.equal_op;
                        let parent = frames.pop().unwrap();
                        if parent.repeated {
                            repeated_depth -= 1;
                            ctx.deque_comparison_repeats -= 1;
                        }
                        let count = active.get_mut(&(parent.left, parent.right)).unwrap();
                        *count -= 1;
                        if *count == 0 {
                            active.remove(&(parent.left, parent.right));
                        }
                        ctx.heap.release_temporaries_since(parent.roots);
                        answer = Some(value);
                    } else {
                        parent.pending = Some((x, y, true));
                        request = Some((x, y, parent.equal_op));
                    }
                } else if !final_pass {
                    ctx.heap.release_temporaries_since(parent.element_roots);
                } else {
                    let parent = frames.pop().unwrap();
                    if parent.repeated {
                        repeated_depth -= 1;
                        ctx.deque_comparison_repeats -= 1;
                    }
                    let count = active.get_mut(&(parent.left, parent.right)).unwrap();
                    *count -= 1;
                    if *count == 0 {
                        active.remove(&(parent.left, parent.right));
                    }
                    ctx.heap.release_temporaries_since(parent.roots);
                    answer = Some(value);
                }
                continue;
            }
            let frame = frames.last_mut().expect("active comparison");
            ctx.heap.release_temporaries_since(frame.element_roots);
            let (x, y) = match &mut frame.cursor {
                Cursor::Deque(left_iter, right_iter) => {
                    (ctx.next_item(*left_iter)?, ctx.next_item(*right_iter)?)
                }
                Cursor::Sequence(index) => {
                    let (a, b) = match (ctx.heap.get(frame.left), ctx.heap.get(frame.right)) {
                        (Value::List(a), Value::List(b)) | (Value::Tuple(a), Value::Tuple(b)) => {
                            (a, b)
                        }
                        _ => unreachable!(),
                    };
                    if a.len() != b.len() {
                        let value = !frame.equal_op;
                        let frame = frames.pop().unwrap();
                        if frame.repeated {
                            repeated_depth -= 1;
                            ctx.deque_comparison_repeats -= 1;
                        }
                        let count = active.get_mut(&(frame.left, frame.right)).unwrap();
                        *count -= 1;
                        if *count == 0 {
                            active.remove(&(frame.left, frame.right));
                        }
                        ctx.heap.release_temporaries_since(frame.roots);
                        answer = Some(value);
                        continue;
                    }
                    let result = (a.get(*index).copied(), b.get(*index).copied());
                    *index += 1;
                    result
                }
            };
            let (Some(x), Some(y)) = (x, y) else {
                let value = if frame.equal_op { x == y } else { x != y };
                let frame = frames.pop().unwrap();
                if frame.repeated {
                    repeated_depth -= 1;
                    ctx.deque_comparison_repeats -= 1;
                }
                let count = active.get_mut(&(frame.left, frame.right)).unwrap();
                *count -= 1;
                if *count == 0 {
                    active.remove(&(frame.left, frame.right));
                }
                ctx.heap.release_temporaries_since(frame.roots);
                answer = Some(value);
                continue;
            };
            ctx.heap.keep(x);
            ctx.heap.keep(y);
            if matches!(
                (ctx.heap.get(x), ctx.heap.get(y)),
                (Value::Deque(_), Value::Deque(_))
                    | (Value::List(_), Value::List(_))
                    | (Value::Tuple(_), Value::Tuple(_))
            ) {
                frame.pending = Some((x, y, false));
                request = Some((x, y, true));
                continue;
            }
            if !ctx.checked_equal(x, y)? {
                let value = if matches!(frame.cursor, Cursor::Sequence(_)) {
                    !frame.equal_op
                } else {
                    let operation = if frame.equal_op { "==" } else { "!=" };
                    let value = ctx.binary(operation, x, y)?;
                    ctx.truth(value)?
                };
                let frame = frames.pop().unwrap();
                if frame.repeated {
                    repeated_depth -= 1;
                    ctx.deque_comparison_repeats -= 1;
                }
                let count = active.get_mut(&(frame.left, frame.right)).unwrap();
                *count -= 1;
                if *count == 0 {
                    active.remove(&(frame.left, frame.right));
                }
                ctx.heap.release_temporaries_since(frame.roots);
                answer = Some(value);
            }
        }
    });
    ctx.heap.release_temporaries_since(roots);
    ctx.deque_comparison_repeats -= repeated_depth;
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
            let removed = ring.remove_at(index).unwrap();
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
    match (name, rest) {
        ("__getitem__", [index]) => return IndexOp::Get.apply(ctx, receiver, *index, true),
        ("__setitem__", [index, value]) => {
            return IndexOp::Set(*value).apply(ctx, receiver, *index, true);
        }
        ("__delitem__", [index]) => return IndexOp::Delete.apply(ctx, receiver, *index, true),
        ("__getitem__" | "__delitem__", _) => {
            return Err(Error::new(
                "TypeError",
                format!("expected 1 argument, got {}", rest.len()),
            ));
        }
        ("__setitem__", _) => {
            return Err(Error::new(
                "TypeError",
                format!("__setitem__ expected 2 arguments, got {}", rest.len()),
            ));
        }
        _ => {}
    }
    if matches!(
        name,
        "append" | "appendleft" | "extend" | "extendleft" | "remove"
    ) {
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
            "extend" | "extendleft" => extend(ctx, receiver, *item, name == "extendleft")?,
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
