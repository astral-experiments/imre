//! Coroutine function flags and the identity marker from Lib/inspect.py.
use super::{Context, Error, FALSE, Id, Result, TRUE, Value};

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
    let mut value = args.first().copied();
    for (key, arg) in kwargs {
        if key != param {
            return Err(Error::new(
                "TypeError",
                format!("{name}() got an unexpected keyword argument '{key}'"),
            ));
        }
        if value.replace(*arg).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() got multiple values for argument '{param}'"),
            ));
        }
    }
    if args.len() > 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes 1 positional argument but {} were given",
                args.len()
            ),
        ));
    }
    let mut obj = value.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!("{name}() missing 1 required positional argument: '{param}'"),
        )
    })?;
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
