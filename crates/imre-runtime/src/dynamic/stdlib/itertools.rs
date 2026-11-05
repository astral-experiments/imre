use super::{Context, Error, Id, NONE, Result, Value};

/// Advance the current child, obtaining another from the outer iterator only on exhaustion.
pub(super) fn chain_next(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [base, receiver] = args else {
        return Err(Error::new("TypeError", "expected a chain instance"));
    };
    if !matches!(ctx.heap.get(*receiver), Value::Instance { class, .. } if ctx.direct_subclass(*class, *base))
    {
        return Err(Error::new("TypeError", "expected a chain instance"));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "chain.__next__() takes no keyword arguments",
        ));
    }
    let roots = ctx.heap.temporary_mark();
    ctx.heap.keep(*receiver);
    let step = ctx.heap.temporary_mark();
    let mut limit = ctx.heap.streaming_collection_limit();
    let result = (|| loop {
        let Value::Instance { fields, .. } = ctx.heap.get(*receiver) else {
            return Err(Error::new("TypeError", "expected a chain instance"));
        };
        let (Some(&source), Some(&active)) = (fields.get("_sources"), fields.get("_active")) else {
            return Err(Error::new("TypeError", "invalid chain state receiver"));
        };
        if source == NONE {
            return Err(Error::new("StopIteration", ""));
        }
        let active = if active == NONE {
            ctx.heap.keep(source);
            let child = ctx
                .next_item(source)
                .and_then(|item| item.ok_or_else(|| Error::new("StopIteration", "")))
                .inspect_err(|_| set_chain_field(ctx, *receiver, "_sources", NONE))?;
            ctx.heap.keep(child);
            let active = match ctx.iterator(child) {
                Ok(active) => active,
                Err(error) => {
                    set_chain_field(ctx, *receiver, "_active", NONE);
                    set_chain_field(ctx, *receiver, "_sources", NONE);
                    return Err(error);
                }
            };
            set_chain_field(ctx, *receiver, "_active", active);
            active
        } else {
            active
        };
        // A callback can advance this same chain and replace either cursor.
        ctx.heap.keep(active);
        if let Some(item) = ctx.next_item(active)? {
            return Ok(item);
        }
        set_chain_field(ctx, *receiver, "_active", NONE);
        ctx.heap.release_temporaries_since(step);
        ctx.collect_streaming_garbage(&mut limit);
    })();
    ctx.heap.release_temporaries_since(roots);
    result
}

fn set_chain_field(ctx: &mut Context, receiver: Id, key: &str, value: Id) {
    let Value::Instance { fields, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!("live chain instance")
    };
    if let Some(slot) = fields.get_mut(key) {
        *slot = value;
    } else {
        fields.insert(key.to_owned(), value);
    }
}

const COUNT: &str = "itertools.count";
const REPEAT: &str = "itertools.repeat";

pub(in crate::dynamic) fn cursor_type(name: &str) -> Option<&'static str> {
    match name {
        COUNT => Some(COUNT),
        REPEAT => Some(REPEAT),
        _ => None,
    }
}

/// These slots are owned by the iterator types, in CPython's type-dictionary order.
pub(in crate::dynamic) fn slots(owner: &str) -> impl Iterator<Item = &'static str> {
    ["__repr__", "__getattribute__", "__iter__", "__next__"]
        .into_iter()
        .chain((owner == REPEAT).then_some("__length_hint__"))
}

pub(in crate::dynamic) fn install_namespace(ctx: &mut Context, class: Id, owner: &str) {
    let doc = if owner == COUNT {
        "Return a count object whose .__next__() method returns consecutive values.\n\nEquivalent to:\n    def count(firstval=0, step=1):\n        x = firstval\n        while 1:\n            yield x\n            x += step"
    } else {
        "repeat(object [,times]) -> create an iterator which returns the object\nfor the specified number of times.  If not specified, returns the object\nendlessly."
    };
    let fields = std::iter::once("__new__")
        .chain(slots(owner))
        .chain(["__doc__", "__module__"]);
    let entries = fields
        .map(|field| {
            let value = match field {
                "__doc__" => ctx.text(doc),
                "__module__" => ctx.text("itertools"),
                _ => ctx.builtin(&format!("{owner}.{field}")),
            };
            (ctx.text(field), value)
        })
        .collect();
    let dictionary = ctx.dict(entries);
    ctx.native_type_dicts.insert(class, dictionary);
}

/// Allocate the native cursor even when the constructor is inherited by a Python class.
pub(in crate::dynamic) fn allocate(
    ctx: &mut Context,
    owner: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&class, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("{owner}.__new__(): not enough arguments"),
        ));
    };
    let base = ctx.builtin(owner);
    if !ctx.is_type_object(class) || !ctx.subclass(class, base) {
        return Err(ctx.invalid_new_subtype(owner, class));
    }
    let cursor = if owner == COUNT {
        count(ctx, args, kwargs)?
    } else {
        repeat(ctx, args, kwargs)?
    };
    if class == base {
        return Ok(cursor);
    }
    let instance = ctx.instance(class, None);
    if let Value::Instance { scalar, .. } = ctx.heap.get_mut(instance) {
        *scalar = Some(cursor);
    }
    Ok(instance)
}

/// The input's iterator is released on exhaustion. Only yielded objects enter the replay buffer.
#[derive(Clone)]
pub(in crate::dynamic) struct Cycle {
    source: Option<Id>,
    saved: Vec<Id>,
    index: usize,
}

/// Native state of the bundled iterator classes. Subclasses keep their
/// ordinary Python fields alongside the same traced cursor.
#[derive(Clone)]
pub(in crate::dynamic) enum Cursor {
    Cycle(Cycle),
    Accumulate(Accumulator),
}

#[derive(Clone)]
pub(in crate::dynamic) struct Accumulator {
    base: Id,
    source: Id,
    function: Id,
    total: Option<Id>,
    initial_pending: bool,
    addition: bool,
}

impl Cursor {
    pub(in crate::dynamic) fn children(&self, visit: &mut crate::dynamic::value::MarkQueue<'_>) {
        match self {
            Self::Cycle(state) => {
                visit.extend(state.source);
                visit.extend(&state.saved);
            }
            Self::Accumulate(state) => {
                visit.extend([state.base, state.source, state.function]);
                visit.extend(state.total);
            }
        }
    }

    pub(in crate::dynamic) fn is_exact_accumulate(&self, class: Id) -> bool {
        matches!(self, Self::Accumulate(state) if state.base == class)
    }
}

fn cursor(ctx: &mut Context, receiver: Id) -> Option<&mut Cursor> {
    match ctx.heap.get_mut(receiver) {
        Value::Instance { itertools, .. } => itertools.as_deref_mut(),
        _ => None,
    }
}

fn cursor_instance(ctx: &mut Context, class: Id, state: Cursor) -> Id {
    let result = ctx.instance(class, None);
    let Value::Instance { itertools, .. } = ctx.heap.get_mut(result) else {
        unreachable!("fresh iterator instance")
    };
    *itertools = Some(Box::new(state));
    result
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
    Ok(cursor_instance(
        ctx,
        *class,
        Cursor::Cycle(Cycle {
            source: Some(source),
            saved: Vec::new(),
            index: 0,
        }),
    ))
}

pub(super) fn cycle_next(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [receiver] = args else {
        return Err(Error::new("TypeError", "expected a cycle instance"));
    };
    let Some(Cursor::Cycle(state)) = cursor(ctx, *receiver) else {
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
    let Some(Cursor::Cycle(state)) = cursor(ctx, *receiver) else {
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

impl Context {
    /// Freeze the exported iterator classes defined by this bundled module generation.
    pub(in crate::dynamic) fn freeze_itertools_types(&mut self, module: Id) {
        let classes: Vec<_> = self
            .scope_bindings(module)
            .filter_map(|(export, value)| {
                let Value::Class { name, scope, .. } = self.heap.get(value) else {
                    return None;
                };
                let visible = !export.starts_with('_')
                    || matches!(export, "_grouper" | "_tee" | "_tee_dataobject");
                (visible && name == export && self.root_scope(*scope) == module).then_some(*scope)
            })
            .collect();
        for scope in classes {
            if let Value::Scope(frame) = self.heap.get_mut(scope)
                && let Some(namespace) = &mut frame.class_namespace
            {
                namespace.immutable_type = true;
            }
        }
    }

    pub(in crate::dynamic) fn itertools_layout(&self, class: Id) -> Option<&'static str> {
        let Value::Class { scope, .. } = *self.heap.get(class) else {
            return None;
        };
        self.scope_data(scope)
            .class_namespace
            .as_ref()?
            .itertools_layout
    }
}

pub(super) fn accumulate_allocate(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if let [base, class, source, function, initial] = args
        && kwargs.is_empty()
        && ctx.itertools_layout(*base) == Some("itertools.accumulate")
        && matches!(ctx.heap.get(*class), Value::Class { ancestors, .. }
            if ancestors.iter().all(|&a| !matches!(ctx.heap.get(a), Value::Native(name) if name != "object")))
        && ctx.allocation_base(*class) == Some(*base)
    {
        return Ok(cursor_instance(
            ctx,
            *class,
            Cursor::Accumulate(Accumulator {
                base: *base,
                source: *source,
                function: *function,
                addition: *function == NONE
                    || matches!(ctx.heap.get(*function), Value::Native(name) if name == "operator.add"),
                total: (*initial != NONE).then_some(*initial),
                initial_pending: *initial != NONE,
            }),
        ));
    }
    Err(Error::new(
        "TypeError",
        "expected an accumulate subclass with compatible layout",
    ))
}

fn accumulator(ctx: &mut Context, receiver: Id) -> Result<&mut Accumulator> {
    match cursor(ctx, receiver) {
        Some(Cursor::Accumulate(state)) => Ok(state),
        _ => Err(Error::new(
            "TypeError",
            "expected an initialized accumulate",
        )),
    }
}

pub(super) fn accumulate_next(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if let [base, receiver] = args
        && kwargs.is_empty()
        && accumulator(ctx, *receiver)?.base == *base
    {
        return next_accumulated(ctx, *receiver)?.ok_or_else(|| Error::new("StopIteration", ""));
    }
    Err(Error::new(
        "TypeError",
        "expected an initialized accumulate",
    ))
}

pub(in crate::dynamic) fn next_accumulated(ctx: &mut Context, receiver: Id) -> Result<Option<Id>> {
    let state = accumulator(ctx, receiver)?;
    if std::mem::take(&mut state.initial_pending) {
        return Ok(state.total);
    }
    let source = state.source;
    let item = if matches!(ctx.heap.get(source), Value::Iterator { .. }) {
        ctx.next_builtin_sequence_item(source)?
    } else {
        ctx.next_result(source)?
    };
    let Some(item) = item else {
        return Ok(None);
    };
    // Fetch the total after the source returns since its __next__ can reenter us.
    let state = accumulator(ctx, receiver)?;
    let value = match (state.function, state.total, state.addition) {
        (_, None, _) => item,
        (function, Some(total), addition) => {
            if addition
                && let (Value::Int(left), Value::Int(right)) =
                    (ctx.heap.get(total), ctx.heap.get(item))
            {
                let sum = crate::integer_binary(left, right, 0)?;
                ctx.alloc(Value::Int(sum))
            } else {
                ctx.heap.keep(item);
                ctx.heap.keep(total);
                if addition {
                    ctx.binary("+", total, item)?
                } else {
                    ctx.call(function, vec![total, item], Vec::new())?
                }
            }
        }
    };
    if let Some(old) = accumulator(ctx, receiver)?.total.replace(value) {
        ctx.heap.weak_edge_removed(old);
    }
    Ok(Some(value))
}

fn compatible_cursor_class(ctx: &Context, base: Id, class: Id, layout: &str) -> bool {
    ctx.itertools_layout(base) == Some(layout)
        && matches!(ctx.heap.get(class), Value::Class { ancestors, .. }
            if (class == base || ancestors.contains(&base))
                && ancestors.iter().all(|a| !matches!(ctx.heap.get(*a), Value::Native(kind) if kind != "object")))
}

pub(super) fn zip_longest_allocate(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if let [base, class, state] = args
        && kwargs.is_empty()
        && compatible_cursor_class(ctx, *base, *class, "itertools.zip_longest")
    {
        return Ok(instance_with_state(ctx, *base, *class, *state));
    }
    Err(Error::new(
        "TypeError",
        "expected a zip_longest subclass with compatible layout",
    ))
}

pub(super) fn starmap_allocate(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [base, class, function, source] = args else {
        return Err(Error::new(
            "TypeError",
            "invalid starmap allocation arguments",
        ));
    };
    if !kwargs.is_empty() || !compatible_cursor_class(ctx, *base, *class, "itertools.starmap") {
        return Err(Error::new(
            "TypeError",
            "expected a starmap subclass with compatible layout",
        ));
    }
    Ok(iterator_instance(ctx, *base, *class, *source, *function))
}

/// Fetch one row, fully unpack it, then invoke the saved callable.
pub(super) fn starmap_next(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if kwargs.is_empty()
        && let [base, receiver] = args
        && ctx.itertools_layout(*base) == Some("itertools.starmap")
        && let Value::Instance {
            member_values: Some(values),
            ..
        } = ctx.heap.get(*receiver)
        && let Some(state) = values.get(base)
        && let Value::Tuple(pair) = ctx.heap.get(*state)
        && let &[source, function] = pair.as_slice()
    {
        let mark = ctx.heap.temporary_mark();
        let row = ctx
            .next_result(source)?
            .ok_or_else(|| Error::new("StopIteration", ""))?;
        ctx.heap.keep(row);
        let tuple = super::super::builtins::call(ctx, "tuple", &[row], &[])?;
        // Converting a non-tuple releases the source row before the callable runs.
        ctx.finish_call_operands(mark, tuple);
        let args = ctx
            .tuple_items(tuple)?
            .expect("tuple constructor returned a tuple");
        return ctx.call(function, args, Vec::new());
    }
    Err(Error::new("TypeError", "expected an initialized starmap"))
}

pub(super) fn predicate_allocate(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [base, class, state, iterable] = args else {
        return Err(Error::new(
            "TypeError",
            "invalid predicate iterator allocation arguments",
        ));
    };
    if !kwargs.is_empty()
        || !matches!(ctx.heap.get(*class), Value::Class { ancestors, .. }
        if *class == *base || ancestors.contains(base))
    {
        return Err(Error::new(
            "TypeError",
            "expected a predicate iterator subclass",
        ));
    }
    let source = ctx.iterator(*iterable)?;
    Ok(iterator_instance(ctx, *base, *class, source, *state))
}

/// Retain the source and cursor state in a GC-traced, class-owned slot.
fn iterator_instance(ctx: &mut Context, base: Id, class: Id, source: Id, state: Id) -> Id {
    ctx.heap.keep(source);
    let state = ctx.alloc(Value::Tuple(vec![source, state]));
    instance_with_state(ctx, base, class, state)
}

fn instance_with_state(ctx: &mut Context, base: Id, class: Id, state: Id) -> Id {
    ctx.heap.keep(state);
    let result = ctx.instance(class, None);
    let Value::Instance { member_values, .. } = ctx.heap.get_mut(result) else {
        unreachable!("fresh iterator instance")
    };
    // The declaring class keys this private slot, so subclass descriptors cannot intercept it.
    member_values
        .get_or_insert_with(Default::default)
        .insert(base, state);
    result
}

pub(super) fn iterator_state(
    ctx: &Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    kind: &str,
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
    Err(Error::new(
        "TypeError",
        format!("expected a {kind} instance"),
    ))
}

/// Store the acquired iterator before pulling either member of the first pair.
pub(super) fn pairwise_allocate(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if let [base, class, source] = args
        && kwargs.is_empty()
        && compatible_cursor_class(ctx, *base, *class, "itertools.pairwise")
    {
        let state = ctx.alloc(Value::Tuple(vec![*source]));
        return Ok(instance_with_state(ctx, *base, *class, state));
    }
    Err(Error::new("TypeError", "expected a pairwise subclass"))
}

/// The absence of a second tuple item distinguishes an unstarted cursor from a previous None.
fn pairwise_state(ctx: &Context, base: Id, receiver: Id) -> Result<(Id, Option<Id>)> {
    if let Value::Instance {
        member_values: Some(values),
        ..
    } = ctx.heap.get(receiver)
        && let Some(&state) = values.get(&base)
        && let Value::Tuple(items) = ctx.heap.get(state)
    {
        match *items.as_slice() {
            [source] => return Ok((source, None)),
            [source, previous] => return Ok((source, Some(previous))),
            _ => {}
        }
    }
    Err(Error::new("TypeError", "expected an initialized pairwise"))
}

fn set_pairwise_state(ctx: &mut Context, base: Id, receiver: Id, source: Id, previous: Option<Id>) {
    let mut items = vec![source];
    items.extend(previous);
    let state = ctx.alloc(Value::Tuple(items));
    let Value::Instance {
        member_values: Some(values),
        ..
    } = ctx.heap.get_mut(receiver)
    else {
        unreachable!("validated pairwise receiver")
    };
    let old = values
        .insert(base, state)
        .expect("validated pairwise state");
    ctx.heap.weak_edge_removed(old);
}

/// Keep the previous item across a reentrant pull, preserving the source's actual exception.
pub(super) fn pairwise_next(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [base, receiver] = *args else {
        return Err(Error::new("TypeError", "expected a pairwise instance"));
    };
    if !kwargs.is_empty() || ctx.itertools_layout(base) != Some("itertools.pairwise") {
        return Err(Error::new("TypeError", "expected a pairwise instance"));
    }
    let (source, previous) = pairwise_state(ctx, base, receiver)?;
    if source == NONE {
        return Err(Error::new("StopIteration", ""));
    }
    ctx.heap.keep(source);
    let pull = |ctx: &mut Context| {
        let result = ctx
            .next_result(source)
            .and_then(|item| item.ok_or_else(|| Error::new("StopIteration", "")));
        if result.is_err() {
            set_pairwise_state(ctx, base, receiver, NONE, None);
        }
        result
    };
    let previous = match previous {
        Some(item) => item,
        None => {
            let item = pull(ctx)?;
            // A reentrant first pull can exhaust the same cursor before returning an item.
            if pairwise_state(ctx, base, receiver)?.0 == NONE {
                set_pairwise_state(ctx, base, receiver, NONE, None);
                return Err(Error::new("StopIteration", ""));
            }
            ctx.heap.keep(item);
            set_pairwise_state(ctx, base, receiver, source, Some(item));
            item
        }
    };
    ctx.heap.keep(previous);
    let following = pull(ctx)?;
    ctx.heap.keep(following);
    let (live_source, _) = pairwise_state(ctx, base, receiver)?;
    set_pairwise_state(ctx, base, receiver, live_source, Some(following));
    Ok(ctx.alloc(Value::Tuple(vec![previous, following])))
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

/// Extract a machine-sized Python integer, including subclasses, without invoking user hooks.
pub(in crate::dynamic) fn machine_integer(ctx: &Context, value: Id) -> Option<i64> {
    match ctx.heap.get(ctx.scalar_value(value)) {
        Value::Int(number) => number.to_i64(),
        Value::Bool(value) => Some(i64::from(*value)),
        _ => None,
    }
}

/// Retain slow-mode operands and normalize the native integer start like CPython.
pub(super) fn count(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [start, step] =
        super::super::builtins::bind_method_arguments("count", args, kwargs, ["start", "step"], 2)?;
    let mut current = start.unwrap_or_else(|| ctx.int(0));
    let step = step.unwrap_or_else(|| ctx.int(1));
    number(ctx, current)?;
    number(ctx, step)?;
    let fast = if machine_integer(ctx, step) == Some(1)
        && let Some(n) = machine_integer(ctx, current).and_then(|n| isize::try_from(n).ok())
    {
        // CPython produces an exact int from its machine counter. A cached
        // small int has the same identity as the original exact argument.
        if !matches!(ctx.heap.get(current), Value::Int(_)) || !(-5..=256).contains(&n) {
            current = ctx.int(n as i64);
        }
        true
    } else {
        false
    };
    Ok(ctx.alloc(Value::Count {
        current,
        step,
        fast,
    }))
}

/// Advance unit integer strides directly. Slow Python addition may reenter this cursor.
pub(in crate::dynamic) fn advance(ctx: &mut Context, id: Id) -> Result<Id> {
    let Value::Count {
        current,
        step,
        fast,
    } = *ctx.heap.get(id)
    else {
        unreachable!("count cursor")
    };
    let position = fast.then(|| machine_integer(ctx, current).expect("machine position"));
    let (item, following, fast) = if let Some(n) = position
        && n < isize::MAX as i64
    {
        (current, ctx.int(n + 1), true)
    } else {
        // CPython materializes maxsize when it first attempts the slow addition.
        // A failing callback may inspect that argument again on a subsequent attempt.
        let item = if fast {
            ctx.int(isize::MAX as i64)
        } else {
            current
        };
        // A nested next() can replace the cursor's reference to the value we owe.
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(item);
        (item, ctx.binary("+", item, step)?, false)
    };
    let Value::Count {
        current: slot,
        fast: mode,
        ..
    } = ctx.heap.get_mut(id)
    else {
        unreachable!("count cursor")
    };
    *slot = following;
    *mode = fast;
    Ok(item)
}

pub(super) fn method(
    ctx: &mut Context,
    owner: &str,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("descriptor '{name}' of '{owner}' object needs an argument"),
        ));
    };
    let storage = ctx.scalar_value(receiver);
    if !matches!(
        (owner, ctx.heap.get(storage)),
        (COUNT, Value::Count { .. }) | (REPEAT, Value::Repeat(_))
    ) {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' requires a '{owner}' object but received a '{}'",
                ctx.type_name(receiver)
            ),
        ));
    }
    if !kwargs.is_empty() {
        let message = if name == "__length_hint__" {
            "repeat.__length_hint__() takes no keyword arguments".to_owned()
        } else {
            format!("wrapper {name}() takes no keyword arguments")
        };
        return Err(Error::new("TypeError", message));
    }
    if name == "__getattribute__" {
        let [key] = rest else {
            return Err(Error::new(
                "TypeError",
                format!("expected 1 argument, got {}", rest.len()),
            ));
        };
        let attr = super::super::builtins::attribute_name(ctx, *key)?;
        return ctx.default_get_attr(receiver, &attr);
    }
    if !rest.is_empty() {
        let message = if name == "__length_hint__" {
            format!(
                "repeat.__length_hint__() takes no arguments ({} given)",
                rest.len()
            )
        } else {
            format!("expected 0 arguments, got {}", rest.len())
        };
        return Err(Error::new("TypeError", message));
    }
    match name {
        "__repr__" => {
            let text = ctx.checked_cursor_slot_repr(receiver)?;
            Ok(ctx.text(text))
        }
        "__iter__" => Ok(receiver),
        "__next__" => ctx
            .next_result(storage)?
            .ok_or_else(|| Error::new("StopIteration", "")),
        "__length_hint__" if let Value::Repeat(repeat) = ctx.heap.get(storage) => {
            let n = repeat
                .remaining
                .ok_or_else(|| Error::new("TypeError", "len() of unsized object"))?;
            Ok(ctx.int(n as i64))
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("{owner} has no method '{name}'"),
        )),
    }
}
