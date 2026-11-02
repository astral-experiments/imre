//! Calendar values used by HTTP response dates and timed cookie signatures.
use num_bigint::{BigInt, Sign};
use num_traits::Euclid;

use super::*;
use crate::dynamic::builtins::bind;
use crate::objects::{Calendar, CalendarNumber};

mod reduction;

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

pub(in crate::dynamic) struct Members {
    pub(in crate::dynamic) owner: &'static str,
    fields: &'static [&'static str],
    pub(in crate::dynamic) methods: &'static [&'static str],
}

pub(in crate::dynamic) fn members(class: &str) -> Option<Members> {
    let (owner, fields, methods): (_, &[_], &[_]) = match class {
        "datetime.date" => (
            "datetime.date",
            &FIELDS[..3],
            &[
                "__format__",
                "__reduce__",
                "__repr__",
                "__str__",
                "ctime",
                "isoweekday",
                "toordinal",
                "weekday",
                "timetuple",
            ],
        ),
        "datetime.datetime" => (
            "datetime.datetime",
            &["hour", "minute", "second", "microsecond", "fold", "tzinfo"],
            &[
                "__reduce__",
                "__reduce_ex__",
                "__repr__",
                "__str__",
                "ctime",
                "timetuple",
            ],
        ),
        "datetime.time" => (
            "datetime.time",
            &["hour", "minute", "second", "microsecond", "fold", "tzinfo"],
            &[
                "__format__",
                "__reduce__",
                "__reduce_ex__",
                "__repr__",
                "__str__",
            ],
        ),
        "datetime.timedelta" => (
            "datetime.timedelta",
            &["days", "seconds", "microseconds"],
            &[
                "__bool__",
                "__neg__",
                "__mul__",
                "__rmul__",
                "__truediv__",
                "__rtruediv__",
                "__reduce__",
                "__repr__",
                "__str__",
                "total_seconds",
            ],
        ),
        _ => return None,
    };
    Some(Members {
        owner,
        fields,
        methods,
    })
}

impl Members {
    pub(in crate::dynamic) fn owns(&self, name: &str) -> bool {
        matches!(name, "__new__" | "min" | "max" | "resolution")
            || self.fields.contains(&name)
            || self.methods.contains(&name)
    }
}

/// Install descriptors once, so inherited access goes through the ordinary class MRO.
pub(in crate::dynamic) fn install_members(ctx: &mut Context, class: &str) {
    let Some(members) = members(class) else {
        return;
    };
    if class == "datetime.datetime" {
        ctx.builtin("datetime.date");
    }
    ctx.builtin(&format!("{class}.__new__"));
    for method in members.methods {
        ctx.builtin(&format!("{class}.{method}"));
    }
    for &field in members.fields {
        if class == "datetime.timedelta" {
            ctx.readonly_member(members.owner, field);
        } else {
            let get = ctx.builtin(&format!("datetime._get.{class}.{field}"));
            let descriptor = ctx.alloc(Value::GetSetDescriptor {
                owner: members.owner,
                name: field,
                get,
                set: None,
            });
            ctx.builtins.insert(format!("{class}.{field}"), descriptor);
        }
    }
}

pub(in crate::dynamic) fn store(ctx: &mut Context, value: Calendar, tzinfo: Id) -> Id {
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
    match ctx.heap.get(ctx.scalar_value(obj)) {
        Value::Calendar { value, .. }
            if value.class() == class || (class == "date" && value.class() == "datetime") =>
        {
            Some(value.clone())
        }
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

pub(super) fn number(ctx: &Context, id: Id) -> Option<CalendarNumber> {
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

/// Read native field storage even when a Python subclass overrides the field name.
pub(in crate::dynamic) fn read_field(
    ctx: &mut Context,
    owner: &str,
    name: &str,
    receiver: Id,
) -> Result<Id> {
    let class = owner.strip_prefix("datetime.").unwrap();
    let Value::Calendar { value, tzinfo } = ctx.heap.get(ctx.scalar_value(receiver)) else {
        return Err(field_type_error(ctx, owner, name, receiver));
    };
    if value.class() != class && !(class == "date" && value.class() == "datetime") {
        return Err(field_type_error(ctx, owner, name, receiver));
    }
    if name == "tzinfo" {
        return Ok(*tzinfo);
    }
    let part = match class {
        "timedelta" => {
            let (days, seconds, micros) = value.delta_parts();
            match name {
                "days" => days as i64,
                "seconds" => seconds as i64,
                _ => micros as i64,
            }
        }
        "time" => value.clock_parts()[FIELDS[3..].iter().position(|&s| s == name).unwrap()],
        _ => value.fields()[FIELDS.iter().position(|&s| s == name).unwrap()] as i64,
    };
    Ok(ctx.int(part))
}

fn field_type_error(ctx: &Context, owner: &str, field: &str, receiver: Id) -> Error {
    Error::new(
        "TypeError",
        format!(
            "descriptor '{field}' for '{owner}' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        ),
    )
}

fn delta_component(ctx: &mut Context, value: Id) -> Result<i32> {
    ctx.index_integer(value)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

/// Integer subclasses can return arbitrary objects from multiplication. CPython splits
/// that result with divmod and validates the normalized microsecond and second fields.
fn split_delta_component(ctx: &mut Context, value: Id, unit: i32) -> Result<(Id, i32)> {
    let divisor = ctx.int(i64::from(unit));
    let result = ctx.divmod(value, divisor)?;
    let tuple = ctx.tuple_items(result)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "divmod() returned non-tuple (type {})",
                ctx.type_name(result)
            ),
        )
    })?;
    let [quotient, remainder] = tuple.as_slice() else {
        return Err(Error::new(
            "TypeError",
            format!("divmod() returned a tuple of size {}", tuple.len()),
        ));
    };
    let remainder = delta_component(ctx, *remainder)?;
    if !(0..unit).contains(&remainder) {
        return Err(Error::new(
            "TypeError",
            "divmod() returned a value out of range",
        ));
    }
    Ok((*quotient, remainder))
}

fn multiply_delta_integer(ctx: &mut Context, factor: Id, us: Id) -> Result<crate::Int> {
    let product = ctx.binary("*", factor, us)?;
    if let Value::Int(n) = ctx.heap.get(product) {
        Ok(n.clone())
    } else {
        let (seconds, micros) = split_delta_component(ctx, product, 1_000_000)?;
        let (days, seconds) = split_delta_component(ctx, seconds, 86_400)?;
        let days = delta_component(ctx, days)?;
        let total =
            (i128::from(days) * 86_400 + i128::from(seconds)) * 1_000_000 + i128::from(micros);
        Ok(crate::Int::new(total.into()))
    }
}

/// Divide integer payloads exactly and resolve half-microsecond ties toward the even result.
fn nearest_quotient(ctx: &Context, numerator: Id, denominator: Id) -> Result<crate::Int> {
    let as_integer = |id| {
        ctx.as_int(id)
            .map_err(|_| Error::new("TypeError", "non-integer arguments in division"))
    };
    let (a, b) = (as_integer(numerator)?, as_integer(denominator)?);
    let (a, b) = (a.big(), b.big());
    if b.sign() == Sign::NoSign {
        return Err(Error::new("ZeroDivisionError", "division by zero"));
    }
    let (mut quotient, remainder) = a.magnitude().div_rem_euclid(b.magnitude());
    let twice = remainder << 1;
    if &twice > b.magnitude() || (&twice == b.magnitude() && quotient.bit(0)) {
        quotient += 1u8;
    }
    Ok(crate::Int::new(BigInt::from_biguint(
        a.sign() * b.sign(),
        quotient,
    )))
}

fn scale_delta(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let name = method.rsplit('.').next().unwrap();
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("descriptor '{name}' of 'datetime.timedelta' object needs an argument"),
        ));
    };
    let duration = calendar(ctx, receiver, "timedelta").ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' requires a 'datetime.timedelta' object but received a '{}'",
                ctx.type_name(receiver)
            ),
        )
    })?;
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("wrapper {name}() takes no keyword arguments"),
        ));
    }
    let &[factor] = args else {
        return Err(Error::new(
            "TypeError",
            format!("expected 1 argument, got {}", args.len()),
        ));
    };
    let us = ctx.alloc(Value::Int(crate::Int::new(
        duration.delta_microseconds().into(),
    )));
    let division = matches!(name, "__truediv__" | "__rtruediv__");
    if division && let Some(other) = calendar(ctx, factor, "timedelta") {
        let other = ctx.alloc(Value::Int(crate::Int::new(
            other.delta_microseconds().into(),
        )));
        return if name == "__rtruediv__" {
            ctx.binary("/", other, us)
        } else {
            ctx.binary("/", us, other)
        };
    }
    let micros = match ctx.heap.get(ctx.scalar_value(factor)) {
        _ if name == "__rtruediv__" => return Ok(crate::dynamic::value::NOT_IMPLEMENTED),
        Value::Int(_) | Value::Bool(_) if division => nearest_quotient(ctx, us, factor)?,
        Value::Int(_) | Value::Bool(_) => multiply_delta_integer(ctx, factor, us)?,
        Value::Float(_) => {
            let ratio = ctx.lookup_attr(factor, "as_integer_ratio")?;
            let pair = ctx.call(ratio, Vec::new(), Vec::new())?;
            let tuple = ctx.tuple_items(pair)?.ok_or_else(|| {
                Error::new("TypeError", format!(
                    "unexpected return type from as_integer_ratio(): expected tuple, not '{:.200}'",
                    ctx.type_name(pair)
                ))
            })?;
            let &[numerator, denominator] = tuple.as_slice() else {
                return Err(Error::new(
                    "ValueError",
                    "as_integer_ratio() must return a 2-tuple",
                ));
            };
            let (scale, divisor) = if division {
                (denominator, numerator)
            } else {
                (numerator, denominator)
            };
            let scaled = ctx.binary("*", us, scale)?;
            nearest_quotient(ctx, scaled, divisor)?
        }
        _ => return Ok(crate::dynamic::value::NOT_IMPLEMENTED),
    };
    Ok(store(
        ctx,
        Calendar::from_delta_microseconds(&micros)?,
        NONE,
    ))
}

pub(super) fn call(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if let Some((kind, operation @ ("__reduce__" | "__reduce_ex__"))) = method.split_once('.') {
        return reduction::reduce(ctx, kind, operation, args, kwargs);
    }
    if let Some(value) = reduction::from_state(ctx, method, args)? {
        return Ok(value);
    }
    if let Some(kind @ ("date" | "datetime" | "time" | "timedelta")) =
        method.strip_suffix(".__new__")
    {
        return new_subtype(ctx, kind, args, kwargs);
    }
    if let Some(field) = method.strip_prefix("_get.")
        && let Some((owner, name)) = field.rsplit_once('.')
        && let [receiver] = args
    {
        return read_field(ctx, owner, name, *receiver);
    }
    match method {
        "time" => time(ctx, args, kwargs),
        "timedelta.__mul__"
        | "timedelta.__rmul__"
        | "timedelta.__truediv__"
        | "timedelta.__rtruediv__" => scale_delta(ctx, method, args, kwargs),
        "datetime.combine" => {
            let [day, clock, tz] = bind("combine", args, kwargs, ["date", "time", "tzinfo"])?;
            let day = required(day, "combine()", "date", 1)?;
            let invalid = |ctx: &Context, id: Id, pos: usize, class: &str| {
                Error::new(
                    "TypeError",
                    format!(
                        "combine() argument {pos} must be datetime.{class}, not {}",
                        if id == NONE {
                            "None"
                        } else {
                            ctx.type_name(id)
                        }
                    ),
                )
            };
            let day = match ctx.heap.get(ctx.scalar_value(day)) {
                Value::Calendar { value, .. } if matches!(value.class(), "date" | "datetime") => {
                    value.clone()
                }
                _ => return Err(invalid(ctx, day, 1, "date")),
            };
            let clock = required(clock, "combine()", "time", 2)?;
            let (clock, clock_tz) = match ctx.heap.get(ctx.scalar_value(clock)) {
                Value::Calendar { value, tzinfo } if value.class() == "time" => {
                    (value.clone(), *tzinfo)
                }
                _ => return Err(invalid(ctx, clock, 2, "time")),
            };
            let tz = tz.unwrap_or(clock_tz);
            Ok(store(
                ctx,
                day.combine(&clock, zone(ctx, tz)?.as_ref())?,
                tz,
            ))
        }
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
        "datetime.now" => {
            let [tz] =
                crate::dynamic::builtins::bind_method_arguments("now", args, kwargs, ["tz"], 1)?;
            let tz = tz.unwrap_or(NONE);
            Ok(store(ctx, Calendar::now(zone(ctx, tz)?.as_ref())?, tz))
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
        "date.__format__" | "time.__format__" => {
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "__format__() takes no keyword arguments",
                ));
            }
            let [receiver, spec] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "__format__() takes exactly 1 argument ({} given)",
                        args.len().saturating_sub(1)
                    ),
                ));
            };
            let storage = ctx.string_storage(*spec).ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "__format__() argument 1 must be str, not {}",
                        if *spec == NONE {
                            "None"
                        } else {
                            ctx.type_name(*spec)
                        }
                    ),
                )
            })?;
            if ctx.truth(storage)? {
                let strftime = ctx.lookup_attr(*receiver, "strftime")?;
                return ctx.call(strftime, vec![*spec], vec![]);
            }
            let string = ctx.builtin("str");
            ctx.call(string, vec![*receiver], vec![])
        }
        "date.__repr__"
        | "datetime.__repr__"
        | "time.__repr__"
        | "timedelta.__repr__"
        | "date.__str__"
        | "datetime.__str__"
        | "time.__str__"
        | "timedelta.__str__"
        | "timedelta.__bool__"
        | "timedelta.__neg__"
        | "datetime.timetuple"
        | "date.timetuple"
        | "date.toordinal"
        | "datetime.toordinal"
        | "date.ctime"
        | "datetime.ctime"
        | "date.isoweekday"
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
                "__neg__" if value.delta_parts() == (0, 0, 0) => ctx.scalar_value(args[0]),
                "__neg__" => store(ctx, value.negate_delta()?, NONE),
                "__bool__" => {
                    if value.delta_parts() == (0, 0, 0) {
                        FALSE
                    } else {
                        TRUE
                    }
                }
                "__repr__" => {
                    let receiver_class = ctx.exact_type(args[0])?;
                    ctx.text(value.subtype_repr(ctx.class_name(receiver_class), class == "date"))
                }
                "__str__" => {
                    if class != "timedelta" {
                        let overridden = match *ctx.heap.get(args[0]) {
                            Value::Instance { class, .. } => {
                                ["isoformat", "__getattribute__", "__getattr__"]
                                    .iter()
                                    .any(|name| ctx.class_attr(class, name).is_some())
                            }
                            _ => false,
                        };
                        match ctx.lookup_attr(args[0], "isoformat") {
                            Ok(method) => {
                                let args = if class == "datetime" {
                                    vec![ctx.text(" ")]
                                } else {
                                    vec![]
                                };
                                return ctx.call(method, args, vec![]);
                            }
                            Err(error) if !overridden && ctx.is_attribute_error(&error) => {}
                            Err(error) => return Err(error),
                        }
                    }
                    ctx.text(if class == "date" {
                        value.iso_text()
                    } else {
                        value.text()
                    })
                }
                "total_seconds" => ctx.alloc(Value::Float(value.seconds())),
                "toordinal" => ctx.int(value.ordinal()),
                "ctime" => {
                    let mut fields = value.timetuple();
                    if class == "date" {
                        fields[3..6].fill(0);
                    }
                    ctx.text(super::time::ctime_text(fields, 4))
                }
                "isoweekday" => ctx.int((value.ordinal() - 1) % 7 + 1),
                "weekday" => ctx.int((value.ordinal() - 1) % 7),
                "timetuple" => {
                    let mut fields = value.timetuple();
                    if class == "date" {
                        // date.timetuple() ignores any clock stored on a datetime receiver.
                        fields[3..6].fill(0);
                    }
                    let values = fields.map(|n| ctx.int(n));
                    super::time::struct_time(ctx, values.into(), NONE, NONE)
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

/// Convert all integer-index operands before validating ranges and timezone, as in _datetime.
fn time(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let names = ["hour", "minute", "second", "microsecond", "tzinfo", "fold"];
    let count = args.len() + kwargs.len();
    if count > names.len() {
        return Err(Error::new(
            "TypeError",
            format!("function takes at most 6 arguments ({count} given)"),
        ));
    }
    if args.len() > 5 {
        return Err(Error::new(
            "TypeError",
            format!(
                "function takes at most 5 positional arguments ({} given)",
                args.len()
            ),
        ));
    }
    let mut fields = [0; 5];
    let mut tz = NONE;
    for (i, name) in names.iter().enumerate() {
        let value = args.get(i).copied().or_else(|| {
            kwargs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, id)| *id)
        });
        if let Some(id) = value {
            if *name == "tzinfo" {
                tz = id;
            } else {
                fields[if i == 5 { 4 } else { i }] = Calendar::component(&ctx.index_integer(id)?)?;
            }
        }
    }
    for (key, _) in kwargs {
        match names.iter().position(|name| name == key) {
            Some(i) if i < args.len() => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "argument for function given by name ('{key}') and position ({})",
                        i + 1
                    ),
                ));
            }
            None => {
                return Err(Error::new(
                    "TypeError",
                    format!("this function got an unexpected keyword argument '{key}'"),
                ));
            }
            _ => {}
        }
    }
    let local = Calendar::time_micros(fields)?;
    Ok(store(
        ctx,
        Calendar::time(local, fields[4], zone(ctx, tz)?.as_ref())?,
        tz,
    ))
}

/// Allocate the native payload for the requested calendar subtype before its initializer runs.
fn new_subtype(ctx: &mut Context, kind: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let owner = format!("datetime.{kind}");
    let Some((&class, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("{owner}.__new__(): not enough arguments"),
        ));
    };
    if !matches!(ctx.heap.get(class), Value::Class { .. })
        && ctx.builtin_type_name(class) != Some("type")
    {
        return Err(Error::new(
            "TypeError",
            format!(
                "{owner}.__new__(X): X is not a type object ({})",
                ctx.type_name(class)
            ),
        ));
    }
    let base = ctx.builtin(&owner);
    if !ctx.direct_subclass(class, base) {
        let name = ctx.class_name(class);
        return Err(Error::new(
            "TypeError",
            format!("{owner}.__new__({name}): {name} is not a subtype of {owner}"),
        ));
    }
    if kind == "date" && ctx.has_base(class, "datetime.datetime") {
        let name = ctx.class_name(class);
        return Err(Error::new(
            "TypeError",
            format!("{owner}.__new__({name}) is not safe, use {name}.__new__()"),
        ));
    }
    let value = call(ctx, kind, args, kwargs)?;
    if class == base {
        return Ok(value);
    }
    let object = ctx.instance(class, None);
    let Value::Instance { scalar, .. } = ctx.heap.get_mut(object) else {
        unreachable!()
    };
    *scalar = Some(value);
    Ok(object)
}
