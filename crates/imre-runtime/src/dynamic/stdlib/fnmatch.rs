use super::{Context, Error, Id, NONE, Result, TRUE, Value};
use crate::objects::fnmatch::{byte_spelling, compile, mixed_message};

fn arguments(args: &[Id], kwargs: &[(String, Id)]) -> Result<(Id, Id)> {
    let mut names = args.first().copied();
    let mut pat = args.get(1).copied();
    for (key, value) in kwargs {
        let slot = match key.as_str() {
            "names" => &mut names,
            "pat" => &mut pat,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("filter() got an unexpected keyword argument '{key}'"),
                ));
            }
        };
        if slot.replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("filter() got multiple values for argument '{key}'"),
            ));
        }
    }
    if args.len() > 2 {
        return Err(Error::new(
            "TypeError",
            format!(
                "filter() takes 2 positional arguments but {} were given",
                args.len()
            ),
        ));
    }
    let missing = match (names, pat) {
        (Some(names), Some(pat)) => return Ok((names, pat)),
        (None, None) => "2 required positional arguments: 'names' and 'pat'",
        (None, Some(_)) => "1 required positional argument: 'names'",
        (Some(_), None) => "1 required positional argument: 'pat'",
    };
    Err(Error::new(
        "TypeError",
        format!("filter() missing {missing}"),
    ))
}

pub(super) fn filter(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (names, pat) = arguments(args, kwargs)?;
    let pat = if let Some(method) = ctx.bound_slot(pat, "__fspath__")? {
        let value = ctx.call(method, Vec::new(), Vec::new())?;
        if !matches!(ctx.heap.get(value), Value::Str(_) | Value::Bytes(_)) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "expected {}.__fspath__() to return str or bytes, not {}",
                    ctx.type_name(pat),
                    ctx.type_name(value)
                ),
            ));
        }
        value
    } else {
        pat
    };
    let (matches, bytes) = match ctx.heap.get(pat) {
        Value::Str(text) => (compile(text), false),
        Value::Bytes(value) => (compile(&byte_spelling(value)), true),
        _ => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "expected str, bytes or os.PathLike object, not {}",
                    ctx.type_name(pat)
                ),
            ));
        }
    };
    let iter = ctx.op("iter", names, NONE, NONE)?;
    let result = ctx.alloc(Value::List(Vec::new()));
    while ctx.op("more", iter, NONE, NONE)? == TRUE {
        let name = ctx.op("next", iter, NONE, NONE)?;
        let selected = match ctx.heap.get(name) {
            Value::Str(text) if !bytes => matches(text),
            Value::Bytes(value) | Value::ByteArray(value) if bytes => {
                matches(&byte_spelling(value))
            }
            Value::Str(_) | Value::Bytes(_) | Value::ByteArray(_) => {
                return Err(Error::new("TypeError", mixed_message(bytes)));
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "expected string or bytes-like object, got '{}'",
                        ctx.type_name(name)
                    ),
                ));
            }
        };
        if selected {
            let Value::List(items) = ctx.heap.get_mut(result) else {
                unreachable!()
            };
            items.push(name);
        }
    }
    Ok(result)
}
