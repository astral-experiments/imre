use super::{option, tuple_text};
use crate::dynamic::{
    Context,
    value::{CACHE_CAPACITY, Error, FALSE, Id, NONE, Result, Value},
};

pub(in crate::dynamic) const PARTIAL_CLASS: &str = "functools.partial";
pub(in crate::dynamic) const LRU_CACHE_CLASS: &str = "functools._lru_cache_wrapper";
pub(in crate::dynamic) const COPY_METHODS: &[&str] = &["__reduce__", "__copy__", "__deepcopy__"];

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

pub(super) const ASSIGNED: &[&str] = &[
    "__module__",
    "__name__",
    "__qualname__",
    "__doc__",
    "__annotate__",
    "__type_params__",
];
pub(super) const UPDATED: &[&str] = &["__dict__"];

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

pub(super) fn cache(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if kwargs.iter().any(|(key, _)| key == "user_function") {
        return Err(Error::new(
            "TypeError",
            "cache() got some positional-only arguments passed as keyword arguments: 'user_function'",
        ));
    }
    let function = super::single_argument("cache", "user_function", args, kwargs)?;
    let module = ctx.module("functools")?;
    let lru_cache = ctx.lookup_attr(module, "lru_cache")?;
    let factory = ctx.call(lru_cache, Vec::new(), vec![("maxsize".into(), NONE)])?;
    ctx.call(factory, vec![function], Vec::new())
}

pub(super) fn lru_cache(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [maxsize, typed] = super::builtins::bind("lru_cache", args, kwargs, ["maxsize", "typed"])?;
    let mut maxsize = maxsize.unwrap_or_else(|| ctx.int(CACHE_CAPACITY as i64));
    let typed = typed.unwrap_or(FALSE);
    if ctx.as_int(maxsize).is_ok() {
        let zero = ctx.int(0);
        let negative = ctx.binary("<", maxsize, zero)?;
        if ctx.truth(negative)? {
            maxsize = zero;
        }
    } else if ctx.callable(maxsize) && matches!(ctx.heap.get(typed), Value::Bool(_)) {
        let capacity = ctx.int(CACHE_CAPACITY as i64);
        return wrap_cache(ctx, maxsize, capacity, typed);
    } else if maxsize != NONE {
        return Err(Error::new(
            "TypeError",
            "Expected first argument to be an integer, a callable, or None",
        ));
    }
    // Computed-only imports can expose the intrinsic module without its bundled Python source.
    let module = ctx.module("functools")?;
    match ctx.lookup_attr(module, "_lru_cache_decorator") {
        Ok(factory) => ctx.call(factory, vec![maxsize, typed], Vec::new()),
        Err(error) if ctx.is_attribute_error(&error) => {
            Ok(ctx.alloc(Value::CacheDecorator { maxsize, typed }))
        }
        Err(error) => Err(error),
    }
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

// Attach the same public attributes when no Python decorating function was registered.
pub(in crate::dynamic) fn wrap_cache(
    ctx: &mut Context,
    function: Id,
    maxsize: Id,
    typed: Id,
) -> Result<Id> {
    let wrapper = make_cache(ctx, function, maxsize, typed)?;
    let mark = ctx.heap.temporary_mark();
    ctx.heap.keep(wrapper);
    let result = (|| {
        let parameters = ctx.alloc(Value::CacheParameters { maxsize, typed });
        ctx.set_attr(wrapper, "cache_parameters", parameters)?;
        update_wrapper(ctx, &[wrapper, function], &[])
    })();
    ctx.heap.release_temporaries_since(mark);
    result
}

pub(super) fn update_wrapper(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let wrapper = option(args, 0, kwargs, "wrapper")?;
    let wrapped = option(args, 1, kwargs, "wrapped")?;
    let (wrapper, wrapped) = match (wrapper, wrapped) {
        (Some(wrapper), Some(wrapped)) => (wrapper, wrapped),
        (None, None) => {
            return Err(Error::new(
                "TypeError",
                "update_wrapper() missing 2 required positional arguments: 'wrapper' and 'wrapped'",
            ));
        }
        (only, _) => {
            let name = if only.is_none() { "wrapper" } else { "wrapped" };
            return Err(Error::new(
                "TypeError",
                format!("update_wrapper() missing 1 required positional argument: '{name}'"),
            ));
        }
    };
    let assigned =
        option(args, 2, kwargs, "assigned")?.unwrap_or_else(|| tuple_text(ctx, ASSIGNED));
    let updated = option(args, 3, kwargs, "updated")?.unwrap_or_else(|| tuple_text(ctx, UPDATED));
    for key in ctx.items(assigned)? {
        let name = ctx.str_value(key)?.to_owned();
        match ctx.lookup_attr(wrapped, &name) {
            Ok(value) => ctx.set_attr(wrapper, &name, value)?,
            Err(error) if ctx.is_attribute_error(&error) => {}
            Err(error) => return Err(error),
        }
    }
    for key in ctx.items(updated)? {
        let name = ctx.str_value(key)?.to_owned();
        let target = ctx.lookup_attr(wrapper, &name)?;
        let update = ctx.lookup_attr(target, "update")?;
        let source = match ctx.lookup_attr(wrapped, &name) {
            Ok(value) => value,
            Err(error) if ctx.is_attribute_error(&error) => ctx.dict(Vec::new()),
            Err(error) => return Err(error),
        };
        ctx.call(update, vec![source], Vec::new())?;
    }
    // A wrapped function's dictionary can itself contain __wrapped__.
    ctx.set_attr(wrapper, "__wrapped__", wrapped)?;
    Ok(wrapper)
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
