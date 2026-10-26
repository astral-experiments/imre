//! Bind the three mail date functions used by HTTP to the native calendar.
use super::*;
use crate::dynamic::builtins::bind;
use crate::objects::Calendar;

fn required(value: Option<Id>, name: &str, parameter: &str) -> Result<Id> {
    value.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!("{name}() missing 1 required positional argument: '{parameter}'"),
        )
    })
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match name {
        "parsedate_to_datetime" => {
            let [data] = bind(name, args, kwargs, ["data"])?;
            let id = required(data, name, "data")?;
            let (value, zone) = if let Value::Str(text) = ctx.heap.get(id) {
                Calendar::parse_mail_date(text)?
            } else {
                if !ctx.truth(id)? {
                    return Err(invalid_date(ctx, id));
                }
                if let Value::Bytes(data) | Value::ByteArray(data) = ctx.heap.get(id) {
                    if data.iter().all(u8::is_ascii_whitespace) {
                        return Err(invalid_date(ctx, id));
                    }
                    return Err(Error::new(
                        "TypeError",
                        "endswith first arg must be bytes or a tuple of bytes, not str",
                    ));
                }
                ctx.lookup_attr(id, "split")?;
                return Err(Error::new("TypeError", "expected a date string"));
            };
            let tzinfo = match zone {
                None => NONE,
                Some(z) if z.is_utc() => datetime::utc(ctx),
                Some(z) => datetime::store(ctx, z, NONE),
            };
            Ok(datetime::store(ctx, value, tzinfo))
        }
        "format_datetime" => {
            let [dt, usegmt] = bind(name, args, kwargs, ["dt", "usegmt"])?;
            let id = required(dt, name, "dt")?;
            let usegmt = ctx.optional_truth(usegmt, false)?;
            let Value::Calendar { value, .. } = ctx.heap.get(id) else {
                ctx.lookup_attr(id, "timetuple")?;
                return Err(Error::new("TypeError", "expected a datetime"));
            };
            if value.class() != "datetime" {
                let attr = if value.class() == "date" {
                    "tzinfo"
                } else {
                    "timetuple"
                };
                return Err(Error::new(
                    "AttributeError",
                    format!(
                        "'datetime.{}' object has no attribute '{attr}'",
                        value.class()
                    ),
                ));
            }
            let text = value.format_mail_date(usegmt)?;
            Ok(ctx.text(text))
        }
        "formatdate" => {
            let [timeval, localtime, usegmt] =
                bind(name, args, kwargs, ["timeval", "localtime", "usegmt"])?;
            let value = timeval
                .filter(|&id| id != NONE)
                .map(|id| {
                    datetime::number(ctx, id).ok_or_else(|| {
                        Error::new(
                            "TypeError",
                            format!("argument must be int or float, not {}", ctx.type_name(id)),
                        )
                    })
                })
                .transpose()?;
            let local = ctx.optional_truth(localtime, false)?;
            let gmt = ctx.optional_truth(usegmt, false)?;
            Ok(ctx.text(Calendar::format_mail_timestamp(value.as_ref(), local, gmt)?))
        }
        _ => unreachable!(),
    }
}

fn invalid_date(ctx: &Context, id: Id) -> Error {
    Error::new(
        "ValueError",
        format!("Invalid date value or format \"{}\"", ctx.display(id)),
    )
}
