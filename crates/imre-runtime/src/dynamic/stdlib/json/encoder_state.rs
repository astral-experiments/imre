use super::{Context, Error, Fragment, Id, NONE, Result, Value};
use crate::dynamic::{builtins::boolean, copy_ids};

/// Parse the nine arguments of _json.make_encoder in CPython's conversion order.
pub(in crate::dynamic::stdlib) fn encoder_state(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    const FIELDS: [&str; 9] = [
        "markers",
        "default",
        "encoder",
        "indent",
        "key_separator",
        "item_separator",
        "sort_keys",
        "skipkeys",
        "allow_nan",
    ];
    let argument = required_args("make_encoder", args, kwargs, &FIELDS)?;
    let mut state = [NONE; FIELDS.len()];
    for (i, slot) in state.iter_mut().enumerate() {
        let value = argument(i)?;
        *slot = match i {
            4 | 5 => {
                Fragment::separator(ctx, value, i + 1)?;
                value
            }
            6..=8 => boolean(ctx.truth(value)?),
            _ => value,
        };
    }
    let markers = state[0];
    if markers != NONE && ctx.dict_entries(markers).is_none() {
        let class = ctx.exact_type(markers)?;
        let name = ctx.class_name(class);
        return Err(Error::new(
            "TypeError",
            format!(
                "make_encoder() argument 1 must be dict or None, not {}",
                &name[..name.floor_char_boundary(200)]
            ),
        ));
    }
    Ok(ctx.alloc(Value::Tuple(copy_ids(&state)?)))
}

/// PyArg_ParseTupleAndKeywords checks the total count first, then converts each
/// required argument before looking for the next. Defer missing-argument errors
/// until each field is read so user conversions retain their exception precedence.
pub(super) fn required_args<'a>(
    function: &'a str,
    args: &'a [Id],
    kwargs: &'a [(String, Id)],
    fields: &'a [&str],
) -> Result<impl Fn(usize) -> Result<Id> + 'a> {
    let count = args.len() + kwargs.len();
    let maximum = fields.len();
    if count > maximum {
        let kind = if args.is_empty() { "keyword " } else { "" };
        return Err(Error::new(
            "TypeError",
            format!("{function}() takes at most {maximum} {kind}arguments ({count} given)"),
        ));
    }
    Ok(move |i| {
        let field = fields[i];
        args.get(i)
            .copied()
            .or_else(|| {
                kwargs
                    .iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, value)| *value)
            })
            .ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "{function}() missing required argument '{field}' (pos {})",
                        i + 1
                    ),
                )
            })
    })
}
