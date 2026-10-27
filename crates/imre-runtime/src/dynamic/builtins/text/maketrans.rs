use super::{Context, Error, Id, NONE, Result, Value, type_error};

/// Construct the ordinary dictionary consumed by `str.translate`.
pub(super) fn table(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error("str.maketrans() takes no keyword arguments"));
    }
    match args {
        [] => Err(type_error("maketrans expected at least 1 argument, got 0")),
        [mapping] => from_dict(ctx, *mapping),
        [from, to, rest @ ..] if rest.len() <= 1 => {
            let text = |id, position| {
                ctx.str_value(id).map_err(|_| {
                    let kind = if id == NONE {
                        "None"
                    } else {
                        ctx.type_name(id)
                    };
                    type_error(format!(
                        "maketrans() argument {position} must be str, not {kind}"
                    ))
                })
            };
            let to = text(*to, 2)?;
            let delete = rest
                .first()
                .map(|&id| text(id, 3))
                .transpose()?
                .unwrap_or("");
            let from = ctx.str_value(*from).map_err(|_| {
                type_error(
                    "first maketrans argument must be a string if there is a second argument",
                )
            })?;
            if from.chars().count() != to.chars().count() {
                return Err(Error::new(
                    "ValueError",
                    "the first two maketrans arguments must have equal length",
                ));
            }
            let entries: Vec<_> = from
                .chars()
                .zip(to.chars().map(Some))
                .chain(delete.chars().map(|ch| (ch, None)))
                .collect();
            let result = ctx.dict(Vec::with_capacity(entries.len()));
            for (ch, replacement) in entries {
                let key = ctx.int(i64::from(u32::from(ch)));
                let value = replacement.map_or(NONE, |ch| ctx.int(i64::from(u32::from(ch))));
                ctx.put_item(result, key, value)?;
            }
            Ok(result)
        }
        _ => Err(type_error(format!(
            "maketrans expected at most 3 arguments, got {}",
            args.len()
        ))),
    }
}

/// Copy values unchanged and normalize each single-character key to its ordinal.
fn from_dict(ctx: &mut Context, mapping: Id) -> Result<Id> {
    let Value::Dict(entries) = ctx.heap.get(mapping) else {
        return Err(type_error(
            "if you give only one argument to maketrans it must be a dict",
        ));
    };
    let entries = entries.clone();
    let result = ctx.dict(Vec::with_capacity(entries.len()));
    for (key, value) in entries {
        let key = match ctx.heap.get(ctx.scalar_value(key)) {
            Value::Str(text) => {
                let mut chars = text.chars();
                let (Some(ch), None) = (chars.next(), chars.next()) else {
                    return Err(Error::new(
                        "ValueError",
                        "string keys in translatetable must be of length 1",
                    ));
                };
                ctx.int(i64::from(u32::from(ch)))
            }
            Value::Int(_) | Value::Bool(_) => key,
            _ => {
                return Err(type_error(
                    "keys in translate table mustbe strings or integers",
                ));
            }
        };
        ctx.put_item(result, key, value)?;
    }
    Ok(result)
}
