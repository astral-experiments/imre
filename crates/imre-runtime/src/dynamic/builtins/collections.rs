use super::super::value::DictViewKind;
use super::*;

pub(super) fn entries(ctx: &Context, dict: Id) -> Result<&[(Id, Id)]> {
    ctx.dict_entries(dict)
        .ok_or_else(|| type_error("expected a dictionary"))
}

fn update(ctx: &mut Context, dict: Id, input: Option<Id>, kwargs: &[(String, Id)]) -> Result<()> {
    if let Some(input) = input
        && !ctx.for_each_mapping_item(input, dict, |ctx, key, value, hash| {
            ctx.set_dictionary_key(dict, key, value, hash, true)
                .map(|_| ())
        })?
    {
        let iterator = ctx.iterator(input)?;
        for index in 0.. {
            let Some(pair) = ctx.next_item(iterator)? else {
                break;
            };
            let pair = ctx.items(pair)?;
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
    match ctx.heap.get(value) {
        Value::List(_)
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
    let Some(index) = ctx.find_dictionary_key(dict, key, None)?.1 else {
        return Ok(None);
    };
    let (Value::Dict(entries) | Value::DefaultDict { entries, .. }) = ctx.heap.get_mut(dict) else {
        unreachable!("dictionary payload")
    };
    Ok(Some(entries.remove(index).1))
}

pub(in super::super) fn set_member(
    ctx: &Context,
    set: Id,
    value: Id,
    allow_mutable_key: bool,
) -> Result<bool> {
    let Value::Set { items, .. } = ctx.heap.get(set) else {
        return Err(type_error("expected set"));
    };
    // Membership accepts a mutable set as an equivalent frozen key. Insertion and
    // issuperset(iterable) require each element itself to be hashable.
    if !(allow_mutable_key && matches!(ctx.heap.get(value), Value::Set { frozen: false, .. }))
        && let Some(part) = unhashable_part(ctx, value)
    {
        return Err(type_error(format!(
            "cannot use '{}' as a set element (unhashable type: '{}')",
            ctx.type_name(value),
            ctx.type_name(part),
        )));
    }
    Ok(items.iter().any(|&entry| ctx.equal(entry, value)))
}

pub(in super::super) fn set_add(ctx: &mut Context, set: Id, value: Id) -> Result<()> {
    if !set_member(ctx, set, value, false)?
        && let Value::Set { items, .. } = ctx.heap.get_mut(set)
    {
        items.push(value);
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
            items: Vec::new(),
            frozen,
        });
        if let [source] = args {
            let iterator = ctx.iterator(*source)?;
            while let Some(item) = ctx.next_item(iterator)? {
                set_add(ctx, set, item)?;
            }
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

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "list.pop" {
        return list_pop(ctx, args, kwargs);
    }
    let slot = matches!(
        name,
        "dict.__init__" | "dict.__len__" | "dict.__iter__" | "dict.__setitem__"
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
    if matches!(
        name,
        "dict.pop" | "set.issuperset" | "frozenset.issuperset" | "frozenset.copy"
    ) && !kwargs.is_empty()
    {
        return Err(type_error(format!("{name}() takes no keyword arguments")));
    }
    if !matches!(name, "dict.update" | "dict.__init__") {
        positional(name, kwargs)?;
    }
    match name {
        "dict.keys" | "dict.values" | "dict.items" => {
            arity(name, args, 0, 0)?;
            let kind = match name {
                "dict.keys" => DictViewKind::Keys,
                "dict.values" => DictViewKind::Values,
                _ => DictViewKind::Items,
            };
            Ok(ctx.alloc(Value::DictView { dict: object, kind }))
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
            match ctx.get_item(receiver, args[0]) {
                Err(error) if error.kind == "KeyError" => {
                    if let Some(hook) = ctx.bound_slot(object, "__missing__")? {
                        ctx.call(hook, vec![args[0]], Vec::new())
                    } else {
                        Err(error)
                    }
                }
                result => result,
            }
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
        "list.append" | "list.extend" => {
            arity(name, args, 1, 1)?;
            let added = if name == "list.extend" {
                ctx.items(args[0])?
            } else {
                vec![args[0]]
            };
            let Value::List(items) = ctx.heap.get_mut(receiver) else {
                return Err(type_error("expected list"));
            };
            items.extend(added);
            Ok(NONE)
        }
        "list.insert" => {
            arity(name, args, 2, 2)?;
            let index = ctx.as_integer(args[0])?;
            let Value::List(items) = ctx.heap.get_mut(receiver) else {
                return Err(type_error("expected list"));
            };
            let len = items.len() as i64;
            let index = if index < 0 {
                (len + index).max(0)
            } else {
                index.min(len)
            };
            items.insert(index as usize, args[1]);
            Ok(NONE)
        }
        "frozenset.__contains__" | "set.__contains__" => {
            arity(name, args, 1, 1)?;
            Ok(boolean(set_member(ctx, receiver, args[0], true)?))
        }
        "frozenset.copy" => {
            arity(name, args, 0, 0)?;
            Ok(receiver)
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
        "set.add" => {
            arity(name, args, 1, 1)?;
            set_add(ctx, receiver, args[0])?;
            Ok(NONE)
        }
        "set.update" => {
            for &input in args {
                for item in ctx.items(input)? {
                    set_add(ctx, receiver, item)?;
                }
            }
            Ok(NONE)
        }
        _ => Err(type_error(format!("unsupported collection method {name}"))),
    }
}

fn list_pop(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some((&receiver, args)) = args.split_first() else {
        return Err(type_error("unbound method list.pop() needs an argument"));
    };
    if !matches!(ctx.heap.get(receiver), Value::List(_)) {
        return Err(type_error(format!(
            "descriptor 'pop' for 'list' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        )));
    }
    if !kwargs.is_empty() {
        return Err(type_error("list.pop() takes no keyword arguments"));
    }
    let index = match args {
        [] => -1,
        [index] => ctx.index_integer(*index)?.to_i64().ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?,
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
    Ok(items.remove(index as usize))
}
