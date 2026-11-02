//! Encode mappings and ordered query pairs for runtime-imported URL consumers.

use super::{Context, Error, Id, Result, Value};

/// Check the length and first tuple before iterating a non-mapping query.
fn check_sequence(ctx: &mut Context, query: Id) -> Result<()> {
    let len = ctx.builtin("len");
    let length = ctx.call(len, vec![query], Vec::new())?;
    if ctx.truth(length)? {
        let zero = ctx.int(0);
        let first = ctx.get_item(query, zero)?;
        let tuple = ctx.builtin("tuple");
        if !ctx.python_instance_check(first, tuple)? {
            return Err(Error::new("TypeError", ""));
        }
    }
    Ok(())
}

/// Join the quoted key-value pairs of a mapping or sequence of tuples.
pub(super) fn call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let &[query, doseq, safe, encoding, errors, via] = args else {
        return Err(Error::new(
            "TypeError",
            "_imre_urlencode expects six arguments",
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "_imre_urlencode expects positional arguments",
        ));
    }
    let query = match ctx.lookup_attr(query, "items") {
        Ok(_) => {
            let method = ctx.lookup_attr(query, "items")?;
            ctx.call(method, Vec::new(), Vec::new())?
        }
        Err(e) if ctx.is_attribute_error(&e) => {
            check_sequence(ctx, query).map_err(|original| {
                if !ctx.is_builtin_error(&original, "TypeError") {
                    return original;
                }
                let cause = ctx.raised_instance(original);
                let mut error = Error::new(
                    "TypeError",
                    "not a valid non-string sequence or mapping object",
                );
                error.tracing().cause = Some(cause);
                error.context = cause;
                error
            })?;
            query
        }
        Err(e) => return Err(e),
    };
    let doseq = ctx.truth(doseq)?;
    let pairs = ctx.iterator(query)?;
    ctx.heap.reserve_temporaries(1)?;
    ctx.heap.keep(pairs);
    let roots = ctx.heap.temporary_mark();
    let mut joined = String::new();
    while let Some(item) = ctx.next_item(pairs)? {
        ctx.heap.reserve_temporaries(1)?;
        ctx.heap.keep(item);
        let fields = ctx.items(item)?;
        let [k, v] = fields.as_slice() else {
            let message = if fields.len() < 2 {
                format!(
                    "not enough values to unpack (expected 2, got {})",
                    fields.len()
                )
            } else {
                format!(
                    "too many values to unpack (expected 2, got {})",
                    fields.len()
                )
            };
            return Err(Error::new("ValueError", message));
        };
        ctx.heap.reserve_temporaries(2)?;
        ctx.heap.keep(*k);
        ctx.heap.keep(*v);
        let key = encoded(ctx, *k, safe, encoding, errors, via)?;
        let expanded = if doseq
            && !matches!(
                ctx.heap.get(ctx.scalar_value(*v)),
                Value::Bytes(_) | Value::Str(_)
            ) {
            let len = ctx.builtin("len");
            match ctx.call(len, vec![*v], Vec::new()) {
                Ok(_) => Some(ctx.iterator(*v)?),
                Err(e) if ctx.is_builtin_error(&e, "TypeError") => None,
                Err(e) => return Err(e),
            }
        } else {
            None
        };
        let mut append = |ctx: &mut Context, value: Id| -> Result<()> {
            let value = encoded(ctx, value, safe, encoding, errors, via)?;
            let key = ctx.str_value(key)?;
            let value = ctx.str_value(value)?;
            let separators = 1 + usize::from(!joined.is_empty());
            joined
                .try_reserve(
                    key.len()
                        .saturating_add(value.len())
                        .saturating_add(separators),
                )
                .map_err(|_| Error::new("MemoryError", ""))?;
            if !joined.is_empty() {
                joined.push('&')
            }
            joined.push_str(key);
            joined.push('=');
            joined.push_str(value);
            Ok(())
        };
        if let Some(iter) = expanded {
            ctx.heap.reserve_temporaries(1)?;
            ctx.heap.keep(iter);
            let held = ctx.heap.temporary_mark();
            let mut limit = ctx.heap.streaming_collection_limit();
            loop {
                ctx.collect_streaming_garbage(&mut limit);
                let Some(value) = ctx.next_result(iter)? else {
                    break;
                };
                ctx.heap.reserve_temporaries(1)?;
                ctx.heap.keep(value);
                append(ctx, value)?;
                ctx.heap.release_temporaries_since(held);
            }
        } else {
            append(ctx, *v)?;
        }
        ctx.heap.release_temporaries_since(roots);
    }
    Ok(ctx.text(joined))
}

/// Encode one key or value through the supplied quoting function.
fn encoded(
    ctx: &mut Context,
    value: Id,
    safe: Id,
    encoding: Id,
    errors: Id,
    via: Id,
) -> Result<Id> {
    let bytes = matches!(ctx.heap.get(ctx.scalar_value(value)), Value::Bytes(_));
    let id = if bytes || matches!(ctx.heap.get(value), Value::Str(_)) {
        value
    } else if let Some(method) = ctx.bound_slot(value, "__str__")? {
        let rendered = ctx.call(method, Vec::new(), Vec::new())?;
        if ctx.str_value(rendered).is_err() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "__str__ returned non-string (type {})",
                    ctx.type_name(rendered)
                ),
            ));
        }
        rendered
    } else {
        ctx.text(ctx.display(value))
    };
    let mut args = vec![id, safe];
    if !bytes {
        args.extend([encoding, errors]);
    }
    let value = ctx.call(via, args, Vec::new())?;
    ctx.str_value(value)?;
    Ok(value)
}
