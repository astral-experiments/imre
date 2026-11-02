//! Boxed collection operations over the same ring used by typed deques.
use super::{Context, Error, Id, NONE, Result, Value, builtins};

pub(super) const CLASS: &str = "collections.deque";
pub(super) const ITERATOR: &str = "collections._deque_iterator";
pub(super) const REVERSE_ITERATOR: &str = "collections._deque_reverse_iterator";
pub(super) const ITERATOR_METHODS: &[&str] =
    &["__iter__", "__next__", "__length_hint__", "__reduce__"];

pub(super) fn iterator_class(name: &str) -> Option<&'static str> {
    match name {
        ITERATOR => Some(ITERATOR),
        REVERSE_ITERATOR => Some(REVERSE_ITERATOR),
        _ => None,
    }
}

pub(super) struct Member {
    pub(super) name: &'static str,
    pub(super) doc: &'static str,
    pub(super) signature: &'static str,
}

impl Member {
    const fn new(name: &'static str, doc: &'static str, signature: &'static str) -> Self {
        Self {
            name,
            doc,
            signature,
        }
    }

    pub(super) fn is_method(&self) -> bool {
        !self.name.starts_with("__") || matches!(self.name, "__copy__" | "__reduce__" | "__reversed__")
    }
}

// The implemented members in CPython 3.14.5 type dictionary order, with their clinic metadata.
const MEMBERS: &[Member] = &[
    Member::new("__repr__", "Return repr(self).", "($self, /)"),
    Member::new("__eq__", "Return self==value.", "($self, value, /)"),
    Member::new("__ne__", "Return self!=value.", "($self, value, /)"),
    Member::new("__iter__", "Implement iter(self).", "($self, /)"),
    Member::new(
        "__init__",
        "Initialize self.  See help(type(self)) for accurate signature.",
        "($self, /, *args, **kwargs)",
    ),
    Member::new("__len__", "Return len(self).", "($self, /)"),
    Member::new("__mul__", "Return self*value.", "($self, value, /)"),
    Member::new("__rmul__", "Return value*self.", "($self, value, /)"),
    Member::new("__getitem__", "Return self[key].", "($self, key, /)"),
    Member::new(
        "__setitem__",
        "Set self[key] to value.",
        "($self, key, value, /)",
    ),
    Member::new("__delitem__", "Delete self[key].", "($self, key, /)"),
    Member::new(
        "__contains__",
        "Return bool(key in self).",
        "($self, key, /)",
    ),
    Member::new("__iadd__", "Implement self+=value.", "($self, value, /)"),
    Member::new("__imul__", "Implement self*=value.", "($self, value, /)"),
    Member::new(
        "append",
        "Add an element to the right side of the deque.",
        "($self, item, /)",
    ),
    Member::new(
        "appendleft",
        "Add an element to the left side of the deque.",
        "($self, item, /)",
    ),
    Member::new("clear", "Remove all elements from the deque.", "($self, /)"),
    Member::new(
        "__copy__",
        "Return a shallow copy of a deque.",
        "($self, /)",
    ),
    Member::new("copy", "Return a shallow copy of a deque.", "($self, /)"),
    Member::new(
        "count",
        "Return number of occurrences of value.",
        "($self, value, /)",
    ),
    Member::new(
        "extend",
        "Extend the right side of the deque with elements from the iterable.",
        "($self, iterable, /)",
    ),
    Member::new(
        "extendleft",
        "Extend the left side of the deque with elements from the iterable.",
        "($self, iterable, /)",
    ),
    Member::new(
        "index",
        "Return first index of value.\n\nRaises ValueError if the value is not present.",
        "($self, value, [start, [stop]])",
    ),
    Member::new(
        "pop",
        "Remove and return the rightmost element.",
        "($self, /)",
    ),
    Member::new(
        "popleft",
        "Remove and return the leftmost element.",
        "($self, /)",
    ),
    Member::new(
        "__reduce__",
        "Return state information for pickling.",
        "($self, /)",
    ),
    Member::new(
        "remove",
        "Remove first occurrence of value.",
        "($self, value, /)",
    ),
    Member::new(
        "__reversed__",
        "Return a reverse iterator over the deque.",
        "($self, /)",
    ),
    Member::new("reverse", "Reverse *IN PLACE*.", "($self, /)"),
];

pub(super) fn member(name: &str) -> Option<&'static Member> {
    MEMBERS.iter().find(|entry| entry.name == name)
}

pub(super) fn owns(name: &str) -> bool {
    member(name).is_some() || matches!(name, "__new__" | "__hash__" | "maxlen")
}

/// Expose the immutable native type's own members through the same objects as class lookup.
pub(super) fn class_dictionary(ctx: &mut Context, class: Id) -> Result<Id> {
    ctx.native_class_dictionary(
        class,
        "<collections.deque namespace>",
        MEMBERS
            .iter()
            .map(|entry| entry.name)
            .chain(["__new__", "__hash__", "maxlen"]),
    )
}

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
pub(super) fn iterator(
    deque: Id,
    ring: &crate::objects::DequeRing<Id>,
    reverse: bool,
    consumed: usize,
) -> Value {
    Value::DequeIterator {
        deque,
        state: ring.state,
        remaining: ring.items.len().saturating_sub(consumed),
        reverse,
    }
}

/// The iterator factories parse positional arguments only and skip within the current ring.
pub(super) fn construct_iterator(ctx: &mut Context, class: &str, args: &[Id]) -> Result<Id> {
    let ([owner] | [owner, _]) = args else {
        return Err(Error::new("TypeError", "expected a deque and optional index"));
    };
    let storage = ctx.scalar_value(*owner);
    if !matches!(ctx.heap.get(storage), Value::Deque(_)) {
        return Err(Error::new(
            "TypeError",
            format!("argument 1 must be collections.deque, not {}", ctx.type_name(*owner)),
        ));
    }
    let consumed = match args.get(1) {
        Some(&index) => {
            let index = crate::signed_index(&ctx.index_integer(index)?).ok_or_else(|| {
                Error::new("OverflowError", "Python int too large to convert to C ssize_t")
            })?;
            index.max(0) as usize
        }
        None => 0,
    };
    // The index conversion may have mutated the deque.
    let Value::Deque(ring) = ctx.heap.get(storage) else {
        unreachable!()
    };
    let cursor = iterator(*owner, ring, class == REVERSE_ITERATOR, consumed);
    Ok(ctx.alloc(cursor))
}

pub(super) fn iterator_method(
    ctx: &mut Context,
    class: &str,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method {class}.{name}() needs an argument"),
        ));
    };
    if ctx.heap.get(receiver).intrinsic_type_name() != Some(class) {
        return Err(Error::new("TypeError", format!("expected a {class} receiver")));
    }
    if !rest.is_empty() || !kwargs.is_empty() {
        return Err(Error::new("TypeError", format!("{class}.{name}() takes no arguments")));
    }
    let Value::DequeIterator { deque, remaining, .. } = *ctx.heap.get(receiver) else {
        unreachable!()
    };
    match name {
        "__iter__" => Ok(receiver),
        "__next__" => ctx.next_item(receiver)?.ok_or_else(|| Error::new("StopIteration", "")),
        "__length_hint__" => Ok(ctx.int(remaining as i64)),
        "__reduce__" => {
            let Value::Deque(ring) = ctx.heap.get(ctx.scalar_value(deque)) else {
                unreachable!()
            };
            // A mutation can shrink the deque before the cursor notices.
            let consumed = ctx.int(ring.items.len() as i64 - remaining as i64);
            let args = ctx.alloc(Value::Tuple(vec![deque, consumed]));
            let factory = ctx.builtin(class);
            Ok(ctx.alloc(Value::Tuple(vec![factory, args])))
        }
        _ => unreachable!("registered deque iterator method"),
    }
}

/// Publish the allocator, data descriptor and inherited native methods for deque subtypes.
pub(super) fn install(ctx: &mut Context) {
    for member in MEMBERS {
        ctx.builtin(&format!("{CLASS}.{}", member.name));
    }
    ctx.builtin("collections.deque.__new__");
    ctx.builtins
        .insert("collections.deque.__hash__".into(), NONE);
    let get = ctx.builtin("collections.deque._get_maxlen");
    let descriptor = ctx.alloc(Value::GetSetDescriptor {
        owner: CLASS,
        name: "maxlen",
        get,
        set: None,
    });
    ctx.builtins
        .insert("collections.deque.maxlen".into(), descriptor);
}

fn empty(ctx: &mut Context) -> Id {
    ctx.alloc(Value::Deque(Box::new(crate::objects::DequeRing::new(
        (),
        None,
    ))))
}

pub(super) fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let queue = empty(ctx);
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
    let storage = ctx.scalar_value(queue);
    let Value::Deque(ring) = ctx.heap.get_mut(storage) else {
        unreachable!()
    };
    ring.maxlen = maxlen;
    builtins::clear_container(ctx, storage);
    if let Some(source) = iterable {
        extend(ctx, queue, source, false)?;
    }
    Ok(())
}

pub(super) fn copy(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let storage = ctx.scalar_value(receiver);
    let Value::Deque(ring) = ctx.heap.get(storage) else {
        unreachable!()
    };
    if storage == receiver {
        let copy = ring.shallow_copy()?;
        return Ok(ctx.alloc(Value::Deque(Box::new(copy))));
    }
    let bound = ring.maxlen;
    let class = ctx.exact_type(receiver)?;
    let mut args = vec![receiver];
    if let Some(n) = bound {
        args.push(ctx.int(n as i64));
    }
    let result = ctx.call(class, args, Vec::new())?;
    if !matches!(ctx.heap.get(ctx.scalar_value(result)), Value::Deque(_)) {
        return Err(Error::new(
            "TypeError",
            format!(
                "{}() must return a deque, not {}",
                ctx.type_name(receiver),
                ctx.type_name(result)
            ),
        ));
    }
    Ok(result)
}

/// A subtype's iterator can mutate its ring while the repetition takes its snapshot.
pub(super) fn repeat(ctx: &mut Context, receiver: Id, count: isize, inplace: bool) -> Result<Id> {
    let owner = if inplace {
        receiver
    } else {
        copy(ctx, receiver)?
    };
    let storage = ctx.scalar_value(owner);
    let Value::Deque(ring) = ctx.heap.get_mut(storage) else {
        unreachable!()
    };
    let size = ring.items.len();
    if size == 0 || count == 1 {
        return Ok(owner);
    }
    if count <= 0 {
        builtins::clear_container(ctx, storage);
        return Ok(owner);
    }
    let count = count as usize;
    let memory = || Error::new("MemoryError", "");
    if size == 1 {
        let length = count.min(ring.maxlen.unwrap_or(count));
        ring.items.try_reserve(length - 1).map_err(|_| memory())?;
        ring.items.resize(length, ring.items[0]);
        ring.state = ring.state.wrapping_add(1);
        return Ok(owner);
    }
    if count > isize::MAX as usize / size {
        return Err(memory());
    }
    if owner == storage {
        // All original references survive. Rotate to the first retained element,
        // then fill from that period without building a second repeated sequence.
        let total = count * size;
        let length = total.min(ring.maxlen.unwrap_or(total));
        if length > size {
            ring.items
                .try_reserve(length - size)
                .map_err(|_| memory())?;
            ring.items.rotate_left((total - length) % size);
            for i in size..length {
                ring.items.push_back(ring.items[i % size]);
            }
            ring.state = ring.state.wrapping_add(1);
        }
        return Ok(owner);
    }

    // PySequence_List observes __iter__ followed by __len__, even if the bound is full.
    let snapshot = ctx.alloc(Value::List(Vec::new()));
    builtins::list_extend(ctx, snapshot, owner)?;
    let Value::List(items) = ctx.heap.get(snapshot) else {
        unreachable!()
    };
    let chunk = items.len();
    let Value::Deque(ring) = ctx.heap.get_mut(storage) else {
        unreachable!()
    };
    let times = ring
        .maxlen
        .map_or(count, |bound| count.min(bound.div_ceil(size)))
        .saturating_sub(1);
    if times != 0 && chunk != 0 {
        let extra = chunk.saturating_mul(times).min(
            ring.maxlen
                .unwrap_or(usize::MAX)
                .saturating_sub(ring.items.len()),
        );
        ring.items.try_reserve(extra).map_err(|_| memory())?;
        let roots = ctx.heap.temporary_mark();
        for _ in 0..times {
            let result = extend(ctx, owner, snapshot, false);
            ctx.heap.release_temporaries_since(roots);
            result?;
        }
    }
    Ok(owner)
}

/// Consume a live Python iterator one item at a time, retaining any successful prefix.
pub(super) fn extend(ctx: &mut Context, queue: Id, source: Id, left: bool) -> Result<()> {
    let storage = ctx.scalar_value(queue);
    let source = if queue == source {
        let items = ctx.items(source)?;
        ctx.alloc(Value::Tuple(items))
    } else {
        source
    };
    let iter = ctx.iterator(source)?;
    let roots = ctx.heap.temporary_mark();
    ctx.heap.keep(queue);
    ctx.heap.keep(iter);
    let step = ctx.heap.temporary_mark();
    let result = (|| {
        loop {
            ctx.heap.release_temporaries_since(step);
            ctx.heap.reserve_slot_or_request_collection();
            ctx.collect_garbage();
            let Some(item) = ctx.next_item(iter)? else {
                break;
            };
            let Value::Deque(ring) = ctx.heap.get_mut(storage) else {
                unreachable!()
            };
            if let Some(removed) = ring.push(left, true, || item)? {
                ctx.heap.weak_edge_removed(removed);
            }
        }
        Ok(())
    })();
    ctx.heap.release_temporaries_since(roots);
    result
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
                let containers = (
                    ctx.heap.get(ctx.scalar_value(a)),
                    ctx.heap.get(ctx.scalar_value(b)),
                );
                let is_deque = matches!(containers, (Value::Deque(_), Value::Deque(_)));
                let unequal = match containers {
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

/// Keep the element alive while its equality and truth hooks run.
fn element_matches(ctx: &mut Context, item: Id, value: Id) -> Result<bool> {
    if item == value {
        return Ok(true);
    }
    let mark = ctx.heap.temporary_mark();
    ctx.heap.keep(item);
    let equal = ctx
        .binary("==", item, value)
        .and_then(|result| ctx.truth(result));
    ctx.heap.release_temporaries_since(mark);
    equal
}

/// Count live elements, checking the generation even after the final match.
fn count(ctx: &mut Context, storage: Id, value: Id) -> Result<Id> {
    let Value::Deque(ring) = ctx.heap.get(storage) else {
        unreachable!()
    };
    let (state, mut remaining) = (ring.state, ring.items.len());
    let mut matches = 0;
    loop {
        let Value::Deque(ring) = ctx.heap.get(storage) else {
            unreachable!()
        };
        let Some(&item) = ring.cursor_item(state, &mut remaining, false)? else {
            return Ok(ctx.int(matches));
        };
        matches += i64::from(element_matches(ctx, item, value)?);
    }
}

/// Convert index bounds before capturing deque state; both conversions may run Python code.
fn index(ctx: &mut Context, queue: Id, args: &[Id]) -> Result<Id> {
    let Some((&value, bounds)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "index expected at least 1 argument, got 0",
        ));
    };
    if bounds.len() > 2 {
        return Err(Error::new(
            "TypeError",
            format!("index expected at most 3 arguments, got {}", args.len()),
        ));
    }
    let mut limits = [None, None];
    for (limit, &bound) in limits.iter_mut().zip(bounds) {
        *limit = Some(ctx.index_value(bound)?.ok_or_else(|| {
            Error::new(
                "TypeError",
                "slice indices must be integers or have an __index__ method",
            )
        })?);
    }
    let Value::Deque(ring) = ctx.heap.get(queue) else {
        unreachable!()
    };
    let state = ring.state;
    for i in ring.search_range(limits[0].as_ref(), limits[1].as_ref()) {
        let Value::Deque(ring) = ctx.heap.get(queue) else {
            unreachable!()
        };
        let item = ring.items[i];
        if element_matches(ctx, item, value)? {
            return Ok(ctx.int(i as i64));
        }
        let Value::Deque(ring) = ctx.heap.get(queue) else {
            unreachable!()
        };
        if ring.state != state {
            return Err(Error::new("RuntimeError", "deque mutated during iteration"));
        }
    }
    Err(Error::new("ValueError", "deque.index(x): x not in deque"))
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
        let equal = element_matches(ctx, item, value)?;
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

/// Let pickle memoize the empty deque before consuming its items, including self-references.
fn reduce(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let getstate = ctx.lookup_attr(receiver, "__getstate__")?;
    let state = ctx.call(getstate, Vec::new(), Vec::new())?;
    ctx.heap.keep(state);
    let items = ctx.iterator(receiver)?;
    ctx.heap.keep(items);
    let Value::Deque(ring) = ctx.heap.get(ctx.scalar_value(receiver)) else {
        unreachable!("validated deque receiver")
    };
    let args = if let Some(bound) = ring.maxlen {
        let empty = ctx.alloc(Value::Tuple(Vec::new()));
        let bound = ctx.int(bound as i64);
        ctx.alloc(Value::Tuple(vec![empty, bound]))
    } else {
        ctx.alloc(Value::Tuple(Vec::new()))
    };
    let class = ctx.exact_type(receiver)?;
    Ok(ctx.alloc(Value::Tuple(vec![class, args, state, items])))
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
            if member(name).is_some_and(|entry| !entry.is_method()) {
                format!("descriptor '{name}' of 'collections.deque' object needs an argument")
            } else {
                format!("unbound method deque.{name}() needs an argument")
            },
        ));
    };
    if name == "__new__" {
        let base = ctx.builtin(CLASS);
        if !ctx.direct_subclass(receiver, base) {
            return Err(ctx.invalid_new_subtype("collections.deque", receiver));
        }
        let storage = empty(ctx);
        if receiver == base {
            return Ok(storage);
        }
        let object = ctx.instance(receiver, None);
        let Value::Instance { scalar, .. } = ctx.heap.get_mut(object) else {
            unreachable!()
        };
        *scalar = Some(storage);
        return Ok(object);
    }
    let storage = ctx.scalar_value(receiver);
    let Value::Deque(ring) = ctx.heap.get_mut(storage) else {
        let actual = ctx.type_name(receiver);
        return Err(Error::new(
            "TypeError",
            if member(name).is_some_and(|entry| !entry.is_method()) {
                format!(
                    "descriptor '{name}' requires a 'collections.deque' object but received a '{actual}'"
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
            if matches!(
                name,
                "__iadd__" | "__contains__" | "__repr__" | "__mul__" | "__rmul__" | "__imul__"
            ) {
                format!("wrapper {name}() takes no keyword arguments")
            } else {
                format!("deque.{name}() takes no keyword arguments")
            },
        ));
    }
    if name == "__repr__" {
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("expected 0 arguments, got {}", rest.len()),
            ));
        }
        if receiver != storage {
            if ctx.repr_ancestors.contains(&receiver) {
                return Ok(ctx.text("[...]"));
            }
            ctx.repr_ancestors
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            ctx.repr_ancestors.push(receiver);
            let rendered = (|| {
                let items = builtins::call(ctx, "list", &[receiver], &[])?;
                let Value::Deque(ring) = ctx.heap.get(storage) else {
                    unreachable!()
                };
                let maxlen = ring.maxlen;
                let body = ctx.checked_format(items, true)?;
                let name = ctx.type_name(receiver);
                Ok(ctx.text(if let Some(n) = maxlen {
                    format!("{name}({body}, maxlen={n})")
                } else {
                    format!("{name}({body})")
                }))
            })();
            ctx.repr_ancestors.pop();
            return rendered;
        }
        let text = ctx.checked_format(receiver, true)?;
        return Ok(ctx.text(text));
    }
    if name == "index" {
        return index(ctx, storage, rest);
    }
    match (name, rest) {
        ("__eq__" | "__ne__", [other]) => {
            if !matches!(ctx.heap.get(ctx.scalar_value(*other)), Value::Deque(_)) {
                return Ok(super::value::NOT_IMPLEMENTED);
            }
            let op = if name == "__eq__" { "==" } else { "!=" };
            return compare(ctx, op, receiver, *other).map(builtins::boolean);
        }
        ("__mul__" | "__rmul__" | "__imul__", [item]) => {
            let (_, n) = ctx.sequence_multiplier(*item, true)?;
            return repeat(ctx, receiver, n, name == "__imul__");
        }
        ("__mul__" | "__rmul__" | "__imul__", _) => {
            return Err(Error::new(
                "TypeError",
                format!("expected 1 argument, got {}", rest.len()),
            ));
        }
        ("__contains__", [item]) => return ctx.contains(*item, storage).map(builtins::boolean),
        ("__getitem__", [index]) => return IndexOp::Get.apply(ctx, storage, *index, true),
        ("__setitem__", [index, value]) => {
            return IndexOp::Set(*value).apply(ctx, storage, *index, true);
        }
        ("__delitem__", [index]) => return IndexOp::Delete.apply(ctx, storage, *index, true),
        ("__iadd__", [item]) => {
            extend(ctx, receiver, *item, false)?;
            return Ok(receiver);
        }
        ("__getitem__" | "__delitem__" | "__iadd__" | "__contains__" | "__eq__" | "__ne__", _) => {
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
        "append" | "appendleft" | "count" | "extend" | "extendleft" | "remove"
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
            "count" => return count(ctx, storage, *item),
            "extend" | "extendleft" => extend(ctx, receiver, *item, name == "extendleft")?,
            "remove" => remove(ctx, storage, *item)?,
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
        "__reduce__" => reduce(ctx, receiver),
        "clear" => {
            builtins::clear_container(ctx, storage);
            Ok(NONE)
        }
        "_get_maxlen" => {
            let bound = ring.maxlen;
            Ok(bound.map_or(NONE, |n| ctx.int(n as i64)))
        }
        "copy" | "__copy__" => copy(ctx, receiver),
        "reverse" => {
            ring.reverse();
            Ok(NONE)
        }
        "__iter__" | "__reversed__" => {
            let iter = iterator(receiver, ring, name == "__reversed__", 0);
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
        _ => unreachable!("registered deque method"),
    }
}
