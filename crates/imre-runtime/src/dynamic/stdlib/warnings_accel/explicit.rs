//! The `_warnings` accelerator's explicit-location argument, filter, and registry paths.
use super::{display, ensure_state, warnings_attr, with_lock};
use crate::dynamic::stdlib::diagnostic_type_name;
use crate::dynamic::{Context, Error, Id, NONE, Result, TRUE, Value, builtins};

pub(super) fn warn_explicit(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    const FIELDS: &[&str] = &[
        "message",
        "category",
        "filename",
        "lineno",
        "module",
        "registry",
        "module_globals",
        "source",
    ];
    let required = super::super::required_args("warn_explicit", args, kwargs, FIELDS)?;
    let message = required(0)?;
    let category = required(1)?;
    let filename = required(2)?;
    let line = required(3)?;
    for (index, field) in FIELDS[..args.len()].iter().enumerate() {
        if kwargs.iter().any(|(key, _)| key == field) {
            let pos = index + 1;
            return Err(Error::new(
                "TypeError",
                format!(
                    "argument for warn_explicit() given by name ('{field}') and position ({pos})"
                ),
            ));
        }
    }
    if let Some((name, _)) = kwargs
        .iter()
        .find(|(key, _)| !FIELDS.contains(&key.as_str()))
    {
        let mut message = format!("warn_explicit() got an unexpected keyword argument '{name}'");
        if let Some(hint) = crate::objects::exceptions::keyword_suggestion(name, FIELDS) {
            message.push_str(&format!(". Did you mean '{hint}'?"));
        }
        return Err(Error::new("TypeError", message));
    }
    if ctx.string_storage(filename).is_none() {
        let actual = diagnostic_type_name(ctx, filename);
        return Err(Error::new(
            "TypeError",
            format!("warn_explicit() argument 'filename' must be str, not {actual}"),
        ));
    }
    let lineno = ctx
        .index_integer(line)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))?;
    let optional = |i: usize| {
        args.get(i).copied().or_else(|| {
            kwargs
                .iter()
                .find(|(key, _)| key == FIELDS[i])
                .map(|(_, value)| *value)
        })
    };
    let globals = optional(6).unwrap_or(NONE);
    let line = if globals == NONE {
        None
    } else {
        if ctx.dict_storage(globals).is_none() {
            let actual = diagnostic_type_name(ctx, globals);
            return Err(Error::new(
                "TypeError",
                format!("module_globals must be a dict, not '{actual}'"),
            ));
        }
        display::source_line(ctx, globals, lineno)?
    };
    emit(
        ctx,
        message,
        category,
        filename,
        lineno,
        optional(4),
        optional(5).unwrap_or(NONE),
        optional(7).unwrap_or(NONE),
        line,
    )
}

fn normalize_module(ctx: &mut Context, filename: Id) -> Result<Id> {
    let chars = ctx.string_codepoints(filename)?;
    if chars.is_empty() {
        return Ok(ctx.text("<unknown>"));
    }
    if let Some(prefix) = chars.strip_suffix(&[46, 112, 121]) {
        let prefix = prefix.to_vec();
        return Ok(ctx.text_codepoints(prefix));
    }
    Ok(filename)
}

fn dict_value(ctx: &mut Context, dict: Id, key: Id) -> Result<Option<Id>> {
    let (_, index) = ctx.find_dictionary_key(dict, key, None)?;
    Ok(index.map(|i| ctx.stored_entries(dict)[i].1))
}

fn already_warned(ctx: &mut Context, dict: Id, key: Id, record: bool) -> Result<bool> {
    let version_key = ctx.heap.code_name("version");
    let stored = dict_value(ctx, dict, version_key)?;
    // Dictionary key equality may already have mutated the filters.
    let version = ensure_state(ctx)?.version;
    let valid = stored.is_some_and(
        |value| matches!(ctx.heap.get(value), Value::Int(n) if n.to_i64() == Some(version as i64)),
    );
    if !valid {
        builtins::clear_container(ctx, dict);
        let current_version = ensure_state(ctx)?.version;
        let current = ctx.int(current_version as i64);
        ctx.set_dictionary_key(dict, version_key, current, None, true)?;
    } else if let Some(value) = dict_value(ctx, dict, key)? {
        let mark = ctx.heap.temporary_mark();
        ctx.heap.keep(value);
        let truth = ctx.truth(value)?;
        release_match_roots(ctx, mark);
        if truth {
            return Ok(true);
        }
    }
    if record {
        ctx.set_dictionary_key(dict, key, TRUE, None, true)?;
    }
    Ok(false)
}

fn matches_filter(ctx: &mut Context, pattern: Id, value: Id) -> Result<bool> {
    if pattern == NONE {
        return Ok(true);
    }
    if matches!(
        ctx.heap.get(pattern),
        Value::Str(_) | Value::SurrogateStr(_)
    ) {
        if ctx.string_storage(value).is_none() {
            return Err(Error::new(
                "TypeError",
                format!("Can't compare str and {}", diagnostic_type_name(ctx, value)),
            ));
        }
        return Ok(ctx.string_codepoints(pattern)? == ctx.string_codepoints(value)?);
    }
    let mark = ctx.heap.temporary_mark();
    let method = ctx.lookup_attr(pattern, "match")?;
    let matched = ctx.call(method, vec![value], Vec::new())?;
    let yes = ctx.truth(matched)?;
    release_match_roots(ctx, mark);
    Ok(yes)
}

// Dropping a regex result or a removed filter can run Python finalizers that
// change the next filter match.
fn release_match_roots(ctx: &mut Context, mark: usize) {
    if ctx.heap.discard_truth_operands(mark, ctx.modules) {
        ctx.collect_unreachable();
    }
}

fn select_action(
    ctx: &mut Context,
    text: Id,
    category: Id,
    module: Id,
    lineno: i32,
) -> Result<(Id, Id)> {
    let var = ensure_state(ctx)?.context;
    let get = ctx.lookup_attr(var, "get")?;
    let context = ctx.call(get, vec![NONE], Vec::new())?;
    let (filters, list_name) = if context == NONE {
        if let Some(value) = warnings_attr(ctx, "filters", false)? {
            ensure_state(ctx)?.filters = value;
        }
        (ensure_state(ctx)?.filters, "filters")
    } else {
        (
            ctx.lookup_attr(context, "_filters")?,
            "_warnings_context _filters",
        )
    };
    ctx.heap.keep(filters);
    let storage = ctx.scalar_value(filters);
    if !matches!(ctx.heap.get(storage), Value::List(_)) {
        let msg = if context == NONE {
            "_warnings.filters must be a list"
        } else {
            "_filters of warnings._warnings_context must be a list"
        };
        return Err(Error::new("ValueError", msg));
    }
    // A matcher can replace, resize, or clear the filters list. Keep the current
    // tuple alive and read each subsequent entry afresh, as filter_search does.
    for i in 0.. {
        let Value::List(list) = ctx.heap.get(storage) else {
            unreachable!()
        };
        let Some(&item) = list.get(i) else { break };
        let mark = ctx.heap.temporary_mark();
        ctx.heap.keep(item);
        let fields = ctx.tuple_items(item)?;
        let Some(&[action, msg, cls, mod_pattern, line]) = fields.as_deref() else {
            return Err(Error::new(
                "ValueError",
                format!("warnings.{list_name} item {i} isn't a 5-tuple"),
            ));
        };
        if ctx.string_storage(action).is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "action must be a string, not '{}'",
                    diagnostic_type_name(ctx, action)
                ),
            ));
        }
        let msg_ok = matches_filter(ctx, msg, text)?;
        let mod_ok = matches_filter(ctx, mod_pattern, module)?;
        let cls_ok = ctx.python_subclass_check(category, cls)?;
        let line = match ctx.heap.get(ctx.scalar_value(line)) {
            Value::Int(n) => n.to_i64().ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?,
            Value::Bool(value) => i64::from(*value),
            _ => return Err(Error::new("TypeError", "an integer is required")),
        };
        if msg_ok && mod_ok && cls_ok && (line == 0 || line == i64::from(lineno)) {
            return Ok((action, item));
        }
        release_match_roots(ctx, mark);
    }
    if let Some(value) = warnings_attr(ctx, "defaultaction", false)? {
        if ctx.string_storage(value).is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "_warnings.defaultaction must be a string, not '{}'",
                    diagnostic_type_name(ctx, value)
                ),
            ));
        }
        ensure_state(ctx)?.default_action = value;
    }
    Ok((ensure_state(ctx)?.default_action, NONE))
}

fn once_registry(ctx: &mut Context) -> Result<Id> {
    if let Some(value) = warnings_attr(ctx, "onceregistry", false)? {
        if ctx.dict_storage(value).is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "_warnings.onceregistry must be a dict, not '{}'",
                    diagnostic_type_name(ctx, value)
                ),
            ));
        }
        ensure_state(ctx)?.once_registry = value;
    }
    let registry = ensure_state(ctx)?.once_registry;
    ctx.heap.keep(registry);
    Ok(ctx
        .dict_storage(registry)
        .expect("validated warning registry"))
}

/// PyErr_SetObject normalizes a factory result against the requested exception class.
fn raise_warning(ctx: &mut Context, category: Id, message: Id) -> Result<Id> {
    let base = ctx.builtin("BaseException");
    if !ctx.is_type_object(category) || !ctx.direct_subclass(category, base) {
        let class = ctx.checked_format(category, true)?;
        return Err(Error::new(
            "SystemError",
            format!("_PyErr_SetObject: exception {class} is not a BaseException subclass"),
        ));
    }
    let actual = ctx.exact_type(message)?;
    if ctx.direct_subclass(actual, base) && ctx.python_subclass_check(actual, category)? {
        return ctx.op("raise", message, NONE, NONE);
    }
    let args = if message == NONE {
        Vec::new()
    } else {
        ctx.tuple_items(message)?.unwrap_or_else(|| vec![message])
    };
    let result = ctx.call(category, args, Vec::new()).and_then(|instance| {
        if matches!(ctx.heap.get(instance), Value::Raised(_)) {
            return Ok(instance);
        }
        let cls = ctx.checked_format(category, true)?;
        Err(Error::new(
            "TypeError",
            format!(
                "calling {cls} should have returned an instance of BaseException, not {}",
                diagnostic_type_name(ctx, instance)
            ),
        ))
    });
    match result {
        Ok(exception) => ctx.op("raise", exception, NONE, NONE),
        Err(error) => {
            let exception = ctx.raised_instance(error);
            ctx.heap.keep(exception);
            let args = ctx
                .checked_format(message, true)
                .unwrap_or_else(|_| "<unknown>".into());
            let name = ctx.class_name(category);
            let note = ctx.text(format!("Normalization failed: type={name} args={args}"));
            let _ = ctx.add_exception_note(exception, note);
            let Value::Raised(error) = ctx.heap.get(exception) else {
                unreachable!("normalization error")
            };
            let mut error = error.clone();
            error.tracing().instance = Some(exception);
            Err(error)
        }
    }
}

pub(in crate::dynamic) fn emit(
    ctx: &mut Context,
    message: Id,
    category: Id,
    filename: Id,
    lineno: i32,
    module: Option<Id>,
    registry: Id,
    source: Id,
    line: Option<Id>,
) -> Result<Id> {
    with_lock(ctx, |ctx| {
        if module == Some(NONE) {
            return Ok(NONE);
        }
        let registry = if registry == NONE {
            None
        } else {
            Some(
                ctx.dict_storage(registry)
                    .ok_or_else(|| Error::new("TypeError", "'registry' must be a dict or None"))?,
            )
        };
        let module = match module {
            Some(value) => value,
            None => normalize_module(ctx, filename)?,
        };
        let (message, text, category) = if super::warn::is_warning(ctx, message)? {
            let str_fn = ctx.builtin("str");
            let text = ctx.call(str_fn, vec![message], Vec::new())?;
            (message, text, ctx.exact_type(message)?)
        } else {
            let instance = ctx.call(category, vec![message], Vec::new())?;
            (instance, message, category)
        };
        let lineno_obj = ctx.int(i64::from(lineno));
        let key = ctx.alloc(Value::Tuple(vec![text, category, lineno_obj]));
        if let Some(dict) = registry
            && already_warned(ctx, dict, key, false)?
        {
            return Ok(NONE);
        }
        let (action, item) = select_action(ctx, text, category, module, lineno)?;
        match ctx.str_value(action).ok() {
            Some("error") => return raise_warning(ctx, category, message),
            Some("ignore") => return Ok(NONE),
            Some("always" | "all") => {}
            action_text => {
                let once = action_text == Some("once");
                let per_module = action_text == Some("module");
                let default = action_text == Some("default");
                if let Some(dict) = registry {
                    ctx.set_dictionary_key(dict, key, TRUE, None, true)?;
                }
                if once || per_module {
                    let dict = match registry {
                        Some(dict) => Some(dict),
                        None if once => Some(once_registry(ctx)?),
                        None => None,
                    };
                    if let Some(dict) = dict {
                        let altkey = ctx.alloc(Value::Tuple(vec![text, category]));
                        if already_warned(ctx, dict, altkey, true)? {
                            return Ok(NONE);
                        }
                    }
                } else if !default {
                    let action = ctx.checked_format(action, true)?;
                    let item = ctx.checked_format(item, true)?;
                    return Err(Error::new(
                        "RuntimeError",
                        format!("Unrecognized action ({action}) in warnings.filters:\n {item}"),
                    ));
                }
            }
        }
        display::show(ctx, text, message, category, filename, lineno, source, line)?;
        Ok(NONE)
    })
}
