//! Coroutine function flags and the identity marker from Lib/inspect.py.
use super::{Context, FALSE, Id, Result, TRUE, Value};

pub(super) fn marker(ctx: &mut Context) -> Id {
    let key = "inspect._is_coroutine_mark";
    if let Some(&value) = ctx.builtins.get(key) {
        return value;
    }
    let class = ctx.builtin("object");
    let value = ctx.instance(class, None);
    ctx.builtins.insert(key.to_owned(), value);
    value
}

fn attribute(ctx: &mut Context, obj: Id, name: &str) -> Result<Option<Id>> {
    match ctx.lookup_attr(obj, name) {
        Err(error) if ctx.is_attribute_error(&error) => Ok(None),
        result => result.map(Some),
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let param = if name == "markcoroutinefunction" {
        "func"
    } else {
        "obj"
    };
    let mut obj = super::single_argument(name, param, args, kwargs)?;
    match name {
        "_signature_identifier" => {
            // A str subclass can replace either the method or attribute lookup.
            if let Value::Instance { class, .. } = *ctx.heap.get(obj) {
                let overridden = ["isidentifier", "__getattribute__", "__getattr__"]
                    .iter()
                    .any(|name| ctx.class_attr(class, name).is_some());
                match ctx.lookup_attr(obj, "isidentifier") {
                    Ok(method) => return ctx.call(method, Vec::new(), Vec::new()),
                    Err(err) if !overridden && ctx.is_attribute_error(&err) => {}
                    Err(err) => return Err(err),
                }
            }
            let name = ctx.str_value(obj)?;
            let valid = ruff_python_stdlib::identifiers::is_identifier(name)
                || ruff_python_stdlib::keyword::is_keyword(name);
            return Ok(if valid { TRUE } else { FALSE });
        }
        "_signature_mapping" => {
            let storage = ctx
                .dict_storage(obj)
                .ok_or_else(|| super::Error::new("TypeError", "expected a parameter dictionary"))?;
            return Ok(ctx.alloc(Value::MappingProxy(storage)));
        }
        "_signature_layout" => return layout(ctx, obj),
        _ => {}
    }
    if name == "markcoroutinefunction" {
        obj = attribute(ctx, obj, "__func__")?.unwrap_or(obj);
        let mark = marker(ctx);
        ctx.set_attr(obj, "_is_coroutine_marker", mark)?;
        return Ok(obj);
    }
    loop {
        match ctx.heap.get(obj) {
            Value::Method { callable, .. }
                if matches!(ctx.heap.get(*callable), Value::Function { .. }) =>
            {
                obj = *callable
            }
            Value::Partial { function, .. } => obj = *function,
            Value::Function { code, .. } if ctx.code_object(*code).coroutine => return Ok(TRUE),
            _ => break,
        }
    }
    let marked =
        attribute(ctx, obj, "_is_coroutine_marker")?.is_some_and(|value| value == marker(ctx));
    Ok(if marked { TRUE } else { FALSE })
}

/// Read the same argument layout used to bind the boxed function's calls.
fn layout(ctx: &mut Context, obj: Id) -> Result<Id> {
    let Value::Function { code, .. } = ctx.heap.get(obj) else {
        return Err(super::Error::new("TypeError", "expected a Python function"));
    };
    let parameters = ctx.code_object(*code).parameters;
    let spec: serde_json::Value = serde_json::from_str(ctx.str_value(parameters)?)
        .map_err(|error| super::Error::new("SystemError", error.to_string()))?;
    let mut result = Vec::new();
    for (index, key) in ["posonly", "positional", "varargs", "kwonly", "kwargs"]
        .iter()
        .enumerate()
    {
        let kind = ctx.int(index as i64);
        let names = &spec[*key];
        let mut append = |name: &str| {
            let name = ctx.text(name);
            result.push(ctx.alloc(Value::Tuple(vec![kind, name])));
        };
        if let Some(names) = names.as_array() {
            for name in names.iter().filter_map(serde_json::Value::as_str) {
                append(name);
            }
        } else if let Some(name) = names.as_str() {
            append(name);
        }
    }
    Ok(ctx.alloc(Value::Tuple(result)))
}
