//! Calendar values used by HTTP response dates and timed cookie signatures.
use num_bigint::{BigInt, Sign};
use num_traits::Euclid;

use super::*;
use crate::dynamic::builtins::bind;
use crate::objects::{Calendar, CalendarNumber};

pub(in crate::dynamic) mod comparison;
mod fromutc;
mod iso_time;
mod reduction;
pub(in crate::dynamic) mod render;
mod strftime;

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
const CLOCK_FIELDS: [&str; 6] = ["hour", "minute", "second", "microsecond", "tzinfo", "fold"];
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

pub(in crate::dynamic) const TZINFO: &str = "datetime.tzinfo";

pub(in crate::dynamic) fn clock_classmethod(symbol: &str) -> bool {
    matches!(
        symbol,
        "datetime.time.fromisoformat"
            | "datetime.time.strptime"
            | "datetime.datetime.now"
            | "datetime.datetime.fromtimestamp"
            | "datetime.datetime.utcnow"
            | "datetime.datetime.utcfromtimestamp"
            | "datetime.datetime.combine"
            | "datetime.datetime.strptime"
            | "datetime.date.today"
            | "datetime.date.fromtimestamp"
            | "datetime.date.fromordinal"
    )
}

pub(in crate::dynamic) fn tzinfo(ctx: &mut Context) -> Id {
    ctx.builtin(TZINFO)
}

pub(in crate::dynamic) fn members(class: &str) -> Option<Members> {
    let (owner, fields, methods): (_, &[_], &[_]) = match class {
        TZINFO => (
            TZINFO,
            &[],
            &["__reduce__", "tzname", "utcoffset", "dst", "fromutc"],
        ),
        "datetime.timezone" => (
            "datetime.timezone",
            &[],
            &[
                "__getinitargs__",
                "__repr__",
                "__str__",
                "utcoffset",
                "dst",
                "tzname",
                "fromutc",
            ],
        ),
        "datetime.date" => (
            "datetime.date",
            &FIELDS[..3],
            &[
                "__eq__",
                "__ne__",
                "__lt__",
                "__le__",
                "__gt__",
                "__ge__",
                "__format__",
                "__reduce__",
                "__repr__",
                "__str__",
                "ctime",
                "isoweekday",
                "toordinal",
                "weekday",
                "timetuple",
                "strftime",
                "fromordinal",
                "fromtimestamp",
                "today",
            ],
        ),
        "datetime.datetime" => (
            "datetime.datetime",
            &["hour", "minute", "second", "microsecond", "fold", "tzinfo"],
            &[
                "__add__",
                "__radd__",
                "__sub__",
                "__rsub__",
                "astimezone",
                "combine",
                "now",
                "fromtimestamp",
                "utcnow",
                "utcfromtimestamp",
                "__reduce__",
                "__reduce_ex__",
                "__eq__",
                "__ne__",
                "__lt__",
                "__le__",
                "__gt__",
                "__ge__",
                "dst",
                "tzname",
                "utcoffset",
                "__repr__",
                "__str__",
                "ctime",
                "date",
                "__replace__",
                "isoformat",
                "replace",
                "time",
                "timetz",
                "timestamp",
                "timetuple",
                "utctimetuple",
                "strptime",
            ],
        ),
        "datetime.time" => (
            "datetime.time",
            &["hour", "minute", "second", "microsecond", "fold", "tzinfo"],
            &[
                "__format__",
                "__replace__",
                "fromisoformat",
                "strptime",
                "isoformat",
                "replace",
                "__reduce__",
                "__reduce_ex__",
                "__eq__",
                "__ne__",
                "__lt__",
                "__le__",
                "__gt__",
                "__ge__",
                "dst",
                "tzname",
                "utcoffset",
                "__repr__",
                "__str__",
                "strftime",
            ],
        ),
        "datetime.timedelta" => (
            "datetime.timedelta",
            &["days", "seconds", "microseconds"],
            &[
                "__bool__",
                "__pos__",
                "__abs__",
                "__neg__",
                "__add__",
                "__radd__",
                "__sub__",
                "__rsub__",
                "__mul__",
                "__rmul__",
                "__truediv__",
                "__rtruediv__",
                "__floordiv__",
                "__rfloordiv__",
                "__mod__",
                "__rmod__",
                "__divmod__",
                "__rdivmod__",
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
            || (self.owner == "datetime.timezone" && name == "utc")
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
    if class == "datetime.timezone" {
        ctx.builtin(TZINFO);
        utc(ctx);
        for (name, max) in [("min", false), ("max", true)] {
            let id = store(ctx, Calendar::zone_limit(max), NONE);
            ctx.builtins.insert(format!("{class}.{name}"), id);
        }
    } else {
        ctx.builtin(&format!("{class}.__new__"));
    }
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
    // A native-created zone (UTC or a parsed mail date) already owns its timedelta.
    let tzoffset = value
        .zone_offset()
        .map_or(NONE, |offset| store(ctx, offset, NONE));
    ctx.alloc(Value::Calendar {
        value,
        tzinfo,
        tzoffset,
        tzname: NONE,
    })
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

pub(in crate::dynamic) fn type_name(ctx: &Context, value: Id) -> std::borrow::Cow<'_, str> {
    if let Value::Calendar { value, .. } = ctx.heap.get(value) {
        return format!("datetime.{}", value.class()).into();
    }
    ctx.heap
        .get(value)
        .instance_class()
        .map(|class| ctx.class_name(class))
        .or_else(|| ctx.builtin_type_name(value))
        .unwrap_or_else(|| ctx.type_name(value))
        .into()
}

/// Return a fixed zone's stored offset or name after validating its datetime argument.
fn fixed_zone_method(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let name = method.rsplit('.').next().unwrap();
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method {method}() needs an argument"),
        ));
    };
    let (value, offset, name_object) = match ctx.heap.get(receiver) {
        Value::Calendar {
            value,
            tzoffset,
            tzname,
            ..
        } if *tzoffset != NONE => (value, *tzoffset, *tzname),
        _ => {
            let kind = type_name(ctx, receiver);
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{name}' for 'datetime.timezone' objects doesn't apply to a '{kind}' object"
                ),
            ));
        }
    };
    let dt = crate::dynamic::builtins::one_positional_arg(method, rest, kwargs)?;
    if dt != NONE && calendar(ctx, dt, "datetime").is_none() {
        let kind = type_name(ctx, dt);
        return Err(Error::new(
            "TypeError",
            format!("{name}(dt) argument must be a datetime instance or None, not {kind}"),
        ));
    }
    Ok(match name {
        "utcoffset" => offset,
        "dst" => NONE,
        _ if name_object != NONE => name_object,
        _ => ctx.text(value.text()),
    })
}

fn zone(ctx: &Context, id: Id) -> Result<Option<Calendar>> {
    // Custom zones live in the boxed clock's tzinfo field. Its native payload stores wall time.
    if id == NONE
        || ctx
            .heap
            .get(id)
            .instance_class()
            .is_some_and(|class| ctx.has_base(class, TZINFO))
    {
        return Ok(None);
    }
    calendar(ctx, id, "timezone").map(Some).ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "tzinfo argument must be None or of a tzinfo subclass, not type '{}'",
                type_name(ctx, id)
            ),
        )
    })
}

/// Call the zone each time its name or offset is requested and preserve the returned Python object.
pub(in crate::dynamic) fn call_tzinfo(
    ctx: &mut Context,
    zone: Id,
    method: &str,
    arg: Id,
) -> Result<Id> {
    if zone == NONE {
        return Ok(NONE);
    }
    let callback = ctx.lookup_attr(zone, method)?;
    let result = ctx.call(callback, vec![arg], vec![])?;
    if result == NONE {
        return Ok(NONE);
    }
    let expected = if method == "tzname" {
        if ctx.string_storage(result).is_some() {
            return Ok(result);
        }
        "a string"
    } else {
        if let Some(offset) = calendar(ctx, result, "timedelta") {
            if offset.delta_microseconds().abs() < 86_400_000_000 {
                return Ok(result);
            }
            let repr = ctx.builtin("repr");
            let desc = ctx.call(repr, vec![result], vec![])?;
            return Err(Error::new(
                "ValueError",
                format!(
                    "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), not {}",
                    ctx.str_value(desc)?
                ),
            ));
        }
        "timedelta"
    };
    Err(Error::new(
        "TypeError",
        format!(
            "tzinfo.{method}() must return None or {expected}, not '{}'",
            type_name(ctx, result)
        ),
    ))
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
    let Value::Calendar { value, tzinfo, .. } = ctx.heap.get(ctx.scalar_value(receiver)) else {
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

fn c_int(ctx: &mut Context, value: Id) -> Result<i32> {
    ctx.index_integer(value)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

/// Split a custom numeric result with divmod and validate the normalized remainder.
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
    let remainder = c_int(ctx, *remainder)?;
    if !(0..unit).contains(&remainder) {
        return Err(Error::new(
            "TypeError",
            "divmod() returned a value out of range",
        ));
    }
    Ok((*quotient, remainder))
}

/// Convert the microseconds returned by integer multiplication or floor division.
fn result_microseconds(ctx: &mut Context, result: Id) -> Result<crate::Int> {
    if let Value::Int(n) = ctx.heap.get(result) {
        Ok(n.clone())
    } else {
        let (seconds, micros) = split_delta_component(ctx, result, 1_000_000)?;
        let (days, seconds) = split_delta_component(ctx, seconds, 86_400)?;
        let days = c_int(ctx, days)?;
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

fn datetime_arithmetic(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("descriptor '{name}' of 'datetime.datetime' object needs an argument"),
        ));
    };
    let scalar = ctx.scalar_value(receiver);
    let (value, tzinfo) = match ctx.heap.get(scalar) {
        Value::Calendar { value, tzinfo, .. } if value.class() == "datetime" => {
            (value.clone(), *tzinfo)
        }
        _ => {
            let kind = match ctx.heap.get(receiver) {
                Value::Calendar { value, .. } => format!("datetime.{}", value.class()),
                _ => ctx.type_name(receiver).to_owned(),
            };
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{name}' requires a 'datetime.datetime' object but received a '{kind}'"
                ),
            ));
        }
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("wrapper {name}() takes no keyword arguments"),
        ));
    }
    let &[other] = args else {
        return Err(Error::new(
            "TypeError",
            format!("expected 1 argument, got {}", args.len()),
        ));
    };
    if matches!(name, "__sub__" | "__rsub__")
        && let Some(other_value) = calendar(ctx, other, "datetime")
    {
        let (a, b) = if name == "__sub__" {
            (receiver, other)
        } else {
            (other, receiver)
        };
        if let Some(delta) = comparison::subtract(ctx, a, b)? {
            return Ok(store(ctx, delta, NONE));
        }
        let other = other_value;
        let result = if name == "__sub__" {
            value.datetime_difference(&other)?
        } else {
            other.datetime_difference(&value)?
        };
        return Ok(store(ctx, result, NONE));
    }
    let Some(delta) = calendar(ctx, other, "timedelta") else {
        return Ok(crate::dynamic::value::NOT_IMPLEMENTED);
    };
    if name == "__rsub__" {
        return Ok(crate::dynamic::value::NOT_IMPLEMENTED);
    }
    let shifted = value.shift(&delta, name == "__sub__")?;
    if receiver == scalar {
        return Ok(store(ctx, shifted, tzinfo));
    }
    // CPython calls the subclass's constructor with all seven fields and the original tzinfo.
    let class = ctx.exact_type(receiver)?;
    let fields = shifted.fields();
    let mut args: Vec<_> = fields[..7]
        .iter()
        .map(|&part| ctx.int(part as i64))
        .collect();
    args.push(tzinfo);
    ctx.call(class, args, Vec::new())
}

fn delta_arithmetic(
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
    if matches!(name, "__neg__" | "__pos__" | "__abs__") {
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("expected 0 arguments, got {}", args.len()),
            ));
        }
        let micros = duration.delta_microseconds();
        if micros == 0 {
            return Ok(ctx.scalar_value(receiver));
        }
        let result = if name == "__neg__" || (name == "__abs__" && micros < 0) {
            duration.negate_delta()?
        } else {
            duration
        };
        return Ok(store(ctx, result, NONE));
    }
    let &[other] = args else {
        return Err(Error::new(
            "TypeError",
            format!("expected 1 argument, got {}", args.len()),
        ));
    };
    if matches!(name, "__add__" | "__radd__" | "__sub__" | "__rsub__") {
        let Some(other) = calendar(ctx, other, "timedelta") else {
            return Ok(crate::dynamic::value::NOT_IMPLEMENTED);
        };
        let result = match name {
            "__sub__" => duration.shift(&other, true),
            "__rsub__" => other.shift(&duration, true),
            _ => duration.shift(&other, false),
        }?;
        return Ok(store(ctx, result, NONE));
    }
    if matches!(name, "__mod__" | "__rmod__" | "__divmod__" | "__rdivmod__") {
        let Some(other) = calendar(ctx, other, "timedelta") else {
            return Ok(crate::dynamic::value::NOT_IMPLEMENTED);
        };
        let as_int = |value: &Calendar| crate::Int::new(value.delta_microseconds().into());
        let (left, right) = if matches!(name, "__rmod__" | "__rdivmod__") {
            (as_int(&other), as_int(&duration))
        } else {
            (as_int(&duration), as_int(&other))
        };
        let micros = crate::integer_binary(&left, &right, 4)?;
        let remainder = store(ctx, Calendar::from_delta_microseconds(&micros)?, NONE);
        if matches!(name, "__divmod__" | "__rdivmod__") {
            let quotient = ctx.alloc(Value::Int(crate::integer_binary(&left, &right, 3)?));
            return Ok(ctx.alloc(Value::Tuple(vec![quotient, remainder])));
        }
        return Ok(remainder);
    }
    scale_delta(ctx, name, &duration, other)
}

fn scale_delta(ctx: &mut Context, name: &str, duration: &Calendar, factor: Id) -> Result<Id> {
    let us = ctx.alloc(Value::Int(crate::Int::new(
        duration.delta_microseconds().into(),
    )));
    let op = match name {
        "__truediv__" | "__rtruediv__" => "/",
        "__floordiv__" | "__rfloordiv__" => "//",
        _ => "*",
    };
    let reflected_division = matches!(name, "__rtruediv__" | "__rfloordiv__");
    if op != "*"
        && let Some(other) = calendar(ctx, factor, "timedelta")
    {
        let other = ctx.alloc(Value::Int(crate::Int::new(
            other.delta_microseconds().into(),
        )));
        return if reflected_division {
            ctx.binary(op, other, us)
        } else {
            ctx.binary(op, us, other)
        };
    }
    let micros = match ctx.heap.get(ctx.scalar_value(factor)) {
        _ if reflected_division => return Ok(crate::dynamic::value::NOT_IMPLEMENTED),
        Value::Int(_) | Value::Bool(_) if op == "/" => nearest_quotient(ctx, us, factor)?,
        Value::Int(_) | Value::Bool(_) => {
            let result = if op == "//" {
                ctx.binary("//", us, factor)?
            } else {
                ctx.binary("*", factor, us)?
            };
            result_microseconds(ctx, result)?
        }
        Value::Float(_) if op != "//" => {
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
            let (scale, divisor) = if op == "/" {
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
    if let Some((
        kind @ ("date" | "datetime" | "time"),
        operation @ ("__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__"),
    )) = method.split_once('.')
    {
        return comparison::rich_method(ctx, kind, operation, args, kwargs);
    }
    if let Some((kind, operation)) = method.split_once('.')
        && matches!(
            (kind, operation),
            ("tzinfo", "__reduce__") | ("timezone", "__getinitargs__")
        )
    {
        return reduction::reduce_zone(ctx, kind, operation, args, kwargs);
    }
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
        "tzinfo" => {
            let class = tzinfo(ctx);
            Ok(ctx.instance(class, None))
        }
        "tzinfo.__new__" => {
            let Some(&class) = args.first() else {
                return Err(Error::new(
                    "TypeError",
                    "datetime.tzinfo.__new__(): not enough arguments",
                ));
            };
            let base = tzinfo(ctx);
            if !ctx.direct_subclass(class, base) {
                return Err(ctx.invalid_new_subtype(TZINFO, class));
            }
            let object = ctx.builtin("object");
            let owner = ctx.tzinfo_allocation_base(class, base, object);
            let storage = match owner.map(|id| ctx.heap.get(id)) {
                Some(Value::Native(kind)) if kind == TZINFO || kind == "property" => None,
                Some(Value::Native(kind)) if kind == "list" => {
                    Some(ctx.alloc(Value::List(Vec::new())))
                }
                Some(Value::Native(kind)) if kind == "bytearray" => {
                    Some(ctx.alloc(Value::ByteArray(Vec::new())))
                }
                _ => {
                    let name = ctx.class_name(class);
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "datetime.tzinfo.__new__({name}) is not safe, use {name}.__new__()"
                        ),
                    ));
                }
            };
            let result = ctx.instance(class, None);
            if let Value::Instance { scalar, .. } = ctx.heap.get_mut(result) {
                *scalar = storage;
            }
            Ok(result)
        }
        "tzinfo.fromutc" | "timezone.fromutc" => fromutc::method(ctx, method, args, kwargs),
        "datetime.astimezone" => fromutc::astimezone(ctx, args, kwargs),
        "timezone.utcoffset" | "timezone.dst" | "timezone.tzname" => {
            fixed_zone_method(ctx, method, args, kwargs)
        }
        "tzinfo.utcoffset" | "tzinfo.dst" | "tzinfo.tzname" => {
            let name = method.rsplit('.').next().unwrap();
            let Some((&receiver, args)) = args.split_first() else {
                return Err(Error::new(
                    "TypeError",
                    format!("unbound method {method}() needs an argument"),
                ));
            };
            let class = tzinfo(ctx);
            if !ctx.instance_of(receiver, class) {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "descriptor '{name}' for '{TZINFO}' objects doesn't apply to a '{}' object",
                        ctx.type_name(receiver)
                    ),
                ));
            }
            let kind = ctx.type_name(receiver);
            let kind = kind.strip_prefix("datetime.").unwrap_or(kind);
            crate::dynamic::builtins::one_positional_arg(&format!("{kind}.{name}"), args, kwargs)?;
            Err(Error::new(
                "NotImplementedError",
                format!("a tzinfo subclass must implement {name}()"),
            ))
        }
        "datetime.__add__" | "datetime.__radd__" | "datetime.__sub__" | "datetime.__rsub__" => {
            datetime_arithmetic(ctx, &method["datetime.".len()..], args, kwargs)
        }
        "date.today" | "date.fromtimestamp" => fromutc::date_constructor(ctx, method, args, kwargs),
        "date.fromordinal" => date_fromordinal(ctx, args, kwargs),
        "date.strftime" => strftime::format(ctx, "date", args, kwargs),
        "time.strftime" => strftime::format(ctx, "time", args, kwargs),
        "time" => time(ctx, args, kwargs),
        "time.fromisoformat" => time_fromisoformat(ctx, args, kwargs),
        "time.strptime" => time_strptime(ctx, args, kwargs),
        "datetime.strptime" => datetime_strptime(ctx, args, kwargs),
        "datetime.date"
        | "datetime.time"
        | "datetime.timetz"
        | "datetime.utctimetuple"
        | "datetime.replace"
        | "datetime.__replace__"
        | "time.replace"
        | "time.__replace__"
        | "datetime.utcoffset"
        | "time.utcoffset"
        | "datetime.dst"
        | "time.dst"
        | "datetime.tzname"
        | "time.tzname" => clock_method(ctx, method, args, kwargs),
        "timedelta.__neg__"
        | "timedelta.__pos__"
        | "timedelta.__abs__"
        | "timedelta.__add__"
        | "timedelta.__radd__"
        | "timedelta.__sub__"
        | "timedelta.__rsub__"
        | "timedelta.__mul__"
        | "timedelta.__rmul__"
        | "timedelta.__truediv__"
        | "timedelta.__rtruediv__"
        | "timedelta.__floordiv__"
        | "timedelta.__rfloordiv__"
        | "timedelta.__mod__"
        | "timedelta.__rmod__"
        | "timedelta.__divmod__"
        | "timedelta.__rdivmod__" => delta_arithmetic(ctx, method, args, kwargs),
        "datetime.combine" => {
            let (&class, args) = args
                .split_first()
                .ok_or_else(|| Error::new("TypeError", "datetime.combine requires a type"))?;
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
                Value::Calendar { value, tzinfo, .. } if value.class() == "time" => {
                    (value.clone(), *tzinfo)
                }
                _ => return Err(invalid(ctx, clock, 2, "time")),
            };
            let tz = tz.unwrap_or(clock_tz);
            let value = day.combine(&clock, zone(ctx, tz)?.as_ref())?;
            fromutc::result_datetime(ctx, class, value, tz)
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
            let tzname = name.unwrap_or(NONE);
            if let Some(name) = name
                && ctx.string_storage(name).is_none()
            {
                let kind = if name == NONE { "None" } else { ctx.type_name(name) };
                return Err(Error::new(
                    "TypeError",
                    format!("timezone() argument 2 must be str, not {kind}"),
                ));
            }
            // Native calendar diagnostics can also retain names representable as UTF-8.
            let native_name = name.and_then(|id| ctx.str_value(id).ok());
            let zone = Calendar::zone(&offset, native_name)?;
            Ok(if tzname == NONE && zone.is_utc() {
                utc(ctx)
            } else {
                ctx.alloc(Value::Calendar {
                    value: zone,
                    tzinfo: NONE,
                    tzoffset: id,
                    tzname,
                })
            })
        }
        "datetime.now"
        | "datetime.fromtimestamp"
        | "datetime.utcnow"
        | "datetime.utcfromtimestamp" => fromutc::constructor(ctx, method, args, kwargs),
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
            if method == "datetime" {
                return new_datetime(ctx, fields, tz);
            }
            Ok(store(
                ctx,
                Calendar::date(fields[0], fields[1], fields[2])?,
                NONE,
            ))
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
        "datetime.isoformat" | "time.isoformat" => render::isoformat(ctx, method, args, kwargs),
        "timezone.__repr__"
        | "timezone.__str__"
        | "date.__repr__"
        | "datetime.__repr__"
        | "time.__repr__"
        | "timedelta.__repr__"
        | "date.__str__"
        | "datetime.__str__"
        | "time.__str__"
        | "timedelta.__str__"
        | "timedelta.__bool__"
        | "datetime.timestamp"
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
                "__bool__" => {
                    if value.delta_parts() == (0, 0, 0) {
                        FALSE
                    } else {
                        TRUE
                    }
                }
                "__repr__" => {
                    let receiver_class = ctx.exact_type(args[0])?;
                    render::repr(ctx, args[0], &value, receiver_class, class)?
                }
                "__str__" if class == "timezone" => {
                    let Value::Calendar { tzname, .. } = *ctx.heap.get(args[0]) else {
                        unreachable!()
                    };
                    if tzname == NONE { ctx.text(value.text()) } else { tzname }
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
                "timestamp" => {
                    let seconds = render::timestamp(ctx, args[0], &value)?;
                    ctx.alloc(Value::Float(seconds))
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
                    if class == "datetime" {
                        fields[8] = render::isdst(ctx, args[0])?;
                    }
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
    let names = CLOCK_FIELDS;
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
    new_time(ctx, fields, tz)
}

fn new_time(ctx: &mut Context, fields: [i32; 5], tz: Id) -> Result<Id> {
    let local = Calendar::time_micros(fields)?;
    let native_zone = zone(ctx, tz)?;
    Ok(store(
        ctx,
        Calendar::time(local, fields[4], native_zone.as_ref())?,
        tz,
    ))
}

fn calendar_class_args<'a>(
    ctx: &mut Context,
    owner: &'static str,
    method: &str,
    args: &'a [Id],
) -> Result<(Id, &'a [Id])> {
    let Some((&class, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("descriptor '{method}' of '{owner}' object needs an argument"),
        ));
    };
    let base = ctx.builtin(owner);
    if !ctx.is_type_object(class) {
        let kind = ctx.exact_type(class)?;
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{method}' for type '{owner}' needs a type, not a '{}' as arg 2",
                ctx.class_name(kind)
            ),
        ));
    }
    if !ctx.direct_subclass(class, base) {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{method}' requires a subtype of '{owner}' but received '{}'",
                ctx.class_name(class)
            ),
        ));
    }
    Ok((class, args))
}

fn date_fromordinal(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (class, args) = calendar_class_args(ctx, "datetime.date", "fromordinal", args)?;
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "fromordinal() takes no keyword arguments",
        ));
    }
    let &[ordinal] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "fromordinal() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    };
    let fields = Calendar::ordinal_fields(ctx.index_c_int(ordinal)?)?;
    let args = fields.into_iter().map(|n| ctx.int(i64::from(n))).collect();
    ctx.call(class, args, vec![])
}

fn strptime_args(
    ctx: &mut Context,
    owner: &'static str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<(Id, Id, Id)> {
    let (class, args) = calendar_class_args(ctx, owner, "strptime", args)?;
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "strptime() takes no keyword arguments",
        ));
    }
    let &[text, format] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "strptime() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    };
    for (i, &arg) in args.iter().enumerate() {
        if ctx.string_storage(arg).is_none() {
            let kind = if arg == NONE {
                "None".into()
            } else {
                type_name(ctx, arg)
            };
            return Err(Error::new(
                "TypeError",
                format!("strptime() argument {} must be str, not {kind}", i + 1),
            ));
        }
    }

    Ok((class, text, format))
}

fn datetime_strptime(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (class, text, format) = strptime_args(ctx, "datetime.datetime", args, kwargs)?;
    let module_mark = ctx.heap.temporary_mark();
    let module = ctx.import_with_builtins("_strptime")?;
    let parser_mark = ctx.heap.temporary_mark();
    let result = ctx
        .lookup_attr(module, "_strptime_datetime_datetime")
        .and_then(|parser| {
            ctx.heap.keep(parser);
            ctx.call(parser, vec![class, text, format], vec![])
        });
    // Release the callable before its module, preserving the result or exception across finalizers.
    for mark in [parser_mark, module_mark] {
        let collect = ctx.heap.discard_truth_operands(mark, ctx.modules);
        match &result {
            Ok(value) => ctx.heap.keep(*value),
            Err(error) => {
                for value in error.roots() {
                    ctx.heap.keep(value);
                }
            }
        }
        if collect {
            ctx.collect_unreachable();
        }
    }
    result
}

fn time_strptime(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (class, text, format) = strptime_args(ctx, "datetime.time", args, kwargs)?;
    let module = ctx.module("_strptime")?;
    let parser = ctx.lookup_attr(module, "_strptime_datetime_time")?;
    ctx.call(parser, vec![class, text, format], vec![])
}

fn time_fromisoformat(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (class, args) = calendar_class_args(ctx, "datetime.time", "fromisoformat", args)?;
    let name = ctx.class_qualname(class);
    let input = crate::dynamic::builtins::one_positional_arg(
        &format!("{name}.fromisoformat"),
        args,
        kwargs,
    )?;
    let Some(storage) = ctx.string_storage(input) else {
        return Err(Error::new(
            "TypeError",
            "fromisoformat: argument must be str",
        ));
    };
    let Some(([mut hour, minute, second, micros], offset)) =
        ctx.str_value(storage).ok().and_then(iso_time::parse)
    else {
        let repr = crate::dynamic::builtins::call(ctx, "repr", &[input], &[])?;
        return Err(Error::new(
            "ValueError",
            format!("Invalid isoformat string: {}", ctx.str_value(repr)?),
        ));
    };
    if hour == 24 {
        if (minute, second, micros) != (0, 0, 0) {
            return Err(Error::new(
                "ValueError",
                "minute, second, and microsecond must be 0 when hour is 24",
            ));
        }
        hour = 0;
    }
    let zone = match offset {
        None => NONE,
        Some(0) => utc(ctx),
        Some(us) => {
            let offset = Calendar::from_delta_microseconds(&crate::Int::from(us))?;
            store(ctx, Calendar::zone(&offset, None)?, NONE)
        }
    };
    time_for_class(ctx, class, [hour, minute, second, micros, 0], zone)
}

/// Read native civil fields even when the receiver overrides their descriptors.
fn clock_method(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (kind, operation) = method.split_once('.').unwrap();
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method {method}() needs an argument"),
        ));
    };
    let (value, tz) = match ctx.heap.get(ctx.scalar_value(receiver)) {
        Value::Calendar { value, tzinfo, .. } if value.class() == kind => (value.clone(), *tzinfo),
        _ => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{operation}' for 'datetime.{kind}' objects doesn't apply to a '{}' object",
                    type_name(ctx, receiver)
                ),
            ));
        }
    };
    if !matches!(operation, "replace" | "__replace__") {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{method}() takes no keyword arguments"),
            ));
        }
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{method}() takes no arguments ({} given)", rest.len()),
            ));
        }
        return match operation {
            "utctimetuple" => {
                let mark = ctx.heap.temporary_mark();
                let offset = call_tzinfo(ctx, tz, "utcoffset", receiver)?;
                let utc = if offset == NONE {
                    value
                } else {
                    // Subtract through the native slot, including Python subtype construction.
                    let shifted = datetime_arithmetic(ctx, "__sub__", &[receiver, offset], &[])?;
                    ctx.finish_call_operands(mark, shifted);
                    calendar(ctx, shifted, "datetime").ok_or_else(|| {
                        Error::new(
                            "TypeError",
                            "datetime subtype constructor did not return a datetime",
                        )
                    })?
                };
                let mut fields = utc.timetuple();
                fields[8] = 0;
                ctx.finish_call_operands(mark, NONE);
                let time = ctx.module("time")?;
                let factory = ctx.lookup_attr(time, "struct_time")?;
                let values = fields.map(|n| ctx.int(n));
                let fields = ctx.alloc(Value::Tuple(values.into()));
                ctx.call(factory, vec![fields], Vec::new())
            }
            "date" => {
                let [y, m, d, ..] = value.fields();
                Ok(store(
                    ctx,
                    Calendar::date(y as i32, m as i32, d as i32)?,
                    NONE,
                ))
            }
            "time" => Ok(store(ctx, value.time_of_day()?, NONE)),
            "timetz" => {
                let [_, _, _, hour, minute, second, micros, fold] = value.fields();
                new_time(
                    ctx,
                    [hour, minute, second, micros, fold].map(|n| n as i32),
                    tz,
                )
            }
            _ => call_tzinfo(
                ctx,
                tz,
                operation,
                if kind == "time" { NONE } else { receiver },
            ),
        };
    }
    if kind == "datetime" {
        let mut fields = value.fields().map(|n| n as i32);
        let [year, month, day, hour, minute, second, micros, zone, fold] =
            crate::dynamic::builtins::bind_method_arguments(
                "replace",
                rest,
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
                8,
            )?;
        for (field, arg) in fields
            .iter_mut()
            .zip([year, month, day, hour, minute, second, micros, fold])
        {
            if let Some(id) = arg {
                *field = c_int(ctx, id)?;
            }
        }
        let class = ctx.exact_type(receiver)?;
        return datetime_replace_fields(ctx, class, fields, zone.unwrap_or(tz));
    }
    let mut fields = value.clock_parts().map(|n| n as i32);
    let [hour, minute, second, microsecond, tzinfo, fold] =
        crate::dynamic::builtins::bind_method_arguments("replace", rest, kwargs, CLOCK_FIELDS, 5)?;
    for (field, replacement) in fields
        .iter_mut()
        .zip([hour, minute, second, microsecond, fold])
    {
        if let Some(id) = replacement {
            *field = c_int(ctx, id)?;
        }
    }
    let tz = tzinfo.unwrap_or(tz);
    let class = ctx.exact_type(receiver)?;
    time_for_class(ctx, class, fields, tz)
}

fn time_for_class(ctx: &mut Context, class: Id, fields: [i32; 5], tz: Id) -> Result<Id> {
    if class == ctx.builtin("datetime.time") {
        return new_time(ctx, fields, tz);
    }
    clock_subclass(ctx, class, &fields[..4], tz, fields[4])
}

fn new_datetime(ctx: &mut Context, fields: [i32; 8], tz: Id) -> Result<Id> {
    let zone = zone(ctx, tz);
    // CPython reports civil-field errors before an invalid zone.
    let value = Calendar::datetime(fields, zone.as_ref().ok().and_then(Option::as_ref))?;
    zone?;
    Ok(store(ctx, value, tz))
}

/// Construct a datetime from civil fields, calling a Python subtype with the public arguments.
pub(super) fn datetime_replace_fields(
    ctx: &mut Context,
    class: Id,
    fields: [i32; 8],
    tz: Id,
) -> Result<Id> {
    if class == ctx.builtin("datetime.datetime") {
        return new_datetime(ctx, fields, tz);
    }
    clock_subclass(ctx, class, &fields[..7], tz, fields[7])
}

fn clock_subclass(ctx: &mut Context, class: Id, fields: &[i32], tz: Id, fold: i32) -> Result<Id> {
    let mut args: Vec<_> = fields.iter().map(|&n| ctx.int(i64::from(n))).collect();
    args.push(tz);
    // Older subtype constructors may predate the keyword-only fold argument.
    let kwargs = if fold == 0 {
        vec![]
    } else {
        vec![("fold".into(), ctx.int(i64::from(fold)))]
    };
    ctx.call(class, args, kwargs)
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
