//! Parse calendar or ISO week dates followed by an optional clock and UTC offset.

use super::*;

use crate::objects::iso::{Parsed, parse, parse_date_only};

/// Accept a lone surrogate at a potential date/time separator, as CPython does before UTF-8 encoding.
fn parse_surrogates(points: &[u32]) -> Result<Option<Parsed>> {
    let mut text = String::new();
    let capacity = points
        .len()
        .checked_mul(4)
        .ok_or_else(|| Error::new("MemoryError", ""))?;
    text.try_reserve_exact(capacity)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let mut replaced = false;
    for (i, &point) in points.iter().enumerate() {
        if let Some(ch) = char::from_u32(point) {
            text.push(ch);
        } else if !replaced && matches!(i, 7 | 8 | 10) {
            text.push('T');
            replaced = true;
        } else {
            return Ok(None);
        }
    }
    Ok(parse(&text))
}

fn invalid_input(ctx: &mut Context, input: Id) -> Result<Id> {
    let repr = builtins::call(ctx, "repr", &[input], &[])?;
    if let Ok(text) = ctx.str_value(repr) {
        return Err(Error::new(
            "ValueError",
            format!("Invalid isoformat string: {text}"),
        ));
    }
    let prefix = ctx.text("Invalid isoformat string: ");
    let message = ctx.concat_strings(prefix, repr)?;
    Err(ctx.lookup_error("ValueError", message))
}

pub(super) fn fromisoformat(
    ctx: &mut Context,
    owner: &'static str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (class, args) = calendar_class_args(ctx, owner, "fromisoformat", args)?;
    let input = match (args, kwargs) {
        ([input], []) => *input,
        _ => {
            let name = ctx.class_qualname(class);
            builtins::one_positional_arg(&format!("{name}.fromisoformat"), args, kwargs)?
        }
    };
    let Some(storage) = ctx.string_storage(input) else {
        return Err(Error::new(
            "TypeError",
            "fromisoformat: argument must be str",
        ));
    };
    if owner == "datetime.date" {
        let Some(fields) = ctx.str_value(storage).ok().and_then(parse_date_only) else {
            return invalid_input(ctx, input);
        };
        // The receiving class validates these fields, including years past 9999.
        let args = fields
            .into_iter()
            .map(|field| ctx.int(i64::from(field)))
            .collect();
        return ctx.call(class, args, vec![]);
    }
    let parsed = match ctx.heap.get(storage) {
        Value::Str(text) => parse(text),
        Value::SurrogateStr(points) => parse_surrogates(points)?,
        _ => unreachable!("validated string storage"),
    };
    let Some(Parsed {
        mut date,
        mut clock,
        offset,
    }) = parsed
    else {
        return invalid_input(ctx, input);
    };
    let tz = iso_zone(ctx, offset)?;
    Calendar::iso_midnight(&mut date, &mut clock)?;
    let [y, m, d] = date;
    let [h, min, sec, us] = clock;
    datetime_replace_fields(ctx, class, [y, m, d, h, min, sec, us, 0], tz)
}
