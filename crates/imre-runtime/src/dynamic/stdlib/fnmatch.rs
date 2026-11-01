use super::{Context, Error, Id, Result, Value};
use crate::objects::fnmatch::{byte_spelling, compile, mixed_message};

fn arguments(
    function: &str,
    first: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<(Id, Id)> {
    let mut names = args.first().copied();
    let mut pat = args.get(1).copied();
    for (key, value) in kwargs {
        let slot = match key.as_str() {
            key if key == first => &mut names,
            "pat" => &mut pat,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("{function}() got an unexpected keyword argument '{key}'"),
                ));
            }
        };
        if slot.replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("{function}() got multiple values for argument '{key}'"),
            ));
        }
    }
    if args.len() > 2 {
        return Err(Error::new(
            "TypeError",
            format!(
                "{function}() takes 2 positional arguments but {} were given",
                args.len()
            ),
        ));
    }
    let missing = match (names, pat) {
        (Some(names), Some(pat)) => return Ok((names, pat)),
        (None, None) => format!("2 required positional arguments: '{first}' and 'pat'"),
        (None, Some(_)) => format!("1 required positional argument: '{first}'"),
        (Some(_), None) => "1 required positional argument: 'pat'".to_owned(),
    };
    Err(Error::new(
        "TypeError",
        format!("{function}() missing {missing}"),
    ))
}

pub(super) fn call(
    ctx: &mut Context,
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let filter = function == "filter";
    let first = if filter { "names" } else { "name" };
    let (names, pat) = arguments(function, first, args, kwargs)?;
    let pat = if filter {
        super::posixpath::fspath(ctx, pat)?
    } else {
        pat
    };
    let (matches, bytes) = match ctx.heap.get(ctx.scalar_value(pat)) {
        Value::Str(text) => (compile(text), false),
        Value::Bytes(value) => (compile(&byte_spelling(value)?), true),
        _ => {
            let name = ctx.type_name(pat);
            let message = if super::builtins::hashable(ctx, pat) {
                format!("object of type '{name}' has no len()")
            } else {
                format!("unhashable type: '{name}'")
            };
            return Err(Error::new("TypeError", message));
        }
    };
    let matches_name = |ctx: &Context, name| -> Result<bool> {
        Ok(match ctx.heap.get(ctx.scalar_value(name)) {
            Value::Str(text) if !bytes => matches(text),
            Value::Bytes(value) | Value::ByteArray(value) if bytes => {
                matches(&byte_spelling(value)?)
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
        })
    };
    if !filter {
        return Ok(super::builtins::boolean(matches_name(ctx, names)?));
    }
    let iter = ctx.iterator(names)?;
    let result = ctx.alloc(Value::List(Vec::new()));
    while let Some(name) = ctx.next_item(iter)? {
        if matches_name(ctx, name)? {
            let Value::List(items) = ctx.heap.get_mut(result) else {
                unreachable!()
            };
            items.push(name);
        }
    }
    Ok(result)
}
