use super::super::value::DictViewKind;
use super::*;

pub(super) fn entries(ctx: &Context, dict: Id) -> Result<&[(Id, Id)]> {
    ctx.dict_entries(dict)
        .ok_or_else(|| type_error("expected a dictionary"))
}

pub(in super::super) fn view_items(
    ctx: &mut Context,
    dict: Id,
    kind: DictViewKind,
) -> Result<Vec<Id>> {
    Ok(entries(ctx, dict)?
        .to_vec()
        .into_iter()
        .map(|(key, value)| match kind {
            DictViewKind::Keys => key,
            DictViewKind::Values => value,
            DictViewKind::Items => ctx.alloc(Value::Tuple(vec![key, value])),
        })
        .collect())
}

fn update(ctx: &mut Context, dict: Id, input: Option<Id>, kwargs: &[(String, Id)]) -> Result<()> {
    if let Some(input) = input
        && !ctx.for_each_mapping_item(input, |ctx, key, value| ctx.put_item(dict, key, value))?
    {
        // Unpacking a pair may mutate the input list or consume the same iterator.
        let snapshot = match ctx.heap.get(input) {
            Value::List(_) | Value::Tuple(_) | Value::Iterator { .. } => Vec::new(),
            _ => ctx.items(input)?,
        };
        for index in 0.. {
            let pair = match ctx.heap.get(input) {
                Value::List(pairs) | Value::Tuple(pairs) => pairs.get(index).copied(),
                Value::Iterator { .. } => ctx.next_item(input)?,
                _ => snapshot.get(index).copied(),
            };
            let Some(pair) = pair else { break };
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
    match ctx.heap.get(value) {
        Value::List(_)
        | Value::ByteArray(_)
        | Value::Context(_)
        | Value::ContextToken { .. }
        | Value::Set(_)
        | Value::Dict(_)
        | Value::DefaultDict { .. }
        | Value::Instance {
            dictionary: Some(_),
            ..
        }
        | Value::DictView {
            kind: DictViewKind::Keys | DictViewKind::Items,
            ..
        } => false,
        Value::Tuple(values) | Value::NamedTuple { values, .. } => {
            values.iter().all(|&x| hashable(ctx, x))
        }
        Value::Instance { class, fields, .. } => ctx
            .namedtuple_fields(*class)
            .is_none_or(|names| names.iter().all(|name| hashable(ctx, fields[name]))),
        _ => true,
    }
}

fn set_add(ctx: &mut Context, set: Id, value: Id) -> Result<()> {
    if !hashable(ctx, value) {
        return Err(type_error("unhashable set element"));
    }
    let Value::Set(items) = ctx.heap.get(set) else {
        return Err(type_error("expected set"));
    };
    if !items.iter().any(|&item| ctx.equal(item, value))
        && let Value::Set(items) = ctx.heap.get_mut(set)
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
        "set" => {
            let set = ctx.alloc(Value::Set(Vec::new()));
            for item in items {
                set_add(ctx, set, item)?;
            }
            Ok(set)
        }
        _ => Err(type_error(format!("unknown collection {name}"))),
    }
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
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
        "dict.get" | "dict.setdefault" => {
            arity(name, args, 1, 2)?;
            if !hashable(ctx, args[0]) {
                return Err(type_error("unhashable dictionary key"));
            }
            if let Some(&(_, value)) = entries(ctx, receiver)?
                .iter()
                .find(|(key, _)| ctx.equal(*key, args[0]))
            {
                return Ok(value);
            }
            let default = args.get(1).copied().unwrap_or(NONE);
            if name == "dict.setdefault" {
                ctx.put_item(receiver, args[0], default)?;
            }
            Ok(default)
        }
        "dict.__len__" => {
            arity(name, args, 0, 0)?;
            Ok(ctx.int(entries(ctx, receiver)?.len() as i64))
        }
        "dict.__iter__" => {
            arity(name, args, 0, 0)?;
            let values = ctx.items(receiver)?;
            Ok(ctx.alloc(Value::Iterator { values, offset: 0 }))
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
