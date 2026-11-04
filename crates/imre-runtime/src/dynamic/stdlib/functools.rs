use crate::dynamic::{
    Context,
    value::{Error, Id, NONE, Result, Value},
};

pub(in crate::dynamic) const PARTIAL_CLASS: &str = "functools.partial";
pub(in crate::dynamic) const LRU_CACHE_CLASS: &str = "functools._lru_cache_wrapper";
pub(in crate::dynamic) const COPY_METHODS: &[&str] = &["__reduce__", "__copy__", "__deepcopy__"];

pub(super) fn placeholder(ctx: &mut Context) -> Id {
    let key = "functools.Placeholder";
    if let Some(&value) = ctx.builtins.get(key) {
        return value;
    }
    let value = ctx.alloc(Value::PartialPlaceholder);
    ctx.builtins.insert(key.to_owned(), value);
    value
}

pub(super) fn partial_setstate(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (receiver, rest) =
        super::builtins::method_receiver_args(ctx, PARTIAL_CLASS, "__setstate__", args, kwargs)?;
    let state = super::builtins::one_positional_arg("partial.__setstate__", rest, kwargs)?;
    let invalid = || Error::new("TypeError", "invalid partial state");
    let state = ctx.tuple_items(state)?.ok_or_else(invalid)?;
    let &[function, args, keywords, namespace] = state.as_slice() else {
        return Err(invalid());
    };
    let raw_args = ctx.tuple_items(args)?.ok_or_else(invalid)?;
    if !ctx.callable(function)
        || (keywords != NONE && ctx.dict_storage(keywords).is_none())
        || (namespace != NONE && ctx.dict_storage(namespace).is_none())
    {
        return Err(invalid());
    }
    if raw_args
        .last()
        .is_some_and(|&arg| matches!(ctx.heap.get(arg), Value::PartialPlaceholder))
    {
        return Err(Error::new(
            "TypeError",
            "trailing Placeholders are not allowed",
        ));
    }

    // Conversion may call Python code. Finish both conversions before replacing any state.
    let tuple = ctx.builtin("tuple");
    let args = ctx.call(tuple, vec![args], Vec::new())?;
    let keywords = if keywords == NONE {
        ctx.dict(Vec::new())
    } else if matches!(ctx.heap.get(keywords), Value::Dict(_)) {
        keywords
    } else {
        let storage = ctx
            .dict_storage(keywords)
            .expect("validated keyword dictionary");
        super::builtins::copy_dict(ctx, keywords, storage)?
    };
    let storage = ctx.scalar_value(receiver);
    let namespace = (namespace != NONE).then_some(namespace);
    let dictionary = if let Value::Instance { class, .. } = *ctx.heap.get(receiver) {
        ctx.replace_instance_dictionary(receiver, class, namespace);
        None
    } else {
        namespace
    };
    let previous = std::mem::replace(
        ctx.heap.get_mut(storage),
        Value::Partial {
            function,
            args,
            keywords,
            dictionary,
        },
    );
    let Value::Partial {
        function,
        args,
        keywords,
        dictionary,
    } = previous
    else {
        unreachable!("validated partial receiver")
    };
    for old in [Some(function), Some(args), Some(keywords), dictionary]
        .into_iter()
        .flatten()
    {
        ctx.heap.weak_edge_removed(old);
    }
    Ok(NONE)
}

/// Keep the original fields live while calling user formatting hooks, as in CPython's partial_repr.
pub(super) fn partial_repr(ctx: &mut Context, object: Id) -> Result<Id> {
    if ctx.repr_ancestors.contains(&object) {
        return Ok(ctx.text("..."));
    }
    let Value::Partial {
        function,
        args,
        keywords,
        ..
    } = *ctx.heap.get(ctx.scalar_value(object))
    else {
        return Err(Error::new("TypeError", "expected a partial object"));
    };
    ctx.repr_ancestors
        .try_reserve(1)
        .map_err(|_| Error::new("MemoryError", ""))?;
    ctx.heap.reserve_temporaries(4)?;
    let roots = ctx.heap.temporary_mark();
    for value in [object, function, args, keywords] {
        ctx.heap.keep(value);
    }
    ctx.repr_ancestors.push(object);
    let result = ctx.with_slot_depth(
        "maximum recursion depth exceeded while getting the repr of an object",
        |ctx| partial_repr_fields(ctx, object, function, args, keywords),
    );
    ctx.repr_ancestors.pop();
    ctx.heap.release_temporaries_since(roots);
    match &result {
        Ok(value) => ctx.heap.keep(*value),
        Err(error) => {
            for value in error.roots() {
                ctx.heap.keep(value);
            }
        }
    }
    result
}

fn partial_repr_fields(
    ctx: &mut Context,
    object: Id,
    function: Id,
    args: Id,
    keywords: Id,
) -> Result<Id> {
    fn append(ctx: &mut Context, buffer: &mut Vec<u32>, op: &str, value: Id) -> Result<()> {
        let text = super::builtins::call(ctx, op, &[value], &[])?;
        let chars = ctx.string_codepoints(text)?;
        buffer
            .try_reserve(chars.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        buffer.extend_from_slice(&chars);
        Ok(())
    }

    let mut suffix = Vec::new();
    let step = ctx.heap.temporary_mark();
    let invalid = || Error::new("TypeError", "invalid partial state");
    let args = ctx.tuple_items(args)?.ok_or_else(invalid)?;
    let keywords = ctx.dict_storage(keywords).ok_or_else(invalid)?;
    for arg in args {
        suffix.extend([u32::from(','), u32::from(' ')]);
        append(ctx, &mut suffix, "repr", arg)?;
        ctx.heap.release_temporaries_since(step);
    }
    let mut position = 0;
    while let Some((index, key, value)) = ctx.stored_entries(keywords).at(position) {
        position = index + 1;
        // The key's __str__ may remove the value from the dictionary.
        ctx.heap.keep(value);
        suffix.extend([u32::from(','), u32::from(' ')]);
        append(ctx, &mut suffix, "str", key)?;
        suffix.push(u32::from('='));
        append(ctx, &mut suffix, "repr", value)?;
        ctx.heap.release_temporaries_since(step);
    }
    let class = ctx.exact_type(object)?;
    let module = ctx
        .own_class_attr(class, "__module__")
        .ok_or_else(|| Error::new("AttributeError", "__module__"))?;
    let qualname = match ctx.own_class_attr(class, "__qualname__") {
        Some(name) => name,
        None => ctx.text(ctx.class_qualname(class).to_owned()),
    };
    // Formatting either name can replace both attributes on the class.
    ctx.heap.keep(module);
    ctx.heap.keep(qualname);
    let mut output = Vec::new();
    append(ctx, &mut output, "str", module)?;
    output.push(u32::from('.'));
    append(ctx, &mut output, "str", qualname)?;
    output.push(u32::from('('));
    append(ctx, &mut output, "repr", function)?;
    output.extend(suffix);
    output.push(u32::from(')'));
    Ok(ctx.text_codepoints(output))
}

/// Argument Clinic accepts either one positional argument or its named equivalent.
pub(in crate::dynamic) fn key_argument(
    name: &str,
    param: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [value] = super::builtins::bind_builtin_arguments(name, args, kwargs, [param], 1)?;
    Ok(value.expect("required argument checked by binder"))
}

pub(in crate::dynamic) fn key_comparison(
    ctx: &mut Context,
    slot: &str,
    comparator: Id,
    object: Option<Id>,
    other: Id,
) -> Result<Option<Id>> {
    let op = match slot {
        "__lt__" => "<",
        "__le__" => "<=",
        "__eq__" => "==",
        "__ne__" => "!=",
        "__ge__" => ">=",
        "__gt__" => ">",
        _ => return Ok(None),
    };
    let Value::KeyWrapper { object: rhs, .. } = *ctx.heap.get(other) else {
        return Err(Error::new("TypeError", "other argument must be K instance"));
    };
    let (Some(left), Some(right)) = (object, rhs) else {
        return Err(Error::new("AttributeError", "object"));
    };
    let result = ctx.call(comparator, vec![left, right], Vec::new())?;
    let zero = ctx.int(0);
    ctx.binary(op, result, zero).map(Some)
}

pub(super) fn key_method(
    ctx: &mut Context,
    slot: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("descriptor '{slot}' of 'functools.KeyWrapper' object needs an argument"),
        ));
    };
    let Value::KeyWrapper { comparator, object } = *ctx.heap.get(receiver) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{slot}' requires a 'functools.KeyWrapper' object but received a '{}'",
                ctx.type_name(receiver)
            ),
        ));
    };
    if slot == "__call__" {
        let value = key_argument("K", "obj", rest, kwargs)?;
        return Ok(ctx.alloc(Value::KeyWrapper {
            comparator,
            object: Some(value),
        }));
    }
    let &[other] = rest else {
        return Err(Error::new(
            "TypeError",
            format!("expected 1 argument, got {}", rest.len()),
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("wrapper {slot}() takes no keyword arguments"),
        ));
    }
    Ok(key_comparison(ctx, slot, comparator, object, other)?.expect("registered comparison slot"))
}

/// Reduce consumes one item between calls, leaving the iterator at the failure point.
pub(super) fn reduce(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let count = args.len() + kwargs.len();
    if count > 3 {
        let kind = if args.is_empty() { "keyword " } else { "" };
        return Err(Error::new(
            "TypeError",
            format!("reduce() takes at most 3 {kind}arguments ({count} given)"),
        ));
    }
    let [function, iterable, rest @ ..] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "reduce() takes at least 2 positional arguments ({} given)",
                args.len()
            ),
        ));
    };
    let [initial] =
        super::builtins::bind_builtin_arguments("reduce", rest, kwargs, ["initial"], 0)?;
    let roots = ctx.heap.temporary_mark();
    let result = reduce_iterable(ctx, *function, *iterable, initial);
    ctx.heap.release_temporaries_since(roots);
    match &result {
        Ok(value) => ctx.heap.keep(*value),
        Err(error) => {
            for value in error.roots() {
                ctx.heap.keep(value);
            }
        }
    }
    result
}

fn reduce_iterable(
    ctx: &mut Context,
    function: Id,
    iterable: Id,
    initial: Option<Id>,
) -> Result<Id> {
    let iterator = ctx.iterator(iterable).map_err(|error| {
        let type_error = ctx.builtin("TypeError");
        if ctx.raised_error_matches(&error, type_error) {
            Error::new("TypeError", "reduce() arg 2 must support iteration")
        } else {
            error
        }
    })?;
    ctx.heap.keep(iterator);
    let mark = ctx.heap.temporary_mark();
    let mut accumulator = match initial {
        Some(value) => value,
        None => ctx.next_item(iterator)?.ok_or_else(|| {
            Error::new(
                "TypeError",
                "reduce() of empty iterable with no initial value",
            )
        })?,
    };
    ctx.heap.keep(accumulator);
    while let Some(item) = ctx.next_item(iterator)? {
        // CPython keeps the last two callback operands alive until the next item arrives.
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(accumulator);
        ctx.heap.keep(item);
        accumulator = ctx.call(function, vec![accumulator, item], Vec::new())?;
        ctx.heap.keep(accumulator);
        ctx.heap.reserve_slot_or_request_collection();
        ctx.collect_garbage();
    }
    Ok(accumulator)
}

pub(super) fn make_cache(ctx: &mut Context, function: Id, maxsize: Id, typed: Id) -> Result<Id> {
    let typed_keys = ctx.truth(typed)?;
    if !ctx.callable(function) {
        return Err(Error::new(
            "TypeError",
            "the first argument must be callable",
        ));
    }
    let capacity = if maxsize == NONE {
        None
    } else {
        let n = ctx
            .as_int(maxsize)?
            .to_i64()
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    format!(
                        "cannot fit '{}' into an index-sized integer",
                        ctx.type_name(maxsize)
                    ),
                )
            })?;
        Some(n.max(0) as usize)
    };
    Ok(ctx.new_cache(function, capacity, typed_keys))
}

pub(super) fn cache_info_type(ctx: &mut Context) -> Result<Id> {
    if let Some(&class) = ctx.builtins.get("functools._CacheInfo") {
        return Ok(class);
    }
    let name = ctx.text("CacheInfo");
    let fields = ctx.text("hits misses maxsize currsize");
    let module = ctx.text("functools");
    let class =
        super::super::namedtuple::factory(ctx, &[name, fields], &[("module".into(), module)])?;
    ctx.builtins.insert("functools._CacheInfo".into(), class);
    Ok(class)
}

pub(super) fn cache_method(ctx: &mut Context, name: &str, wrapper: Id) -> Result<Id> {
    let Value::Cached {
        table,
        capacity,
        hits,
        misses,
        ..
    } = *ctx.heap.get(wrapper)
    else {
        return Err(Error::new("TypeError", "expected a cache wrapper"));
    };
    if name.ends_with(".cache_clear") {
        let Value::Cached {
            entries,
            hits,
            misses,
            ..
        } = ctx.heap.get_mut(wrapper)
        else {
            unreachable!()
        };
        entries.clear();
        *hits = 0;
        *misses = 0;
        let clear = ctx.lookup_attr(table, "clear")?;
        return ctx.call(clear, Vec::new(), Vec::new());
    }
    let class = cache_info_type(ctx)?;
    let size = ctx.dict_entries(table).unwrap().len() as i64;
    let maxsize = capacity.map_or(NONE, |size| ctx.int(size as i64));
    let args = vec![ctx.int(hits), ctx.int(misses), maxsize, ctx.int(size)];
    ctx.call(class, args, Vec::new())
}
