//! Wall time and the civil-time tuples used by HTTP dates and request logging.
use super::*;
use crate::dynamic::builtins::bind;
use crate::objects::time::{ClockParts, wall_time};

const FIELDS: [&str; 9] = [
    "tm_year", "tm_mon", "tm_mday", "tm_hour", "tm_min", "tm_sec", "tm_wday", "tm_yday", "tm_isdst",
];

pub(super) fn struct_time(ctx: &mut Context, values: Vec<Id>, zone: Id, offset: Id) -> Id {
    ctx.alloc(Value::NamedTuple {
        class: Some("time.struct_time"),
        names: FIELDS.map(str::to_owned).into(),
        values,
        extras: vec![("tm_zone", zone), ("tm_gmtoff", offset)],
    })
}

fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [sequence, attributes] = bind("structseq", args, kwargs, ["sequence", "dict"])?;
    let sequence = sequence.ok_or_else(|| {
        Error::new(
            "TypeError",
            "structseq() missing required argument 'sequence' (pos 1)",
        )
    })?;
    let mut values = ctx.items(sequence)?;
    let n = values.len();
    if !(9..=11).contains(&n) {
        let bound = if n < 9 { "least 9" } else { "most 11" };
        return Err(Error::new(
            "TypeError",
            format!("time.struct_time() takes an at {bound}-sequence ({n}-sequence given)"),
        ));
    }
    let mut zone = values.get(9).copied().unwrap_or(NONE);
    let mut offset = values.get(10).copied().unwrap_or(NONE);
    if let Some(dict) = attributes {
        let entries = ctx.dict_entries(dict).ok_or_else(|| {
            Error::new(
                "TypeError",
                "time.struct_time() takes a dict as second arg, if any",
            )
        })?;
        for &(key, value) in entries {
            match ctx.str_value(key).ok() {
                Some("tm_zone") if n == 9 => zone = value,
                Some("tm_gmtoff") if n < 11 => offset = value,
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        "time.struct_time() got duplicate or unexpected field name(s)",
                    ));
                }
            }
        }
    }
    values.truncate(9);
    Ok(struct_time(ctx, values, zone, offset))
}

fn civil(ctx: &mut Context, stamp: Id, utc: bool) -> Result<ClockParts> {
    match ctx.heap.get(stamp) {
        Value::None => Ok(ClockParts::from_float(wall_time(), utc)?),
        Value::Float(n) => Ok(ClockParts::from_float(*n, utc)?),
        Value::Int(_) | Value::Bool(_) => Ok(ClockParts::from_int(&ctx.as_int(stamp)?, utc)?),
        _ => {
            let index = ctx.bound_slot(stamp, "__index__")?.ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        ctx.type_name(stamp)
                    ),
                )
            })?;
            let value = ctx.call(index, Vec::new(), Vec::new())?;
            let n = ctx.as_int(value).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!("__index__ returned non-int (type {})", ctx.type_name(value)),
                )
            })?;
            Ok(ClockParts::from_int(&n, utc)?)
        }
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "struct_time" {
        return construct(ctx, args, kwargs);
    }
    let function = if name == "time" { "time.time" } else { name };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{function}() takes no keyword arguments"),
        ));
    }
    match name {
        "time" if args.is_empty() => Ok(ctx.alloc(Value::Float(wall_time()))),
        "time" => Err(Error::new(
            "TypeError",
            format!("time.time() takes no arguments ({} given)", args.len()),
        )),
        "gmtime" | "localtime" => {
            if args.len() > 1 {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{function}() takes at most 1 argument ({} given)",
                        args.len()
                    ),
                ));
            }
            let parts = civil(ctx, args.first().copied().unwrap_or(NONE), name == "gmtime")?;
            let fields = parts.fields.map(|n| ctx.int(n));
            let zone = parts.zone.map_or(NONE, |s| ctx.text(s.as_str()));
            let offset = ctx.int(parts.offset);
            Ok(struct_time(ctx, fields.into(), zone, offset))
        }
        _ => unreachable!("unregistered time function"),
    }
}
