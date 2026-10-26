//! Calendar values used by HTTP response dates and timed cookie signatures.
use super::*;
use crate::dynamic::builtins::bind;
use crate::objects::{Calendar, CalendarNumber};

const FIELDS: [&str; 8] = [
    "year",
    "month",
    "day",
    "hour",
    "minute",
    "second",
    "microsecond",
    "fold",
];
const UNITS: [&str; 7] = [
    "days",
    "seconds",
    "microseconds",
    "milliseconds",
    "minutes",
    "hours",
    "weeks",
];
const TM: [&str; 9] = [
    "tm_year", "tm_mon", "tm_mday", "tm_hour", "tm_min", "tm_sec", "tm_wday", "tm_yday", "tm_isdst",
];

fn store(ctx: &mut Context, value: Calendar, tzinfo: Id) -> Id {
    ctx.alloc(Value::Calendar { value, tzinfo })
}

pub(in crate::dynamic) fn utc(ctx: &mut Context) -> Id {
    if let Some(&value) = ctx.builtins.get("datetime.timezone.utc") {
        return value;
    }
    let value = store(ctx, Calendar::utc(), NONE);
    ctx.builtins.insert("datetime.timezone.utc".into(), value);
    value
}

fn calendar(ctx: &Context, obj: Id, class: &str) -> Option<Calendar> {
    match ctx.heap.get(obj) {
        Value::Calendar { value, .. } if value.class() == class => Some(value.clone()),
        _ => None,
    }
}

fn zone(ctx: &Context, id: Id) -> Result<Option<Calendar>> {
    if id == NONE {
        return Ok(None);
    }
    calendar(ctx, id, "timezone").map(Some).ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "tzinfo argument must be None or of a tzinfo subclass, not type '{}'",
                ctx.type_name(id)
            ),
        )
    })
}

fn number(ctx: &Context, id: Id) -> Option<CalendarNumber> {
    match ctx.heap.get(id) {
        Value::Float(n) => Some(CalendarNumber::Float(*n)),
        Value::Int(n) => Some(CalendarNumber::Int(n.clone())),
        Value::Bool(b) => Some(CalendarNumber::Int(crate::Int::from(i64::from(*b)))),
        _ => None,
    }
}

fn required(value: Option<Id>, function: &str, name: &str, pos: usize) -> Result<Id> {
    value.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!("{function} missing required argument '{name}' (pos {pos})"),
        )
    })
}

pub(in crate::dynamic) fn attribute(
    ctx: &mut Context,
    receiver: Id,
    value: &Calendar,
    tzinfo: Id,
    name: &str,
) -> Result<Id> {
    let class = value.class();
    match (class, name) {
        ("datetime", "tzinfo") => return Ok(tzinfo),
        ("date" | "datetime", "timetuple" | "toordinal" | "weekday")
        | ("timedelta", "total_seconds") => {
            let callable = ctx.builtin(&format!("datetime.{class}.{name}"));
            return Ok(ctx.alloc(Value::Method { callable, receiver }));
        }
        ("datetime" | "date", _) => {
            let fields = if class == "date" {
                &FIELDS[..3]
            } else {
                &FIELDS[..]
            };
            if let Some(i) = fields.iter().position(|s| *s == name) {
                return Ok(ctx.int(value.fields()[i] as i64));
            }
        }
        ("timedelta", "days" | "seconds" | "microseconds") => {
            let (days, seconds, micros) = value.delta_parts();
            return Ok(ctx.int(match name {
                "days" => days,
                "seconds" => seconds,
                _ => micros,
            } as i64));
        }
        _ => {}
    }
    Err(Error::new(
        "AttributeError",
        format!("'datetime.{class}' object has no attribute '{name}'"),
    ))
}

pub(super) fn call(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match method {
        "timedelta" => {
            let args = bind(method, args, kwargs, UNITS)?;
            let mut units = std::array::from_fn(|_| CalendarNumber::Int(crate::Int::from(0)));
            for (i, id) in args.into_iter().enumerate() {
                if let Some(id) = id {
                    units[i] = number(ctx, id).ok_or_else(|| {
                        Error::new(
                            "TypeError",
                            format!(
                                "unsupported type for timedelta {} component: {}",
                                UNITS[i],
                                ctx.type_name(id)
                            ),
                        )
                    })?;
                }
            }
            Ok(store(ctx, Calendar::duration(&units)?, NONE))
        }
        "timezone" => {
            let [offset, name] = bind(method, args, kwargs, ["offset", "name"])?;
            let id = required(offset, "timezone()", "offset", 1)?;
            let offset = calendar(ctx, id, "timedelta").ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "timezone() argument 1 must be datetime.timedelta, not {}",
                        ctx.type_name(id)
                    ),
                )
            })?;
            let name = name
                .map(|id| {
                    ctx.str_value(id).map_err(|_| {
                        Error::new(
                            "TypeError",
                            format!(
                                "timezone() argument 2 must be str, not {}",
                                if id == NONE {
                                    "None"
                                } else {
                                    ctx.type_name(id)
                                }
                            ),
                        )
                    })
                })
                .transpose()?;
            let zone = Calendar::zone(&offset, name)?;
            Ok(if zone.is_utc() {
                utc(ctx)
            } else {
                store(ctx, zone, NONE)
            })
        }
        "datetime.fromtimestamp" => {
            let [timestamp, tz] = bind("fromtimestamp", args, kwargs, ["timestamp", "tz"])?;
            let tz = tz.unwrap_or(NONE);
            let zone = zone(ctx, tz)?;
            let id = required(timestamp, "fromtimestamp()", "timestamp", 1)?;
            let timestamp = number(ctx, id).ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!("argument must be int or float, not {}", ctx.type_name(id)),
                )
            })?;
            Ok(store(
                ctx,
                Calendar::from_timestamp(&timestamp, zone.as_ref())?,
                tz,
            ))
        }
        "date" | "datetime" => {
            let values = if method == "date" {
                let mut fields = [None; 9];
                fields[..3].copy_from_slice(&bind(method, args, kwargs, ["year", "month", "day"])?);
                fields
            } else {
                if args.len() > 8 {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "function takes at most 8 positional arguments ({} given)",
                            args.len()
                        ),
                    ));
                }
                bind(
                    method,
                    args,
                    kwargs,
                    [
                        "year",
                        "month",
                        "day",
                        "hour",
                        "minute",
                        "second",
                        "microsecond",
                        "tzinfo",
                        "fold",
                    ],
                )?
            };
            let mut fields = [0; 8];
            for (i, &key) in FIELDS.iter().enumerate() {
                let arg = values[if i == 7 { 8 } else { i }];
                if i < 3 {
                    required(arg, "function", key, i + 1)?;
                }
                if let Some(id) = arg {
                    let n = ctx.as_int(id).map_err(|_| {
                        Error::new(
                            "TypeError",
                            format!(
                                "'{}' object cannot be interpreted as an integer",
                                ctx.type_name(id)
                            ),
                        )
                    })?;
                    fields[i] = Calendar::component(&n)?;
                }
            }
            let tz = values[7].unwrap_or(NONE);
            let result = if method == "date" {
                Calendar::date(fields[0], fields[1], fields[2])?
            } else {
                Calendar::datetime(fields, zone(ctx, tz)?.as_ref())?
            };
            Ok(store(ctx, result, tz))
        }
        "datetime.timetuple"
        | "date.timetuple"
        | "date.toordinal"
        | "datetime.toordinal"
        | "date.weekday"
        | "datetime.weekday"
        | "timedelta.total_seconds" => {
            if args.len() != 1 || !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{}() takes no arguments",
                        method.split('.').next_back().unwrap()
                    ),
                ));
            }
            let (class, operation) = method.split_once('.').unwrap();
            let value = calendar(ctx, args[0], class)
                .ok_or_else(|| Error::new("TypeError", format!("expected datetime.{class}")))?;
            Ok(match operation {
                "total_seconds" => ctx.alloc(Value::Float(value.seconds())),
                "toordinal" => ctx.int(value.ordinal()),
                "weekday" => ctx.int((value.ordinal() - 1) % 7),
                "timetuple" => {
                    let values = value.timetuple().map(|n| ctx.int(n)).into();
                    ctx.alloc(Value::NamedTuple {
                        class: Some("time.struct_time"),
                        names: TM.map(str::to_owned).into(),
                        values,
                    })
                }
                _ => unreachable!(),
            })
        }
        _ => Err(Error::new(
            "AttributeError",
            format!("unknown datetime operation {method}"),
        )),
    }
}
