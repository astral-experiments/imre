//! Convert UTC civil fields through a timezone's Python fromutc protocol.
use super::*;

fn invoke(ctx: &mut Context, tz: Id, dt: Id) -> Result<Id> {
    ctx.heap.keep(dt);
    let callback = ctx.lookup_attr(tz, "fromutc")?;
    ctx.call(callback, vec![dt], vec![])
}

pub(super) fn constructor(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&class, args) = args
        .split_first()
        .ok_or_else(|| Error::new("TypeError", "datetime class method requires a type"))?;
    if matches!(method, "datetime.utcnow" | "datetime.utcfromtimestamp") {
        return deprecated(ctx, class, method, args, kwargs);
    }
    let (stamp, tz) = if method == "datetime.now" {
        let [tz] = crate::dynamic::builtins::bind_method_arguments("now", args, kwargs, ["tz"], 1)?;
        (None, tz)
    } else {
        let [stamp, tz] = bind("fromtimestamp", args, kwargs, ["timestamp", "tz"])?;
        (
            Some(required(stamp, "fromtimestamp()", "timestamp", 1)?),
            tz,
        )
    };
    let tz = tz.unwrap_or(NONE);
    let zone = zone(ctx, tz)?;
    let stamp = stamp
        .map(|id| {
            number(ctx, id).ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!("argument must be int or float, not {}", ctx.type_name(id)),
                )
            })
        })
        .transpose()?;
    instant(ctx, class, stamp.as_ref(), tz, zone.as_ref(), false)
}

fn instant(
    ctx: &mut Context,
    class: Id,
    timestamp: Option<&CalendarNumber>,
    tz: Id,
    zone: Option<&Calendar>,
    naive_utc: bool,
) -> Result<Id> {
    let custom = tz != NONE && zone.is_none();
    let utc = (custom || naive_utc).then(Calendar::utc);
    let basis = zone.or(utc.as_ref());
    let value = match timestamp {
        Some(stamp) => Calendar::from_timestamp(stamp, basis)?,
        None => Calendar::now(basis)?,
    };
    if utc.is_none() {
        return result_datetime(ctx, class, value, tz);
    }
    let dt = result_datetime(ctx, class, value.with_datetime_zone(None)?, tz)?;
    if custom { invoke(ctx, tz, dt) } else { Ok(dt) }
}

fn deprecated(
    ctx: &mut Context,
    class: Id,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let now = method == "datetime.utcnow";
    if !kwargs.is_empty() {
        let name = if now {
            "datetime.utcnow"
        } else {
            "utcfromtimestamp"
        };
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    if now && !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!(
                "datetime.utcnow() takes no arguments ({} given)",
                args.len()
            ),
        ));
    }
    let message = if now {
        "datetime.datetime.utcnow() is deprecated and scheduled for removal in a future version. Use timezone-aware objects to represent datetimes in UTC: datetime.datetime.now(datetime.UTC)."
    } else {
        "datetime.datetime.utcfromtimestamp() is deprecated and scheduled for removal in a future version. Use timezone-aware objects to represent datetimes in UTC: datetime.datetime.fromtimestamp(timestamp, datetime.UTC)."
    };
    ctx.emit_builtin_warning(
        crate::objects::exceptions::Kind::DeprecationWarning,
        message,
        1,
    )?;
    let timestamp = if now {
        None
    } else {
        let [id] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "utcfromtimestamp() takes exactly 1 argument ({} given)",
                    args.len()
                ),
            ));
        };
        Some(number(ctx, *id).ok_or_else(|| {
            Error::new(
                "TypeError",
                format!("argument must be int or float, not {}", ctx.type_name(*id)),
            )
        })?)
    };
    instant(ctx, class, timestamp.as_ref(), NONE, None, true)
}

pub(super) fn result_datetime(ctx: &mut Context, class: Id, value: Calendar, tz: Id) -> Result<Id> {
    if class == ctx.builtin("datetime.datetime") {
        return Ok(store(ctx, value, tz));
    }
    datetime_replace_fields(ctx, class, value.fields().map(|n| n as i32), tz)
}

fn offset(ctx: &mut Context, zone: Id, method: &str, dt: Id, missing: &str) -> Result<Calendar> {
    let value = call_tzinfo(ctx, zone, method, dt)?;
    calendar(ctx, value, "timedelta").ok_or_else(|| Error::new("ValueError", missing))
}

pub(super) fn method(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&tz, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method {method}() needs an argument"),
        ));
    };
    let fixed = method == "timezone.fromutc";
    let owner = if fixed { "datetime.timezone" } else { TZINFO };
    let class = ctx.builtin(owner);
    let actual = ctx.exact_type(tz)?;
    if !ctx.direct_subclass(actual, class) || (fixed && calendar(ctx, tz, "timezone").is_none()) {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor 'fromutc' for '{owner}' objects doesn't apply to a '{}' object",
                ctx.type_name(tz)
            ),
        ));
    }
    let dt = crate::dynamic::builtins::one_positional_arg(method, rest, kwargs)?;
    let (value, stored) = match ctx.heap.get(ctx.scalar_value(dt)) {
        Value::Calendar { value, tzinfo, .. } if value.class() == "datetime" => {
            (value.clone(), *tzinfo)
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                "fromutc: argument must be a datetime",
            ));
        }
    };
    if stored != tz {
        return Err(Error::new("ValueError", "fromutc: dt.tzinfo is not self"));
    }
    let dt_class = ctx.exact_type(dt)?;
    if fixed {
        let Value::Calendar { tzoffset, .. } = ctx.heap.get(ctx.scalar_value(tz)) else {
            unreachable!()
        };
        let offset = calendar(ctx, *tzoffset, "timedelta").expect("fixed zone offset");
        return result_datetime(ctx, dt_class, value.shift(&offset, false)?, tz);
    }
    let off = offset(
        ctx,
        tz,
        "utcoffset",
        dt,
        "fromutc: non-None utcoffset() result required",
    )?;
    let dst = offset(
        ctx,
        tz,
        "dst",
        dt,
        "fromutc: non-None dst() result required",
    )?;
    let standard = off.shift(&dst, true)?;
    let adjusted = result_datetime(ctx, dt_class, value.shift(&standard, false)?, tz)?;
    ctx.heap.keep(adjusted);
    let dst = offset(
        ctx,
        tz,
        "dst",
        adjusted,
        "fromutc: tz.dst() gave inconsistent results; cannot convert",
    )?;
    if dst.delta_microseconds() == 0 {
        return Ok(adjusted);
    }
    let value = calendar(ctx, adjusted, "datetime").ok_or_else(|| {
        Error::new(
            "TypeError",
            "datetime subtype constructor did not return a datetime",
        )
    })?;
    let class = ctx.exact_type(adjusted)?;
    result_datetime(ctx, class, value.shift(&dst, false)?, tz)
}

pub(super) fn astimezone(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some((&dt, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "unbound datetime.astimezone() needs an argument",
        ));
    };
    let [target] =
        crate::dynamic::builtins::bind_method_arguments("astimezone", rest, kwargs, ["tz"], 1)?;
    let target = target.unwrap_or(NONE);
    let target_zone = zone(ctx, target)?;
    let (value, source) = match ctx.heap.get(ctx.scalar_value(dt)) {
        Value::Calendar { value, tzinfo, .. } if value.class() == "datetime" => {
            (value.clone(), *tzinfo)
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                "astimezone() requires a datetime receiver",
            ));
        }
    };
    if source == NONE || target == NONE {
        return Err(Error::new(
            "NotImplementedError",
            "astimezone() with the system local timezone",
        ));
    }
    if source == target {
        return Ok(dt);
    }
    let source_offset = call_tzinfo(ctx, source, "utcoffset", dt)?;
    let off = calendar(ctx, source_offset, "timedelta")
        .ok_or_else(|| Error::new("NotImplementedError", "astimezone() with a naive datetime"))?;
    let shifted = value.shift(&off, true)?;
    let class = ctx.exact_type(dt)?;
    let intermediate = if class == ctx.builtin("datetime.datetime") {
        store(
            ctx,
            shifted.with_datetime_zone(target_zone.as_ref())?,
            target,
        )
    } else {
        // Subtracting the offset calls the subtype constructor with the original zone.
        let mark = ctx.heap.temporary_mark();
        let result = result_datetime(ctx, class, shifted, source)?;
        ctx.heap.release_temporaries_since(mark);
        let intermediate = attach_target(ctx, result, target, target_zone.as_ref())?;
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(intermediate);
        if intermediate != result && ctx.heap.observes_released_roots() {
            ctx.collect_unreachable();
        }
        intermediate
    };
    invoke(ctx, target, intermediate)
}

fn attach_target(ctx: &mut Context, result: Id, target: Id, zone: Option<&Calendar>) -> Result<Id> {
    ctx.heap.keep(result);
    let scalar = ctx.scalar_value(result);
    let (wall, old_zone) = match ctx.heap.get(scalar) {
        Value::Calendar { value, tzinfo, .. } if value.class() == "datetime" => {
            (value.clone(), *tzinfo)
        }
        _ => return Ok(result),
    };
    let zoned = wall.with_datetime_zone(zone)?;
    if old_zone == NONE {
        // A naive result requires fresh aware storage. Allocate its actual type
        // without repeating __new__ or copying Python instance attributes.
        let copy = store(ctx, zoned, target);
        if scalar == result {
            return Ok(copy);
        }
        let class = ctx.exact_type(result)?;
        let instance = ctx.instance(class, None);
        if let Value::Instance { scalar, .. } = ctx.heap.get_mut(instance) {
            *scalar = Some(copy);
        }
        return Ok(instance);
    }
    if ctx.heap.observes_released_roots() {
        // A releasing zone's finalizer can see the intermediate already in UTC.
        let utc = utc(ctx);
        change_zone(
            ctx,
            scalar,
            wall.with_datetime_zone(Some(&Calendar::utc()))?,
            utc,
        );
        ctx.collect_unreachable();
    }
    change_zone(ctx, scalar, zoned, target);
    Ok(result)
}

fn change_zone(ctx: &mut Context, scalar: Id, wall: Calendar, zone: Id) {
    if let Value::Calendar { value, tzinfo, .. } = ctx.heap.get_mut(scalar) {
        *value = wall;
        let old = std::mem::replace(tzinfo, zone);
        ctx.heap.weak_edge_removed(old);
    }
}
