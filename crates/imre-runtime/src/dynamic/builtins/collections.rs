use super::super::calls::MappingOperation;
use super::super::value::{DictViewKind, NOT_IMPLEMENTED};
use super::*;
use num_traits::Signed;

static SORTING_EMPTY: crate::SortMarker<Id> = crate::SortMarker(NONE);

/// Clearing an already detached list leaves the active sort's marker intact.
fn take_list_contents(items: &mut Vec<Id>) -> Vec<Id> {
    if SORTING_EMPTY.is_untouched(items) {
        Vec::new()
    } else {
        std::mem::take(items)
    }
}

/// Clear container storage, then finalize any buffered writer whose last owner was removed.
pub(in super::super) fn clear_container(ctx: &mut Context, receiver: Id) {
    let released = ctx.heap.has_unbound_finalizer(NONE) && ctx.heap.clear_owns_finalizer(receiver);
    match ctx.heap.get_mut(receiver) {
        Value::List(items) => {
            for item in take_list_contents(items) {
                ctx.heap.weak_edge_removed(item);
            }
        }
        Value::Dict(entries) | Value::DefaultDict { entries, .. } | Value::Set { entries, .. } => {
            for (key, value) in entries.clear() {
                ctx.heap.weak_edge_removed(key);
                ctx.heap.weak_edge_removed(value);
            }
        }
        Value::Deque(ring) => {
            for item in ring.take_items() {
                ctx.heap.weak_edge_removed(item);
            }
        }
        _ => unreachable!("clearable container"),
    }
    if released {
        ctx.collect_unreachable();
    }
}

/// Select the first minimum or maximum, evaluating each key as its candidate arrives.
pub(super) fn extremum(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some(&first) = args.first() else {
        return Err(type_error(format!(
            "{name} expected at least 1 argument, got 0"
        )));
    };
    if let [left, right] = *args
        && kwargs.is_empty()
    {
        // Two exact numbers have no comparison callbacks or iterator state to retain.
        let order = match (ctx.heap.get(right), ctx.heap.get(left)) {
            (Value::Int(right), Value::Int(left)) => Some(right.cmp(left)),
            (Value::Float(right), Value::Float(left)) => right.partial_cmp(left),
            _ => None,
        };
        if let Some(order) = order {
            let better = if name == "min" {
                order.is_lt()
            } else {
                order.is_gt()
            };
            return Ok(if better { right } else { left });
        }
    }
    let [key, default] = bind_method_arguments(name, &[], kwargs, ["key", "default"], 0)?;
    let key = key.filter(|&key| key != NONE);
    if args.len() > 1 {
        if default.is_some() {
            return Err(type_error(format!(
                "Cannot specify a default for {name}() with multiple positional arguments"
            )));
        }
        let mut values = args.iter().copied();
        return select_item(ctx, name, key, None, |_| Ok(values.next()));
    }
    if matches!(ctx.heap.get(first), Value::Instance { .. })
        && ctx.slot(first, "__iter__").is_none()
        && ctx.slot(first, "__getitem__").is_some()
    {
        let mut index = 0_i64;
        return select_item(ctx, name, key, default, |ctx| {
            let position = ctx.int(index);
            match ctx.get_item(first, position) {
                Ok(value) => {
                    index += 1;
                    Ok(Some(value))
                }
                Err(error) => {
                    let index_error = ctx.builtin("IndexError");
                    if ctx.raised_error_matches(&error, index_error)
                        || ctx.is_stop_iteration(&error)
                    {
                        Ok(None)
                    } else {
                        Err(error)
                    }
                }
            }
        });
    }
    let iterator = ctx.iterator(first)?;
    select_item(ctx, name, key, default, |ctx| ctx.next_item(iterator))
}

/// Keep the winning item and key alive across iterator and comparison callbacks.
fn select_item(
    ctx: &mut Context,
    name: &str,
    key: Option<Id>,
    default: Option<Id>,
    mut next: impl FnMut(&mut Context) -> Result<Option<Id>>,
) -> Result<Id> {
    let mark = ctx.heap.temporary_mark();
    let Some(mut winner) = next(ctx)? else {
        return default.ok_or_else(|| {
            Error::new("ValueError", format!("{name}() iterable argument is empty"))
        });
    };
    let comparison = if name == "min" { "<" } else { ">" };
    let rank = |ctx: &mut Context, item| match key {
        Some(key) => ctx.call(key, vec![item], Vec::new()),
        None => Ok(item),
    };
    ctx.heap.keep(winner);
    let mut winning_key = rank(ctx, winner)?;
    let mut limit = ctx.heap.streaming_collection_limit();
    loop {
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(winner);
        ctx.heap.keep(winning_key);
        ctx.collect_streaming_garbage(&mut limit);
        let Some(item) = next(ctx)? else {
            break;
        };
        ctx.heap.keep(item);
        let item_key = rank(ctx, item)?;
        let better = ctx.binary(comparison, item_key, winning_key)?;
        if ctx.truth(better)? {
            winner = item;
            winning_key = item_key;
        }
    }
    ctx.heap.release_temporaries_since(mark);
    ctx.heap.keep(winner);
    Ok(winner)
}

/// Materialize the iterable before parsing the forwarded list.sort keywords.
pub(super) fn sorted(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [iterable] = args else {
        return Err(type_error(format!(
            "sorted expected 1 argument, got {}",
            args.len()
        )));
    };
    let list = ctx.alloc(Value::List(Vec::new()));
    let mark = ctx.heap.temporary_mark();
    list_extend(ctx, list, *iterable)?;
    ctx.heap.release_temporaries_since(mark);
    list_sort(ctx, list, &[], kwargs)?;
    Ok(list)
}

/// Leave an empty list visible to callbacks and restore its original elements on every exit.
fn list_sort(ctx: &mut Context, receiver: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [key, reverse] = bind_method_arguments("sort", args, kwargs, ["key", "reverse"], 0)?;
    let reverse = ctx.optional_truth(reverse, false)?;
    let Value::List(live) = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    let count = live.len();
    ctx.heap.reserve_temporaries(count)?;
    let Value::List(live) = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    let mut values = std::mem::replace(live, SORTING_EMPTY.empty());
    for &value in &values {
        ctx.heap.keep(value);
    }
    let result = sort_list_items(ctx, &mut values, key, reverse);
    let Value::List(live) = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    let modified = !SORTING_EMPTY.is_untouched(live);
    *live = values;
    result?;
    if modified {
        return Err(Error::new("ValueError", "list modified during sort"));
    }
    Ok(NONE)
}

/// Evaluate each key in input order before changing the order of the items.
fn sort_list_items(
    ctx: &mut Context,
    values: &mut [Id],
    key: Option<Id>,
    reverse: bool,
) -> Result<()> {
    if let Some(key) = key.filter(|&key| key != NONE) {
        let mut keyed = Vec::new();
        keyed
            .try_reserve_exact(values.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        let mark = ctx.heap.temporary_mark();
        for &value in values.iter() {
            let sort_key = ctx.call(key, vec![value], vec![])?;
            ctx.heap.release_temporaries_since(mark + keyed.len());
            ctx.heap.reserve_temporaries(1)?;
            ctx.heap.keep(sort_key);
            keyed.push((sort_key, value));
            ctx.heap.reserve_slot_or_request_collection();
            ctx.collect_garbage();
        }
        sort_values(ctx, &mut keyed, reverse, |(key, _)| key)?;
        for (slot, (_, value)) in values.iter_mut().zip(keyed) {
            *slot = value;
        }
    } else {
        sort_values(ctx, values, reverse, |value| value)?;
    }
    Ok(())
}

pub(in super::super) fn sort_values<T: Copy>(
    ctx: &mut Context,
    values: &mut [T],
    reverse: bool,
    key: impl Fn(T) -> Id,
) -> Result<()> {
    crate::objects::sort_by_less(
        values,
        reverse,
        |a, b| {
            let result = ctx.binary("<", key(a), key(b))?;
            ctx.truth(result)
        },
        || Error::new("MemoryError", ""),
    )
}

/// Borrows the current list storage between iterator callbacks.
fn list_items_mut(ctx: &mut Context, receiver: Id) -> Result<&mut Vec<Id>> {
    match ctx.heap.get_mut(receiver) {
        Value::List(items) => Ok(items),
        _ => Err(type_error("expected list")),
    }
}

pub(super) fn entries(ctx: &Context, dict: Id) -> Result<&[(Id, Id)]> {
    ctx.dict_entries(dict)
        .ok_or_else(|| type_error("expected a dictionary"))
}

/// Copy the mapping before a caller invokes Python code on its entries.
pub(in super::super) fn copy_dict(ctx: &mut Context, object: Id, storage: Id) -> Result<Id> {
    let copy = ctx.dict(Vec::new());
    if entries(ctx, storage)?.is_empty() {
        return Ok(copy);
    }
    if let Some(storage) = ctx.raw_dict_source(object)
        && let Value::Dict(entries) | Value::DefaultDict { entries, .. } = ctx.heap.get(storage)
        && let Some(entries) = entries.clone_dense()?
    {
        *ctx.heap.get_mut(copy) = Value::Dict(entries);
    } else {
        ctx.for_each_mapping_item(
            object,
            copy,
            MappingOperation::Copy,
            |ctx, key, value, hash| {
                ctx.set_dictionary_key(copy, key, value, hash, true)
                    .map(|_| ())
            },
        )?;
    }
    Ok(copy)
}

/// Apply the positional entries before keyword entries, as in dict.update.
pub(in super::super) fn update(
    ctx: &mut Context,
    dict: Id,
    input: Option<Id>,
    kwargs: &[(String, Id)],
) -> Result<()> {
    if let Some(input) = input
        && !ctx.for_each_mapping_item(
            input,
            dict,
            MappingOperation::Update,
            |ctx, key, value, hash| {
                ctx.set_dictionary_key(dict, key, value, hash, true)
                    .map(|_| ())
            },
        )?
    {
        let iterator = ctx.iterator(input)?;
        for index in 0.. {
            let Some(pair) = ctx.next_item(iterator)? else {
                break;
            };
            let pair = if matches!(ctx.heap.get(pair), Value::List(_) | Value::Tuple(_)) {
                ctx.items(pair)?
            } else {
                // PySequence_Fast replaces TypeError only while obtaining the iterator.
                let iterator = ctx.iterator(pair).map_err(|error| {
                    let class = ctx.builtin("TypeError");
                    if ctx.raised_error_matches(&error, class) {
                        type_error("object is not iterable")
                    } else {
                        error
                    }
                })?;
                ctx.items(iterator)?
            };
            let [key, value] = pair.as_slice() else {
                return Err(Error::new(
                    "ValueError",
                    format!(
                        "dictionary update sequence element #{index} has length {}; 2 is required",
                        pair.len()
                    ),
                ));
            };
            ctx.put_item(dict, *key, *value)?;
        }
    }
    for (key, value) in kwargs {
        let key = ctx.text(key.clone());
        ctx.put_item(dict, key, *value)?;
    }
    Ok(())
}

/// Python tuples inherit hashability from their elements, including named tuples.
pub(in super::super) fn hashable(ctx: &Context, value: Id) -> bool {
    unhashable_part(ctx, value).is_none()
}

fn unhashable_part(ctx: &Context, value: Id) -> Option<Id> {
    match ctx.tuple_backing(value) {
        Value::Weak(wr) if wr.kind != super::super::weakref::Kind::Reference => Some(value),
        Value::Weak(wr) => wr.target.and_then(|target| unhashable_part(ctx, target)),
        Value::Union(union) => unhashable_part(ctx, union.args),
        Value::KeyWrapper { .. }
        | Value::Deque(_)
        | Value::List(_)
        | Value::ByteArray(_)
        | Value::Array(_)
        | Value::Context(_)
        | Value::ContextToken { .. }
        | Value::Set { frozen: false, .. }
        | Value::Dict(_)
        | Value::DefaultDict { .. }
        | Value::Instance {
            dictionary: Some(_),
            ..
        }
        | Value::DictView {
            kind: DictViewKind::Keys | DictViewKind::Items,
            ..
        } => Some(value),
        Value::Slice(values) => values.iter().find_map(|&x| unhashable_part(ctx, x)),
        Value::Tuple(values) | Value::NamedTuple { values, .. } => {
            values.iter().find_map(|&x| unhashable_part(ctx, x))
        }
        _ => None,
    }
}

/// Removes an equal key from the builtin dictionary payload and returns its stored value.
pub(in super::super) fn remove_dict_item(
    ctx: &mut Context,
    dict: Id,
    key: Id,
) -> Result<Option<Id>> {
    remove_dict_item_hashed(ctx, dict, key, None)
}

pub(in super::super) fn remove_dict_item_hashed(
    ctx: &mut Context,
    dict: Id,
    key: Id,
    hash: Option<i64>,
) -> Result<Option<Id>> {
    let Some(index) = ctx.find_dictionary_key(dict, key, hash)?.1 else {
        return Ok(None);
    };
    Ok(Some(ctx.heap.remove_table_entry(dict, index)?.1))
}

pub(in super::super) fn set_member(
    ctx: &mut Context,
    set: Id,
    value: Id,
    allow_mutable_key: bool,
) -> Result<bool> {
    // A mutable set may be tested for membership as an equivalent frozen key.
    let value = if allow_mutable_key
        && let Value::Set {
            entries,
            frozen: false,
        } = ctx.heap.get(value)
    {
        ctx.alloc(Value::Set {
            entries: entries.try_clone()?,
            frozen: true,
        })
    } else {
        value
    };
    Ok(ctx
        .find_dictionary_key(ctx.scalar_value(set), value, None)?
        .1
        .is_some())
}

pub(in super::super) fn set_add(ctx: &mut Context, set: Id, value: Id) -> Result<()> {
    ctx.set_dictionary_key(ctx.scalar_value(set), value, NONE, None, false)?;
    Ok(())
}

pub(in super::super) fn update_set(ctx: &mut Context, target: Id, source: Id) -> Result<()> {
    let target = ctx.scalar_value(target);
    let storage = ctx.scalar_value(source);
    if matches!(ctx.heap.get(storage), Value::Set { .. })
        || matches!(ctx.heap.get(source), Value::Dict(_))
    {
        return merge_set_entries(ctx, target, storage);
    }
    let roots = ctx.heap.temporary_mark();
    let iterator = ctx.iterator(source)?;
    ctx.heap.keep(iterator);
    let step = ctx.heap.temporary_mark();
    let mut failed_key = None;
    let updated: Result<()> = (|| {
        while let Some(key) = ctx.next_item(iterator)? {
            ctx.heap.keep(key);
            failed_key = Some(key);
            set_add(ctx, target, key)?;
            failed_key = None;
            // An equal key stays out of the result and can finalize before the next yield.
            ctx.discard_truth(step);
        }
        Ok(())
    })();

    ctx.heap.release_temporaries_since(roots);
    if let Err(error) = &updated {
        for id in error.roots() {
            ctx.heap.keep(id);
        }
    }
    // CPython releases the iterator before a key whose insertion failed.
    let key_mark = ctx.heap.temporary_mark();
    if let Some(key) = failed_key {
        ctx.heap.keep(key);
    }
    let iterator_mark = ctx.heap.temporary_mark();
    ctx.heap.keep(iterator);
    ctx.discard_truth(iterator_mark);
    ctx.discard_truth(key_mark);
    updated
}

/// Release each yielded key before requesting the next one or dropping the iterator.
fn iterable_members_match(ctx: &mut Context, receiver: Id, other: Id, present: bool) -> Result<Id> {
    let iterator = ctx.iterator(other)?;
    ctx.heap.keep(iterator);
    let mark = ctx.heap.temporary_mark();
    let mut limit = ctx.heap.streaming_collection_limit();
    loop {
        ctx.collect_streaming_garbage(&mut limit);
        let Some(item) = ctx.next_item(iterator)? else {
            return Ok(TRUE);
        };
        ctx.heap.keep(item);
        let member = match set_member(ctx, receiver, item, false) {
            Ok(member) => member,
            Err(error) => return ctx.release_with_result(mark, Err(error)),
        };
        ctx.discard_truth(mark);
        if member != present {
            return Ok(FALSE);
        }
    }
}

fn set_issubset(ctx: &mut Context, receiver: Id, other: Id) -> Result<Id> {
    let storage = ctx.scalar_value(other);
    if matches!(ctx.heap.get(storage), Value::Set { .. }) {
        return ctx.order_sets("<=", receiver, storage);
    }

    let roots = ctx.heap.temporary_mark();
    let found = intersect_iterable(ctx, receiver, other)?;
    let subset = ctx.stored_entries(found).len() == ctx.stored_entries(receiver).len();
    ctx.discard_truth(roots);
    Ok(boolean(subset))
}

/// Intersect left to right, releasing each intermediate result before the next operand.
fn set_intersection(ctx: &mut Context, receiver: Id, operands: &[Id]) -> Result<Id> {
    if operands.is_empty() {
        return ctx.set_binary("&", receiver, receiver, false);
    }
    let roots = ctx.heap.temporary_mark();
    let mut result = receiver;
    for &other in operands {
        let storage = ctx.scalar_value(other);
        result = if matches!(ctx.heap.get(storage), Value::Set { .. }) {
            ctx.set_binary("&", result, storage, false)?
        } else {
            intersect_iterable(ctx, result, other)?
        };
        ctx.discard_temporaries(roots, Some(result));
    }
    Ok(result)
}

/// Prove an exact string contains none of a set's single-byte string keys.
fn disjoint_ascii_characters(ctx: &Context, receiver: Id, other: Id) -> bool {
    let Value::Str(text) = ctx.heap.get(other) else {
        return false;
    };
    let entries = ctx.stored_entries(receiver);
    if entries.len() > 128 {
        return false;
    }
    let mut members = 0_u128;
    for &(key, _) in entries.iter() {
        let Value::Str(character) = ctx.heap.get(key) else {
            return false;
        };
        let [byte] = character.as_bytes() else {
            return false;
        };
        members |= 1_u128 << byte;
    }
    text.bytes()
        .all(|byte| !byte.is_ascii() || members & (1_u128 << byte) == 0)
}

/// Hash each yielded key once and stop on a match that completes the intersection.
fn intersect_iterable(ctx: &mut Context, receiver: Id, other: Id) -> Result<Id> {
    let roots = ctx.heap.temporary_mark();
    let found = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: matches!(ctx.heap.get(receiver), Value::Set { frozen: true, .. }),
    });
    if disjoint_ascii_characters(ctx, receiver, other) {
        return Ok(found);
    }
    let iterator = ctx.iterator(other)?;
    ctx.heap.keep(iterator);
    let step = ctx.heap.temporary_mark();
    let mut failed_key = None;
    let scan: Result<()> = (|| {
        let mut limit = ctx.heap.streaming_collection_limit();
        loop {
            ctx.collect_streaming_garbage(&mut limit);
            let Some(key) = ctx.next_item(iterator)? else {
                return Ok(());
            };
            ctx.heap.keep(key);
            failed_key = Some(key);
            let hash = ctx.dictionary_hash(key)?;
            let (_, member) = ctx.find_dictionary_key(receiver, key, Some(hash))?;
            let mut done = false;
            if member.is_some() {
                ctx.set_dictionary_key(found, key, NONE, Some(hash), false)?;
                done = ctx.stored_entries(found).len() >= ctx.stored_entries(receiver).len();
            }
            failed_key = None;
            ctx.discard_truth(step);
            if done {
                return Ok(());
            }
        }
    })();

    // Rebuild the temporary roots without running Python. Dropping the iterator
    // can mutate the receiver in __del__ or a generator's finally clause.
    ctx.heap.release_temporaries_since(roots);
    if let Err(error) = &scan {
        for id in error.roots() {
            ctx.heap.keep(id);
        }
    }
    let key_mark = ctx.heap.temporary_mark();
    if let Some(key) = failed_key {
        ctx.heap.keep(key);
    }
    let found_mark = ctx.heap.temporary_mark();
    ctx.heap.keep(found);
    let iterator_mark = ctx.heap.temporary_mark();
    ctx.heap.keep(iterator);
    ctx.discard_truth(iterator_mark);
    if scan.is_err() {
        ctx.discard_truth(found_mark);
        ctx.discard_truth(key_mark);
    }
    scan.map(|()| found)
}

fn merge_set_entries(ctx: &mut Context, target: Id, source: Id) -> Result<()> {
    if target == source {
        return Ok(());
    }
    if matches!(ctx.heap.get(target), Value::Set { entries, .. } if entries.is_empty())
        && let Value::Set { entries, .. } = ctx.heap.get(source)
    {
        let copy = entries.try_clone()?;
        let Value::Set { entries, .. } = ctx.heap.get_mut(target) else {
            unreachable!()
        };
        *entries = copy;
        return Ok(());
    }
    let mut position = 0;
    while let Some((at, key, _, hash)) = ctx.dictionary_pair(source, position) {
        ctx.set_dictionary_key(target, key, NONE, hash, false)?;
        position = at + 1;
    }
    Ok(())
}

pub(super) fn construct(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if matches!(name, "set" | "frozenset") {
        if !kwargs.is_empty() {
            return Err(type_error(format!("{name}() takes no keyword arguments")));
        }
        if args.len() > 1 {
            return Err(type_error(format!(
                "{name} expected at most 1 argument, got {}",
                args.len()
            )));
        }
        let frozen = name == "frozenset";
        if frozen
            && let [one] = args
            && matches!(ctx.heap.get(*one), Value::Set { frozen: true, .. })
        {
            return Ok(*one);
        }
        let set = ctx.alloc(Value::Set {
            entries: Default::default(),
            frozen,
        });
        if let [source] = args {
            update_set(ctx, set, *source)?;
        }
        return Ok(set);
    }
    arity(name, args, 0, 1)?;
    if name == "dict" {
        let dict = ctx.dict(Vec::new());
        update(ctx, dict, args.first().copied(), kwargs)?;
        return Ok(dict);
    }
    positional(name, kwargs)?;
    if name == "tuple"
        && let [one] = args
        && matches!(ctx.heap.get(*one), Value::Tuple(_))
    {
        return Ok(*one);
    }
    if name == "list"
        && let [source] = args
        && let Some(list) = ctx.list_from_int_character_map(*source)
    {
        return list;
    }
    // list() consults the cursor's signed length hint before pulling an item.
    if name == "list"
        && let [source] = args
        && matches!(ctx.heap.get(*source), Value::SetIterator(_))
    {
        let list = ctx.alloc(Value::List(Vec::new()));
        list_extend(ctx, list, *source)?;
        return Ok(list);
    }
    if let [source] = args
        && matches!(ctx.heap.get(*source), Value::Generator { .. })
    {
        return ctx.collect_iterable(*source, name == "tuple");
    }
    let items = args
        .first()
        .map(|&it| ctx.items(it))
        .transpose()?
        .unwrap_or_default();
    match name {
        "list" => Ok(ctx.alloc(Value::List(items))),
        "tuple"
            if items.is_empty()
                || args
                    .first()
                    .is_some_and(|&id| matches!(ctx.heap.get(id), Value::List(_))) =>
        {
            Ok(ctx.heap.packed_tuple(items, false))
        }
        "tuple" => Ok(ctx.alloc(Value::Tuple(items))),
        _ => Err(type_error(format!("unknown collection {name}"))),
    }
}

/// Build a fresh mapping through the bound class, sharing the supplied value.
fn fromkeys(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some((&class, args)) = args.split_first() else {
        return Err(type_error(
            "descriptor 'fromkeys' of 'dict' object needs an argument",
        ));
    };
    super::super::stdlib::datetime::validate_classmethod_receiver(ctx, "dict", "fromkeys", class)?;
    if !kwargs.is_empty() {
        let name = ctx.class_name(class);
        let name = match ctx.own_class_attr(class, "__qualname__") {
            Some(qualname) => ctx.str_value(qualname)?,
            None => name.rsplit('.').next().unwrap_or(name),
        };
        return Err(type_error(format!(
            "{name}.fromkeys() takes no keyword arguments"
        )));
    }
    let (keys, value) = match args {
        [keys] => (*keys, NONE),
        [keys, value] => (*keys, *value),
        [] => return Err(type_error("fromkeys expected at least 1 argument, got 0")),
        _ => {
            return Err(type_error(format!(
                "fromkeys expected at most 2 arguments, got {}",
                args.len()
            )));
        }
    };
    let result = ctx.call(class, Vec::new(), Vec::new())?;
    ctx.heap.keep(result);
    if matches!(ctx.heap.get(result), Value::Dict(_))
        && matches!(ctx.heap.get(keys), Value::Dict(_) | Value::Set { .. })
    {
        let mut position = 0;
        while let Some((at, key, _, hash)) = ctx.dictionary_pair(keys, position) {
            ctx.set_dictionary_key(result, key, value, hash, true)?;
            position = at + 1;
        }
    } else {
        let iterator = ctx.iterator(keys)?;
        let roots = ctx.heap.temporary_mark();
        ctx.heap.keep(iterator);
        ctx.heap.keep(value);
        let step = ctx.heap.temporary_mark();
        let mut limit = ctx.heap.streaming_collection_limit();
        let populated: Result<()> = (|| {
            loop {
                ctx.heap.release_temporaries_since(step);
                let due = ctx.heap.prepare_streaming_collection(limit);
                ctx.collect_garbage();
                if due {
                    limit = ctx.heap.streaming_collection_limit();
                }
                let Some(key) = ctx.next_item(iterator)? else {
                    return Ok(());
                };
                ctx.put_item(result, key, value)?;
            }
        })();
        ctx.heap.release_temporaries_since(roots);
        populated?;
    }
    Ok(result)
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if matches!(
        name,
        "list.__add__"
            | "list.__contains__"
            | "list.__setitem__"
            | "list.__iadd__"
            | "list.__init__"
            | "list.__iter__"
            | "tuple.__iter__"
            | "list.__repr__"
            | "tuple.__repr__"
            | "list.__len__"
            | "tuple.__len__"
            | "set.__repr__"
            | "frozenset.__repr__"
            | "set.__iter__"
            | "frozenset.__iter__"
            | "set.__len__"
            | "frozenset.__len__"
    ) {
        return builtin_slot(ctx, name, args, kwargs);
    }
    if matches!(
        name,
        "set.__class_getitem__" | "frozenset.__class_getitem__" | "mappingproxy.__class_getitem__"
    ) {
        let Some((&owner, rest)) = args.split_first() else {
            return Err(type_error(format!(
                "unbound method {name}() needs an argument"
            )));
        };
        let qualified = format!("{}.__class_getitem__", ctx.class_qualname(owner));
        let index = super::one_positional_arg(&qualified, rest, kwargs)?;
        return Ok(super::super::typing::builtin_alias(ctx, owner, index));
    }
    if name == "dict.fromkeys" {
        return fromkeys(ctx, args, kwargs);
    }
    if let Some((owner, method)) = name.split_once('.')
        && ((owner == "list" && LIST_METHODS.contains(&method))
            || matches!(name, "tuple.count" | "tuple.index"))
    {
        let Some((&receiver, args)) = args.split_first() else {
            return Err(type_error(format!(
                "unbound method {name}() needs an argument"
            )));
        };
        if !ctx.builtin_slot_accepts(receiver, owner) {
            return Err(type_error(format!(
                "descriptor '{method}' for '{owner}' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            )));
        }
        let receiver = ctx.scalar_value(receiver);
        let one_arg = || {
            if !kwargs.is_empty() {
                return Err(type_error(format!("{name}() takes no keyword arguments")));
            }
            match args {
                [value] => Ok(*value),
                _ => Err(type_error(format!(
                    "{name}() takes exactly one argument ({} given)",
                    args.len()
                ))),
            }
        };
        return match method {
            "__getitem__" => ctx.sequence_subscript(receiver, one_arg()?),
            "append" => {
                let value = one_arg()?;
                let items = list_items_mut(ctx, receiver)?;
                items
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                items.push(value);
                Ok(NONE)
            }
            "index" => sequence_index(ctx, owner, receiver, args, kwargs),
            "extend" => {
                list_extend(ctx, receiver, one_arg()?)?;
                Ok(NONE)
            }
            "count" => {
                let value = one_arg()?;
                let mut count = 0;
                let mut start = 0;
                while let Some(i) = find_sequence_item(ctx, receiver, value, start..i64::MAX)? {
                    count += 1;
                    start = i + 1;
                }
                Ok(ctx.int(count))
            }
            "remove" => {
                let i = find_sequence_item(ctx, receiver, one_arg()?, 0..i64::MAX)?
                    .ok_or_else(|| Error::new("ValueError", "list.remove(x): x not in list"))?
                    as usize;
                let items = list_items_mut(ctx, receiver)?;
                if i < items.len() {
                    let removed = items.remove(i);
                    ctx.heap.weak_edge_removed(removed);
                }
                Ok(NONE)
            }
            "insert" => {
                if !kwargs.is_empty() {
                    return Err(type_error("list.insert() takes no keyword arguments"));
                }
                let [index, value] = args else {
                    return Err(type_error(format!(
                        "insert expected 2 arguments, got {}",
                        args.len()
                    )));
                };
                let index = list_position_arg(ctx, *index)?;
                let items = list_items_mut(ctx, receiver)?;
                let len = items.len() as i64;
                let index = if index < 0 {
                    (len + index).max(0)
                } else {
                    index.min(len)
                };
                items
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                items.insert(index as usize, *value);
                Ok(NONE)
            }
            "pop" => list_pop(ctx, receiver, args, kwargs),
            "sort" => list_sort(ctx, receiver, args, kwargs),
            _ => {
                if !kwargs.is_empty() {
                    return Err(type_error(format!("{name}() takes no keyword arguments")));
                }
                if !args.is_empty() {
                    return Err(type_error(format!(
                        "{name}() takes no arguments ({} given)",
                        args.len()
                    )));
                }
                let Value::List(items) = ctx.heap.get_mut(receiver) else {
                    unreachable!()
                };
                if method == "copy" {
                    let copy = super::super::copy_ids(items)?;
                    return Ok(ctx.alloc(Value::List(copy)));
                }
                if method == "reverse" {
                    items.reverse();
                } else {
                    clear_container(ctx, receiver);
                }
                Ok(NONE)
            }
        };
    }
    let slot = super::dict_slot(name);
    let set_method = name.split_once('.').filter(|(owner, method)| {
        *method != "__reduce__" && super::set_methods(owner).contains(method)
    });
    let (&object, args) = args
        .split_first()
        .ok_or_else(|| match name.strip_prefix("dict.") {
            Some(method) if slot => type_error(format!(
                "descriptor '{method}' of 'dict' object needs an argument"
            )),
            Some(_) => type_error(format!("unbound method {name}() needs an argument")),
            None if set_method.is_some() => {
                type_error(format!("unbound method {name}() needs an argument"))
            }
            None => type_error("missing collection receiver"),
        })?;
    let receiver = if let Some((kind, method)) = set_method {
        let storage = ctx.scalar_value(object);
        if !matches!(ctx.heap.get(storage), Value::Set { frozen, .. } if *frozen == (kind == "frozenset"))
        {
            return Err(type_error(format!(
                "descriptor '{method}' for '{kind}' objects doesn't apply to a '{}' object",
                ctx.type_name(object)
            )));
        }
        storage
    } else if let Some(method) = name.strip_prefix("dict.") {
        ctx.dict_storage(object).ok_or_else(|| {
            let kind = ctx.type_name(object);
            if slot {
                type_error(format!(
                    "descriptor '{method}' requires a 'dict' object but received a '{kind}'"
                ))
            } else {
                type_error(format!(
                    "descriptor '{method}' for 'dict' objects doesn't apply to a '{kind}' object"
                ))
            }
        })?
    } else {
        object
    };
    if name == "dict.__repr__" {
        if !kwargs.is_empty() {
            return Err(type_error("wrapper __repr__() takes no keyword arguments"));
        }
        if !args.is_empty() {
            return Err(type_error(format!(
                "expected 0 arguments, got {}",
                args.len()
            )));
        }
        let representation = ctx.checked_container_slot_repr(object, receiver)?;
        return Ok(ctx.text(representation));
    }
    if matches!(name, "dict.__or__" | "dict.__ror__" | "dict.__ior__") {
        if !kwargs.is_empty() {
            return Err(type_error(format!(
                "wrapper {}() takes no keyword arguments",
                &name[5..]
            )));
        }
        let [other] = args else {
            return Err(type_error(format!(
                "expected 1 argument, got {}",
                args.len()
            )));
        };
        if name == "dict.__ior__" {
            update(ctx, receiver, Some(*other), &[])?;
            return Ok(object);
        }
        let Some(other_storage) = ctx.dict_storage(*other) else {
            return Ok(NOT_IMPLEMENTED);
        };
        let (left, storage, right) = if name == "dict.__ror__" {
            (*other, other_storage, object)
        } else {
            (object, receiver, *other)
        };
        let merged = copy_dict(ctx, left, storage)?;
        update(ctx, merged, Some(right), &[])?;
        return Ok(merged);
    }
    if name == "dict.__eq__" {
        if !kwargs.is_empty() {
            return Err(type_error("wrapper __eq__() takes no keyword arguments"));
        }
        let [other] = args else {
            return Err(type_error(format!(
                "expected 1 argument, got {}",
                args.len()
            )));
        };
        return match ctx.dict_storage(*other) {
            Some(right) => Ok(boolean(ctx.checked_equal(receiver, right)?)),
            None => Ok(NOT_IMPLEMENTED),
        };
    }
    if name == "dict.__delitem__" {
        if !kwargs.is_empty() {
            return Err(type_error(
                "wrapper __delitem__() takes no keyword arguments",
            ));
        }
        let [key] = args else {
            return Err(type_error(format!(
                "expected 1 argument, got {}",
                args.len()
            )));
        };
        // Calling the base descriptor operates on the actual mapping and
        // does not redispatch to an override on the original receiver.
        ctx.delete_item(receiver, *key)?;
        return Ok(NONE);
    }
    if (set_method.is_some() || matches!(name, "dict.pop" | "dict.popitem" | "dict.__reversed__"))
        && !kwargs.is_empty()
    {
        return Err(type_error(format!("{name}() takes no keyword arguments")));
    }
    if !matches!(name, "dict.update" | "dict.__init__") {
        positional(name, kwargs)?;
    }
    if matches!(name, "dict.popitem" | "dict.__reversed__") && !args.is_empty() {
        return Err(type_error(format!(
            "{name}() takes no arguments ({} given)",
            args.len()
        )));
    }
    match name {
        "dict.__reversed__" => Ok(ctx.dict_iterator(object, DictViewKind::Keys, false, true)),
        "dict.popitem" => {
            let Some(last) = ctx.stored_entries(receiver).len().checked_sub(1) else {
                let message = ctx.text("popitem(): dictionary is empty");
                return Err(ctx.lookup_error("KeyError", message));
            };
            let (key, value) = ctx.heap.remove_table_entry(receiver, last)?;
            Ok(ctx.alloc(Value::Tuple(vec![key, value])))
        }
        "dict.copy" => {
            arity(name, args, 0, 0)?;
            copy_dict(ctx, object, receiver)
        }
        "dict.clear" => {
            arity(name, args, 0, 0)?;
            clear_container(ctx, receiver);
            Ok(NONE)
        }
        "dict.keys" | "dict.values" | "dict.items" => {
            arity(name, args, 0, 0)?;
            let kind = match name {
                "dict.keys" => DictViewKind::Keys,
                "dict.values" => DictViewKind::Values,
                _ => DictViewKind::Items,
            };
            Ok(ctx.alloc(Value::DictView {
                dict: object,
                kind,
                ordered: false,
            }))
        }
        "dict.pop" => {
            let (key, default) = match args {
                [key] => (*key, None),
                [key, default] => (*key, Some(*default)),
                [] => return Err(type_error("pop expected at least 1 argument, got 0")),
                _ => {
                    return Err(type_error(format!(
                        "pop expected at most 2 arguments, got {}",
                        args.len()
                    )));
                }
            };
            // An empty dictionary takes the missing-key path without hashing the key.
            if !entries(ctx, receiver)?.is_empty()
                && let Some(value) = remove_dict_item(ctx, receiver, key)?
            {
                return Ok(value);
            }
            default.ok_or_else(|| ctx.lookup_error("KeyError", key))
        }
        "dict.get" | "dict.setdefault" => {
            arity(name, args, 1, 2)?;
            let default = args.get(1).copied().unwrap_or(NONE);
            if name == "dict.setdefault" {
                return ctx.set_dictionary_key(receiver, args[0], default, None, false);
            }
            Ok(ctx.dictionary_value(receiver, args[0])?.unwrap_or(default))
        }
        "dict.__len__" => {
            arity(name, args, 0, 0)?;
            Ok(ctx.int(entries(ctx, receiver)?.len() as i64))
        }
        "dict.__iter__" => {
            arity(name, args, 0, 0)?;
            ctx.iterator(receiver)
        }
        "dict.__contains__" => {
            arity(name, args, 1, 1)?;
            Ok(boolean(ctx.contains(args[0], receiver)?))
        }
        "dict.__getitem__" => {
            arity(name, args, 1, 1)?;
            ctx.dictionary_item(object, args[0])
        }
        "dict.__setitem__" => {
            arity(name, args, 2, 2)?;
            ctx.put_item(receiver, args[0], args[1])?;
            Ok(NONE)
        }
        "dict.update" | "dict.__init__" => {
            arity(name, args, 0, 1)?;
            update(ctx, receiver, args.first().copied(), kwargs)?;
            Ok(NONE)
        }
        "frozenset.__contains__" | "set.__contains__" => {
            let [value] = args else {
                return Err(type_error(format!(
                    "{name}() takes exactly one argument ({} given)",
                    args.len()
                )));
            };
            Ok(boolean(set_member(ctx, receiver, *value, true)?))
        }
        "set.__reduce__" | "frozenset.__reduce__" => {
            arity(name, args, 0, 0)?;
            let entries = match ctx.heap.get(receiver) {
                Value::Set { entries, frozen } if *frozen == (name == "frozenset.__reduce__") => {
                    entries
                }
                _ => return Err(type_error(format!("invalid receiver for {name}"))),
            };
            let items = entries.iter().map(|&(key, _)| key).collect();
            let items = ctx.alloc(Value::List(items));
            let args = ctx.alloc(Value::Tuple(vec![items]));
            let class = ctx.exact_type(receiver)?;
            Ok(ctx.alloc(Value::Tuple(vec![class, args, NONE])))
        }
        "set.copy" | "frozenset.copy" => {
            if !args.is_empty() {
                return Err(type_error(format!(
                    "{name}() takes no arguments ({} given)",
                    args.len()
                )));
            }
            let frozen = name == "frozenset.copy";
            if frozen && object == receiver {
                return Ok(object);
            }
            let entries = ctx.stored_entries(receiver).try_clone()?;
            Ok(ctx.alloc(Value::Set { entries, frozen }))
        }
        "set.isdisjoint" | "frozenset.isdisjoint" => {
            let other = super::one_positional_arg(name, args, kwargs)?;
            if object == other {
                return Ok(boolean(ctx.stored_entries(receiver).is_empty()));
            }
            if matches!(ctx.heap.get(other), Value::Set { .. }) {
                let (source, probe) =
                    if ctx.stored_entries(other).len() > ctx.stored_entries(receiver).len() {
                        (receiver, other)
                    } else {
                        (other, receiver)
                    };
                return ctx.set_members_match(source, probe, false);
            }
            iterable_members_match(ctx, receiver, other, false)
        }
        "set.issuperset" | "frozenset.issuperset" | "set.issubset" | "frozenset.issubset" => {
            let [source] = args else {
                return Err(type_error(format!(
                    "{name}() takes exactly one argument ({} given)",
                    args.len()
                )));
            };
            if name.ends_with(".issubset") {
                return set_issubset(ctx, receiver, *source);
            }
            let other = ctx.scalar_value(*source);
            if matches!(ctx.heap.get(other), Value::Set { .. }) {
                return ctx.order_sets(">=", receiver, other);
            }
            iterable_members_match(ctx, receiver, *source, true)
        }
        "set.intersection" | "frozenset.intersection" => set_intersection(ctx, receiver, args),
        "set.difference" | "frozenset.difference" => ctx.set_difference(receiver, args),
        "set.intersection_update" => {
            // Later intersections can compare colliding keys in an intermediate set.
            if args.is_empty() || matches!(args, [other] if ctx.scalar_value(*other) == receiver) {
                return Ok(NONE);
            }
            let result = set_intersection(ctx, receiver, args)?;
            ctx.replace_set_entries(receiver, result);
            Ok(NONE)
        }
        "set.symmetric_difference" | "frozenset.symmetric_difference" => {
            let other = super::one_positional_arg(name, args, kwargs)?;
            ctx.set_xor(receiver, other, false)
        }
        "set.symmetric_difference_update" => {
            let other = super::one_positional_arg(name, args, kwargs)?;
            let storage = ctx.scalar_value(other);
            let source = if matches!(ctx.heap.get(storage), Value::Set { .. }) {
                storage
            } else if matches!(ctx.heap.get(other), Value::Dict(_)) {
                other
            } else {
                // Deduplicate the iterable completely before changing the receiver.
                let unique = ctx.alloc(Value::Set {
                    entries: Default::default(),
                    frozen: false,
                });
                update_set(ctx, unique, other)?;
                unique
            };
            ctx.set_binary("^", receiver, source, true)?;
            Ok(NONE)
        }
        "set.difference_update" => {
            for &source in args {
                ctx.set_difference_update(receiver, source)?;
            }
            Ok(NONE)
        }
        "set.pop" | "set.clear" => {
            if !args.is_empty() {
                return Err(type_error(format!(
                    "{name}() takes no arguments ({} given)",
                    args.len()
                )));
            }
            if name == "set.clear" {
                clear_container(ctx, receiver);
                return Ok(NONE);
            }
            let Some(last) = ctx.stored_entries(receiver).len().checked_sub(1) else {
                let message = ctx.text("pop from an empty set");
                return Err(ctx.lookup_error("KeyError", message));
            };
            Ok(ctx.heap.remove_table_entry(receiver, last)?.0)
        }
        "set.discard" | "set.remove" => {
            let key = super::one_positional_arg(name, args, kwargs)?;
            let index = match ctx.find_dictionary_key(receiver, key, None) {
                Ok((_, index)) => index,
                Err(error) if ctx.is_builtin_error(&error, "TypeError") => {
                    let Value::Set {
                        frozen: false,
                        entries,
                    } = ctx.heap.get(ctx.scalar_value(key))
                    else {
                        return Err(error);
                    };
                    // Retry a mutable set key with its equivalent frozenset's hash.
                    // Equality callbacks and KeyError still receive the original key.
                    let entries = entries.try_clone()?;
                    let frozen = ctx.alloc(Value::Set {
                        entries,
                        frozen: true,
                    });
                    let hash = ctx.dictionary_hash(frozen)?;
                    ctx.find_dictionary_key(receiver, key, Some(hash))?.1
                }
                Err(error) => return Err(error),
            };
            if let Some(index) = index {
                ctx.heap.remove_table_entry(receiver, index)?;
            } else if name == "set.remove" {
                return Err(ctx.lookup_error("KeyError", key));
            }
            Ok(NONE)
        }
        "set.add" => {
            let item = super::one_positional_arg(name, args, kwargs)?;
            set_add(ctx, receiver, item)?;
            Ok(NONE)
        }
        "set.union" | "frozenset.union" => {
            let result = ctx.alloc(Value::Set {
                entries: Default::default(),
                frozen: name == "frozenset.union",
            });
            update_set(ctx, result, receiver)?;
            for &source in args {
                if source != object {
                    update_set(ctx, result, source)?;
                }
            }
            Ok(result)
        }
        "set.update" => {
            for &input in args {
                update_set(ctx, receiver, input)?;
            }
            Ok(NONE)
        }
        _ => Err(type_error(format!("unsupported collection method {name}"))),
    }
}

/// Convert slice bounds before searching the underlying list or tuple.
fn sequence_index(
    ctx: &mut Context,
    owner: &str,
    receiver: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "{owner}.index() takes no keyword arguments"
        )));
    }
    let Some((&needle, bounds)) = args.split_first() else {
        return Err(type_error("index expected at least 1 argument, got 0"));
    };
    if bounds.len() > 2 {
        return Err(type_error(format!(
            "index expected at most 3 arguments, got {}",
            args.len()
        )));
    }
    let mut limits = [0, i64::MAX];
    for (limit, &bound) in limits.iter_mut().zip(bounds) {
        let n = ctx.index_value(bound)?.ok_or_else(|| {
            type_error("slice indices must be integers or have an __index__ method")
        })?;
        *limit = n.to_i64().unwrap_or_else(|| {
            if n.big().is_negative() {
                i64::MIN
            } else {
                i64::MAX
            }
        });
    }
    let (length, _) = ctx.sequence_at(receiver, 0).unwrap();
    let [start, stop] = limits.map(|i| if i < 0 { (length as i64 + i).max(0) } else { i });
    find_sequence_item(ctx, receiver, needle, start..stop)?
        .map(|i| ctx.int(i))
        .ok_or_else(|| Error::new("ValueError", format!("{owner}.index(x): x not in {owner}")))
}

/// Search by identity or rich equality, re-reading storage if a callback mutates a list.
fn find_sequence_item(
    ctx: &mut Context,
    receiver: Id,
    needle: Id,
    positions: std::ops::Range<i64>,
) -> Result<Option<i64>> {
    let mark = ctx.heap.temporary_mark();
    for i in positions {
        let Some((_, Some(item))) = ctx.sequence_at(receiver, i as usize) else {
            break;
        };
        if ctx.sequence_item_matches(item, needle, mark)? {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

/// Extend the list from a concrete sequence or consume the source iterator in order.
pub(in crate::dynamic) fn list_extend(ctx: &mut Context, receiver: Id, source: Id) -> Result<()> {
    // Self-extension snapshots the destination, including a list subclass's payload.
    let source = if ctx.scalar_value(source) == receiver {
        receiver
    } else {
        source
    };
    if let Value::List(items) | Value::Tuple(items) = ctx.heap.get(source) {
        let added = super::super::copy_ids(items)?;
        let items = list_items_mut(ctx, receiver)?;
        items
            .try_reserve(added.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        items.extend(added);
    } else {
        let iterator = ctx.iterator(source)?;
        ctx.heap.keep(iterator);
        let default = ctx.int(8);
        let hint = ctx.length_hint(source, Some(default))?;
        let hint = ctx.as_integer(hint)? as usize;
        let items = list_items_mut(ctx, receiver)?;
        if hint <= isize::MAX as usize - items.len() {
            items
                .try_reserve(hint)
                .map_err(|_| Error::new("MemoryError", ""))?;
        }
        ctx.extend_list_from_iterator(receiver, iterator)?;
    }
    Ok(())
}

/// Call a builtin slot on the receiver’s native payload, bypassing its override.
pub(super) fn builtin_slot(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (owner, method) = name.split_once('.').expect("builtin slot");
    let Some((&object, rest)) = args.split_first() else {
        return Err(type_error(format!(
            "descriptor '{method}' of '{owner}' object needs an argument"
        )));
    };
    if !ctx.builtin_slot_accepts(object, owner) {
        return Err(type_error(format!(
            "descriptor '{method}' requires a '{owner}' object but received a '{}'",
            ctx.builtin_type_name(object)
                .unwrap_or_else(|| ctx.type_name(object))
        )));
    }
    let storage = ctx.scalar_value(object);
    if name == "list.__setitem__" {
        if !kwargs.is_empty() {
            return Err(type_error(
                "wrapper __setitem__() takes no keyword arguments",
            ));
        }
        let [key, value] = rest else {
            return Err(type_error(format!(
                "__setitem__ expected 2 arguments, got {}",
                rest.len()
            )));
        };
        // A list assigned to its own slice copies its contents without invoking __iter__.
        let value = if object == *value && matches!(ctx.heap.get(*key), Value::Slice(_)) {
            storage
        } else {
            *value
        };
        ctx.put_item(storage, *key, value)?;
        return Ok(NONE);
    }
    let set_op = set_operator(method);
    let comparison = super::super::operators::comparison_op(method);
    if matches!(
        method,
        "__add__" | "__iadd__" | "__contains__" | "__pow__" | "__truediv__" | "__rtruediv__"
    ) || set_op.is_some()
        || comparison.is_some()
    {
        if !kwargs.is_empty() {
            return Err(type_error(format!(
                "wrapper {method}() takes no keyword arguments"
            )));
        }
        if method == "__pow__" {
            let (exponent, modulus) = match *rest {
                [exponent] => (exponent, NONE),
                [exponent, modulus] => (exponent, modulus),
                _ => {
                    return Err(type_error(format!(
                        "expected 1 or 2 arguments, got {}",
                        rest.len()
                    )));
                }
            };
            if !ctx.builtin_slot_accepts(exponent, "int")
                || (modulus != NONE && !ctx.builtin_slot_accepts(modulus, "int"))
            {
                return Ok(NOT_IMPLEMENTED);
            }
            // The slot operates on the integer payloads without reflected dispatch.
            let operands = [
                storage,
                ctx.scalar_value(exponent),
                ctx.scalar_value(modulus),
            ];
            return super::power(ctx, &operands, &[]);
        }
        let [value] = rest else {
            return Err(type_error(format!(
                "expected 1 argument, got {}",
                rest.len()
            )));
        };
        if method == "__contains__" {
            return Ok(boolean(ctx.contains(*value, storage)?));
        }
        if let Some(op) = comparison {
            return Ok(ctx
                .real_compare_slot(op, storage, *value)?
                .unwrap_or(NOT_IMPLEMENTED));
        }
        if owner == "float" {
            let other = ctx.scalar_value(*value);
            if !matches!(
                ctx.heap.get(other),
                Value::Float(_) | Value::Int(_) | Value::Bool(_)
            ) {
                return Ok(NOT_IMPLEMENTED);
            }
            // Calling a builtin wrapper uses the two native values, bypassing Python overrides.
            let (left, right) = if method == "__rtruediv__" {
                (other, storage)
            } else {
                (storage, other)
            };
            return ctx.binary("/", left, right);
        }
        if owner == "int" {
            return integer::add(ctx, storage, *value);
        }
        if method == "__add__" {
            return ctx.concat_list(storage, *value);
        }
        if let Some(op) = set_op {
            let other = ctx.scalar_value(*value);
            if !matches!(ctx.heap.get(other), Value::Set { .. }) {
                return Ok(NOT_IMPLEMENTED);
            }
            let (left, right) = if method.starts_with("__r") {
                (other, storage)
            } else {
                (storage, other)
            };
            let inplace = method.starts_with("__i");
            let result = ctx.set_binary(op, left, right, inplace)?;
            return Ok(if inplace { object } else { result });
        }
        list_extend(ctx, storage, *value)?;
        return Ok(object);
    }
    if matches!(
        method,
        "__repr__"
            | "__str__"
            | "__index__"
            | "__hash__"
            | "__bool__"
            | "__abs__"
            | "__neg__"
            | "__int__"
            | "__float__"
            | "__iter__"
            | "__len__"
    ) {
        if !kwargs.is_empty() {
            return Err(type_error(format!(
                "wrapper {method}() takes no keyword arguments"
            )));
        }
        if !rest.is_empty() {
            return Err(type_error(format!(
                "expected 0 arguments, got {}",
                rest.len()
            )));
        }
        if method == "__iter__" {
            return match owner {
                "list" => Ok(ctx.forward_iterator(object, "list_iterator")),
                "tuple" => Ok(ctx.forward_iterator(object, "tuple_iterator")),
                "set" | "frozenset" => Ok(ctx.set_iterator(object)),
                _ => ctx.iterator(storage),
            };
        }
        if method == "__len__" {
            let length = match ctx.heap.get(storage) {
                Value::List(items)
                | Value::Tuple(items)
                | Value::NamedTuple { values: items, .. } => items.len(),
                Value::Set { entries, .. } => entries.len(),
                _ => unreachable!("validated collection receiver"),
            };
            return Ok(ctx.int(length as i64));
        }
        if method == "__bool__" {
            return Ok(boolean(ctx.truth(storage)?));
        }
        if method == "__abs__" {
            return super::absolute_value(ctx, object);
        }
        if method == "__neg__" {
            return ctx.op("neg", storage, NONE, NONE);
        }
        if method == "__int__" {
            return integer::truncate_real(ctx, object);
        }
        if method == "__float__" {
            if let Value::Float(number) = *ctx.heap.get(storage) {
                return Ok(if storage == object {
                    object
                } else {
                    ctx.alloc(Value::Float(number))
                });
            }
            let number = crate::floats::int_to_float(&ctx.as_int(storage)?)?;
            return Ok(ctx.alloc(Value::Float(number)));
        }
        if method == "__hash__" {
            let hash = ctx.dictionary_hash(storage)?;
            return Ok(ctx.int(hash));
        }
        if method == "__index__" {
            return Ok(integer::exact_value(ctx, object));
        }
        if method == "__str__" {
            return ctx.unchanged_string(object, storage);
        }
        let text = match ctx.heap.get(storage) {
            Value::Bool(value) if owner == "int" => u8::from(*value).to_string(),
            Value::Int(value) => super::super::operators::rendered_integer(value)?.into_string(),
            Value::Bool(_)
            | Value::Float(_)
            | Value::Str(_)
            | Value::SurrogateStr(_)
            | Value::Bytes(_) => ctx.repr(storage),
            Value::ByteArray(bytes) => ctx.bytearray_repr(object, bytes),
            Value::List(_) | Value::Tuple(_) | Value::Set { .. } => {
                ctx.checked_container_slot_repr(object, storage)?
            }
            _ => {
                // Named tuple records store their fields directly rather than in a separate tuple.
                let fields = ctx
                    .sequence(object)
                    .expect("validated named tuple receiver");
                let tuple = ctx.alloc(Value::Tuple(fields));
                ctx.checked_container_slot_repr(object, tuple)?
            }
        };
        return Ok(ctx.text(text));
    }
    if !kwargs.is_empty() {
        return Err(type_error("list() takes no keyword arguments"));
    }
    if rest.len() > 1 {
        return Err(type_error(format!(
            "list expected at most 1 argument, got {}",
            rest.len()
        )));
    }
    let Value::List(items) = ctx.heap.get_mut(storage) else {
        unreachable!()
    };
    for item in take_list_contents(items) {
        ctx.heap.weak_edge_removed(item);
    }
    if let Some(&input) = rest.first() {
        let iter = ctx.iterator(input)?;
        let mark = ctx.heap.temporary_mark();
        ctx.heap.keep(storage);
        ctx.heap.keep(iter);
        let extend: Result<()> = (|| {
            while let Some(item) = ctx.next_item(iter)? {
                let Value::List(items) = ctx.heap.get_mut(storage) else {
                    unreachable!()
                };
                items
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                items.push(item);
            }
            Ok(())
        })();
        ctx.heap.release_temporaries_since(mark);
        extend?;
    }
    Ok(NONE)
}

/// Convert a list method's positional index before reading the list's current length.
fn list_position_arg(ctx: &mut Context, value: Id) -> Result<i64> {
    ctx.index_integer(value)?.to_i64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

fn list_pop(ctx: &mut Context, receiver: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error("list.pop() takes no keyword arguments"));
    }
    let index = match args {
        [] => -1,
        [index] => list_position_arg(ctx, *index)?,
        _ => {
            return Err(type_error(format!(
                "pop expected at most 1 argument, got {}",
                args.len()
            )));
        }
    };
    // __index__ can mutate the list. Its resulting length determines which item to remove.
    let Value::List(items) = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    if items.is_empty() {
        return Err(Error::new("IndexError", "pop from empty list"));
    }
    let len = items.len() as i64;
    let index = if index < 0 { len + index } else { index };
    if !(0..len).contains(&index) {
        return Err(Error::new("IndexError", "pop index out of range"));
    }
    let value = items.remove(index as usize);
    ctx.heap.weak_edge_removed(value);
    Ok(value)
}
