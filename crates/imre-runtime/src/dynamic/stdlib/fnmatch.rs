use super::{Context, Error, Id, Result, Value};
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
    let pat = super::posixpath::fspath(ctx, pat)?;
    let (matches, bytes) = match ctx.heap.get(ctx.scalar_value(pat)) {
        Value::Str(text) => (compile(text), false),
        Value::Bytes(value) => (compile(&byte_spelling(value)?), true),
        _ => unreachable!(),
    };
    let iter = ctx.iterator(names)?;
    let result = ctx.alloc(Value::List(Vec::new()));
    while let Some(name) = ctx.next_item(iter)? {
        let selected = match ctx.heap.get(ctx.scalar_value(name)) {
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
