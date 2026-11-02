//! Convert UTC civil fields through a timezone's Python fromutc protocol.
use super::*;

fn invoke(ctx: &mut Context, tz: Id, dt: Id) -> Result<Id> {
    ctx.heap.keep(dt);
    let callback = ctx.lookup_attr(tz, "fromutc")?;
    ctx.call(callback, vec![dt], vec![])
}

/// Date.today calls the public clock before looking up fromtimestamp on the receiving class.
pub(super) fn date_constructor(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&class, args) = args
        .split_first()
        .ok_or_else(|| Error::new("TypeError", "date class method requires a type"))?;
    let name = ctx.class_name(class).rsplit('.').next().unwrap();
    if method == "date.today" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}.today() takes no keyword arguments"),
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}.today() takes no arguments ({} given)", args.len()),
            ));
        }
        let mark = ctx.heap.temporary_mark();
        let time = import_clock_module(ctx)?;
        ctx.heap.keep(time);
        let clock = ctx.lookup_attr(time, "time")?;
        ctx.finish_call_operands(mark, clock);
        ctx.heap.keep(clock);
        let stamp = ctx.call(clock, vec![], vec![])?;
        // Releasing the last clock reference can replace the class's fromtimestamp method.
        ctx.finish_call_operands(mark, stamp);
        ctx.heap.keep(stamp);
        let construct = ctx.lookup_attr(class, "fromtimestamp")?;
        return ctx.call(construct, vec![stamp], vec![]);
    }
    let name = format!("{name}.fromtimestamp");
    let stamp = crate::dynamic::builtins::one_positional_arg(&name, args, kwargs)?;
    let parts = super::super::time::civil(ctx, Some(stamp), false)?;
    let fields = parts.fields[..3].iter().map(|&n| ctx.int(n)).collect();
    ctx.call(class, fields, vec![])
}

/// Import for side effects, then resolve the clock module from the interpreter's registry.
fn import_clock_module(ctx: &mut Context) -> Result<Id> {
    let mark = ctx.heap.temporary_mark();
    let builtins_key = ctx.heap.code_name("__builtins__");
    let globals = if ctx.heap.python_depth() == 0 {
        let builtins = ctx.module("builtins")?;
        ctx.dict(vec![(builtins_key, builtins)])
    } else {
        ctx.module_globals(ctx.heap.current_scope())?
    };
    ctx.heap.keep(globals);
    let builtins = ctx.get_item(globals, builtins_key)?;
    ctx.heap.keep(builtins);
    let hook = if ctx.dict_storage(builtins).is_some() {
        let key = ctx.heap.code_name("__import__");
        ctx.get_item(builtins, key)
            .map_err(|_| ctx.lookup_error("KeyError", key))?
    } else {
        ctx.lookup_attr(builtins, "__import__")?
    };
    ctx.heap.keep(hook);
    let name = ctx.heap.code_name("time");
    let fromlist = ctx.alloc(Value::List(Vec::new()));
    ctx.heap.keep(fromlist);
    let level = ctx.int(0);
    ctx.heap.keep(level);
    let call_mark = ctx.heap.temporary_mark();
    let returned = ctx.call(hook, vec![name, globals, globals, fromlist, level], vec![])?;
    // The hook's result can finalize and replace the registry entry before it is read.
    ctx.heap.keep(returned);
    ctx.discard_truth(call_mark);
    let time = ctx.get_item(ctx.modules, name)?;
    let collect = ctx.heap.discard_truth_operands(mark, ctx.modules);
    ctx.heap.keep(time);
    if collect {
        ctx.collect_unreachable();
    }
    Ok(time)
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
    if source == target && source != NONE {
        return Ok(dt);
    }
    let offset_mark = ctx.heap.temporary_mark();
    let source_offset = call_tzinfo(ctx, source, "utcoffset", dt)?;
    let off = if source_offset == NONE {
        value.system_offset()?
    } else {
        calendar(ctx, source_offset, "timedelta").expect("validated utcoffset")
    };
    let shifted = value.shift(&off, true)?;
    // The system timezone is chosen after the subtype constructor runs, using
    // the actual UTC intermediate it returned.
    let system_target = target == NONE;
    let (target, target_zone) = if system_target {
        (utc(ctx), Some(Calendar::utc()))
    } else {
        (target, target_zone)
    };
    let class = ctx.exact_type(dt)?;
    let exact = class == ctx.builtin("datetime.datetime");
    let mark = if system_target {
        offset_mark
    } else {
        ctx.heap.temporary_mark()
    };
    let result = if exact {
        store(
            ctx,
            shifted.with_datetime_zone(target_zone.as_ref())?,
            target,
        )
    } else {
        // Subtracting the offset calls the subtype constructor with the original zone.
        result_datetime(ctx, class, shifted, source)?
    };
    // An offset subtype can switch TZ in __del__ before the system zone is sampled.
    if system_target
        && ctx.scalar_value(source_offset) != source_offset
        && ctx.heap.observes_released_roots()
    {
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(result);
        ctx.collect_unreachable();
    }
    let intermediate = if exact {
        result
    } else {
        ctx.heap.release_temporaries_since(mark);
        let intermediate = attach_target(ctx, result, target, target_zone.as_ref())?;
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(intermediate);
        if intermediate != result && ctx.heap.observes_released_roots() {
            ctx.collect_unreachable();
        }
        intermediate
    };
    if system_target {
        let scalar = ctx.scalar_value(intermediate);
        let wall = calendar(ctx, intermediate, "datetime").ok_or_else(|| {
            Error::new(
                "TypeError",
                "datetime subtype constructor did not return a datetime",
            )
        })?;
        let (zone, name) = wall.system_zone_at_utc()?;
        let zoned = wall.with_datetime_zone(Some(&zone))?;
        let target = if zone.is_utc() {
            utc(ctx)
        } else {
            let tzname = name.map_or(NONE, |name| ctx.text(name));
            let tzoffset = store(ctx, zone.zone_offset().expect("fixed system zone"), NONE);
            ctx.alloc(Value::Calendar {
                value: zone,
                tzinfo: NONE,
                tzoffset,
                tzname,
            })
        };
        change_zone(ctx, scalar, zoned, target);
        return invoke(ctx, target, intermediate);
    }
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
