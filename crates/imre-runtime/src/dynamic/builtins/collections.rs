use super::super::calls::MappingOperation;
use super::super::value::DictViewKind;
use super::*;
use num_traits::Signed;

/// Clear container storage, then finalize any buffered writer whose last owner was removed.
pub(in super::super) fn clear_container(ctx: &mut Context, receiver: Id) {
    let owns_writer = |&id: &Id| ctx.heap.could_own_writer(id, true);
    let released = ctx.heap.has_unbound_writer(NONE)
        && match ctx.heap.get(receiver) {
            Value::List(items) => items.iter().any(owns_writer),
            Value::Dict(entries) | Value::DefaultDict { entries, .. } => entries
                .iter()
                .any(|(key, value)| owns_writer(key) || owns_writer(value)),
            Value::Deque(ring) => ring.items.iter().any(owns_writer),
            _ => unreachable!("clearable container"),
        };
    match ctx.heap.get_mut(receiver) {
        Value::List(items) => items.clear(),
        Value::Dict(entries) | Value::DefaultDict { entries, .. } => {
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
    loop {
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(winner);
        ctx.heap.keep(winning_key);
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
    let values = ctx.items(*iterable)?;
    let list = ctx.alloc(Value::List(values));
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
    let mut values = std::mem::take(live);
    for &value in &values {
        ctx.heap.keep(value);
    }
    ctx.sorting_lists.push(receiver);
    let result = sort_list_items(ctx, &mut values, key, reverse);
    ctx.sorting_lists.pop();
    let Value::List(live) = ctx.heap.get_mut(receiver) else {
        unreachable!()
    };
    // An append allocates storage even if the callback subsequently pops the item.
    let modified = live.capacity() != 0;
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
        for &value in values.iter() {
            let sort_key = ctx.call(key, vec![value], vec![])?;
            ctx.heap.keep(sort_key);
            keyed.push((sort_key, value));
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

fn sort_values<T: Copy>(
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
        && let Some(entries) = entries.clone_dense()
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
                    if ctx.error_matches(&error.kind, class) {
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
        Value::Deque(_)
        | Value::List(_)
        | Value::ByteArray(_)
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
        Value::Instance { class, fields, .. } => ctx.namedtuple_fields(*class).and_then(|names| {
            names
                .iter()
                .find_map(|name| unhashable_part(ctx, fields[name]))
        }),
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
    let (Value::Dict(entries) | Value::DefaultDict { entries, .. }) = ctx.heap.get_mut(dict) else {
        unreachable!("dictionary payload")
    };
    let (key, value) = entries.remove(index);
    ctx.heap.weak_edge_removed(key);
    ctx.heap.weak_edge_removed(value);
    Ok(Some(value))
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
            entries: entries.clone(),
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
    if matches!(ctx.heap.get(storage), Value::Set { .. }) {
        return merge_set(ctx, target, storage);
    }
    let iterator = ctx.iterator(source)?;
    while let Some(item) = ctx.next_item(iterator)? {
        set_add(ctx, target, item)?;
    }
    Ok(())
}

fn merge_set(ctx: &mut Context, target: Id, source: Id) -> Result<()> {
    if target == source {
        return Ok(());
    }
    if matches!(ctx.heap.get(target), Value::Set { entries, .. } if entries.is_empty()) {
        let Value::Set { entries, .. } = ctx.heap.get(source) else {
            unreachable!()
        };
        let copy = entries.clone();
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
    let items = args
        .first()
        .map(|&it| ctx.items(it))
        .transpose()?
        .unwrap_or_default();
    match name {
        "list" => Ok(ctx.alloc(Value::List(items))),
        "tuple" => Ok(ctx.alloc(Value::Tuple(items))),
        _ => Err(type_error(format!("unknown collection {name}"))),
    }
}

/// Build a fresh mapping through the bound class, sharing the supplied value.
fn fromkeys(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some((&class, args)) = args.split_first() else {
        return Err(type_error("dict.fromkeys needs a class"));
    };
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
        ctx.heap.keep(iterator);
        while let Some(key) = ctx.next_item(iterator)? {
            ctx.put_item(result, key, value)?;
        }
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
            | "list.__iadd__"
            | "list.__init__"
            | "list.__repr__"
            | "tuple.__repr__"
            | "set.__repr__"
            | "frozenset.__repr__"
    ) {
        return builtin_slot(ctx, name, args, kwargs);
    }
    if name == "dict.fromkeys" {
        return fromkeys(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("list.")
        && LIST_METHODS.contains(&method)
    {
        let Some((&receiver, args)) = args.split_first() else {
            return Err(type_error(format!(
                "unbound method {name}() needs an argument"
            )));
        };
        let receiver = ctx.scalar_value(receiver);
        if !matches!(ctx.heap.get(receiver), Value::List(_)) {
            return Err(type_error(format!(
                "descriptor '{method}' for 'list' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            )));
        }
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
            "append" => {
                let value = one_arg()?;
                let items = list_items_mut(ctx, receiver)?;
                items
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                items.push(value);
                Ok(NONE)
            }
            "index" => list_index(ctx, receiver, args, kwargs),
            "extend" => {
                list_extend(ctx, receiver, one_arg()?)?;
                Ok(NONE)
            }
            "count" => {
                let value = one_arg()?;
                let mut count = 0;
                let mut start = 0;
                while let Some(i) = find_list_item(ctx, receiver, value, start..i64::MAX)? {
                    count += 1;
                    start = i + 1;
                }
                Ok(ctx.int(count))
            }
            "remove" => {
                let i = find_list_item(ctx, receiver, one_arg()?, 0..i64::MAX)?
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
    let slot = matches!(
        name,
        "dict.__init__"
            | "dict.__repr__"
            | "dict.__len__"
            | "dict.__iter__"
            | "dict.__setitem__"
            | "dict.__delitem__"
    );
    let (&object, args) = args
        .split_first()
        .ok_or_else(|| match name.strip_prefix("dict.") {
            Some(method) if slot => type_error(format!(
                "descriptor '{method}' of 'dict' object needs an argument"
            )),
            Some(_) => type_error(format!("unbound method {name}() needs an argument")),
            None => type_error("missing collection receiver"),
        })?;
    let receiver = if let Some(method) = name.strip_prefix("dict.") {
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
    if matches!(
        name,
        "dict.pop"
            | "set.issuperset"
            | "frozenset.issuperset"
            | "set.discard"
            | "frozenset.copy"
            | "set.copy"
            | "frozenset.__contains__"
            | "set.__contains__"
    ) && !kwargs.is_empty()
    {
        return Err(type_error(format!("{name}() takes no keyword arguments")));
    }
    if !matches!(name, "dict.update" | "dict.__init__") {
        positional(name, kwargs)?;
    }
    match name {
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
            let (_, index) = ctx.find_dictionary_key(receiver, args[0], None)?;
            Ok(index.map_or(default, |i| {
                ctx.dict_entries(receiver).expect("dictionary")[i].1
            }))
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
        "frozenset.copy" => {
            arity(name, args, 0, 0)?;
            Ok(receiver)
        }
        "set.copy" => {
            arity(name, args, 0, 0)?;
            let Value::Set {
                entries,
                frozen: false,
            } = ctx.heap.get(receiver)
            else {
                return Err(type_error("descriptor 'copy' requires a 'set' object"));
            };
            let entries = entries.clone();
            Ok(ctx.alloc(Value::Set {
                entries,
                frozen: false,
            }))
        }
        "set.issuperset" | "frozenset.issuperset" => {
            let [source] = args else {
                return Err(type_error(format!(
                    "{name}() takes exactly one argument ({} given)",
                    args.len()
                )));
            };
            let iterator = ctx.iterator(*source)?;
            while let Some(item) = ctx.next_item(iterator)? {
                if !set_member(ctx, receiver, item, false)? {
                    return Ok(FALSE);
                }
            }
            Ok(TRUE)
        }
        "set.discard" => {
            arity(name, args, 1, 1)?;
            if !matches!(ctx.heap.get(receiver), Value::Set { frozen: false, .. }) {
                return Err(type_error("descriptor 'discard' requires a 'set' object"));
            }
            let key = if let Value::Set {
                entries,
                frozen: false,
            } = ctx.heap.get(args[0])
            {
                ctx.alloc(Value::Set {
                    entries: entries.clone(),
                    frozen: true,
                })
            } else {
                args[0]
            };
            if let Some(index) = ctx.find_dictionary_key(receiver, key, None)?.1 {
                let Value::Set { entries, .. } = ctx.heap.get_mut(receiver) else {
                    unreachable!()
                };
                let (key, value) = entries.remove(index);
                ctx.heap.weak_edge_removed(key);
                ctx.heap.weak_edge_removed(value);
            }
            Ok(NONE)
        }
        "set.add" => {
            arity(name, args, 1, 1)?;
            set_add(ctx, receiver, args[0])?;
            Ok(NONE)
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

/// Compare against the live list; __index__ and __eq__ can both change its contents.
fn list_index(ctx: &mut Context, receiver: Id, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error("list.index() takes no keyword arguments"));
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
    let Value::List(items) = ctx.heap.get(receiver) else {
        unreachable!()
    };
    let [start, stop] = limits.map(|i| {
        if i < 0 {
            (items.len() as i64 + i).max(0)
        } else {
            i
        }
    });
    find_list_item(ctx, receiver, needle, start..stop)?
        .map(|i| ctx.int(i))
        .ok_or_else(|| Error::new("ValueError", "list.index(x): x not in list"))
}

/// Locate the next match in a live list. Equality callbacks may resize or clear it.
fn find_list_item(
    ctx: &mut Context,
    receiver: Id,
    needle: Id,
    positions: std::ops::Range<i64>,
) -> Result<Option<i64>> {
    for i in positions {
        let Value::List(items) = ctx.heap.get(receiver) else {
            unreachable!()
        };
        let Some(&item) = items.get(i as usize) else {
            break;
        };
        if item != needle {
            let mark = ctx.heap.temporary_mark();
            ctx.heap.keep(item);
            let matched = ctx
                .binary("==", item, needle)
                .and_then(|equal| ctx.truth(equal));
            ctx.heap.release_temporaries_since(mark);
            if !matched? {
                continue;
            }
        }
        return Ok(Some(i));
    }
    Ok(None)
}

/// Extend the list from a concrete sequence or consume the source iterator in order.
fn list_extend(ctx: &mut Context, receiver: Id, source: Id) -> Result<()> {
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
        while let Some(item) = ctx.next_item(iterator)? {
            let items = list_items_mut(ctx, receiver)?;
            items
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            items.push(item);
        }
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
            ctx.type_name(object)
        )));
    }
    let storage = ctx.scalar_value(object);
    if matches!(method, "__add__" | "__iadd__") {
        if !kwargs.is_empty() {
            return Err(type_error(format!(
                "wrapper {method}() takes no keyword arguments"
            )));
        }
        let [value] = rest else {
            return Err(type_error(format!(
                "expected 1 argument, got {}",
                rest.len()
            )));
        };
        if method == "__add__" {
            return ctx.concat_list(storage, *value);
        }
        list_extend(ctx, storage, *value)?;
        return Ok(object);
    }
    if method == "__repr__" {
        if !kwargs.is_empty() {
            return Err(type_error("wrapper __repr__() takes no keyword arguments"));
        }
        if !rest.is_empty() {
            return Err(type_error(format!(
                "expected 0 arguments, got {}",
                rest.len()
            )));
        }
        let text = match ctx.heap.get(storage) {
            Value::Str(_) | Value::Bytes(_) => ctx.repr(storage),
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
    let old = std::mem::take(items);
    for item in old {
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
