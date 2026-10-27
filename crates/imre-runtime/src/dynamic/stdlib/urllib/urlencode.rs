//! Encode mappings and ordered query pairs for runtime-imported URL consumers.

use super::{Context, Error, Id, NONE, Result, Value, quote::quote, required};

/// Bind the Python function's arguments, including its keyword diagnostics.
fn arguments(args: &[Id], kwargs: &[(String, Id)]) -> Result<[Option<Id>; 6]> {
    let names = ["query", "doseq", "safe", "encoding", "errors", "quote_via"];
    let mut values = [None; 6];
    for (slot, &arg) in values.iter_mut().zip(args) {
        *slot = Some(arg);
    }
    for (key, arg) in kwargs {
        let Some(index) = names.iter().position(|name| name == key) else {
            let mut message = format!("urlencode() got an unexpected keyword argument '{key}'");
            if let Some(hint) = crate::objects::exceptions::keyword_suggestion(key, &names) {
                message.push_str(&format!(". Did you mean '{hint}'?"));
            }
            return Err(Error::new("TypeError", message));
        };
        if values[index].replace(*arg).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("urlencode() got multiple values for argument '{key}'"),
            ));
        }
    }
    if args.len() > names.len() {
        return Err(Error::new(
            "TypeError",
            format!(
                "urlencode() takes from 1 to 6 positional arguments but {} were given",
                args.len()
            ),
        ));
    }
    Ok(values)
}

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
    let [query, doseq, safe, encoding, errors, via] = arguments(args, kwargs)?;
    let query = required(query, "urlencode", "query")?;
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
    let doseq = ctx.optional_truth(doseq, false)?;
    let safe = safe.unwrap_or_else(|| ctx.text(""));
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
        let expanded = if doseq && !matches!(ctx.heap.get(*v), Value::Bytes(_) | Value::Str(_)) {
            let len = ctx.builtin("len");
            match ctx.call(len, vec![*v], Vec::new()) {
                Ok(_) => Some(ctx.items(*v)?),
                Err(e) if e.kind == "TypeError" => None,
                Err(e) => return Err(e),
            }
        } else {
            None
        };
        if let Some(values) = &expanded {
            ctx.heap.reserve_temporaries(values.len())?;
            for &value in values {
                ctx.heap.keep(value);
            }
        }
        for value in expanded.as_deref().unwrap_or(std::slice::from_ref(v)) {
            let value = encoded(ctx, *value, safe, encoding, errors, via)?;
            if !joined.is_empty() {
                joined.push('&')
            }
            joined.push_str(&key);
            joined.push('=');
            joined.push_str(&value);
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
    encoding: Option<Id>,
    errors: Option<Id>,
    via: Option<Id>,
) -> Result<String> {
    let bytes = matches!(ctx.heap.get(value), Value::Bytes(_));
    let id = if bytes || matches!(ctx.heap.get(value), Value::Str(_)) {
        value
    } else if let Some(method) = ctx.bound_slot(value, "__str__")? {
        let rendered = ctx.call(method, Vec::new(), Vec::new())?;
        if !matches!(ctx.heap.get(rendered), Value::Str(_)) {
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
    let value = if let Some(function) = via {
        let mut args = vec![id, safe];
        if !bytes {
            args.extend([encoding.unwrap_or(NONE), errors.unwrap_or(NONE)]);
        }
        ctx.call(function, args, Vec::new())?
    } else {
        quote(
            ctx,
            "quote_plus",
            id,
            Some(safe),
            if bytes { None } else { encoding },
            if bytes { None } else { errors },
        )?
    };
    Ok(ctx.str_value(value)?.to_owned())
}
