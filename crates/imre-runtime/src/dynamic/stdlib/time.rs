//! Wall time and the civil-time tuples used by HTTP dates and request logging.
use super::*;
use crate::dynamic::builtins::bind;
use crate::objects::Calendar;
use crate::objects::time::{ClockParts, LocalTime, wall_time, wall_time_ns};

/// Install the snapshot of local standard and daylight time exported by time.
pub(in crate::dynamic) fn install_timezone(ctx: &mut Context, scope: Id) -> Result<()> {
    let [(standard, std_name), (summer, dst_name)] = crate::objects::time::timezone_samples()?;
    let std_name = ctx.text(std_name);
    let dst_name = ctx.text(dst_name);
    let names = ctx.alloc(Value::Tuple(vec![std_name, dst_name]));
    let zone = ctx.int(standard);
    let altzone = ctx.int(summer);
    let daylight = ctx.int(i64::from(standard != summer));
    ctx.bind(scope, "timezone", zone);
    ctx.bind(scope, "altzone", altzone);
    ctx.bind(scope, "daylight", daylight);
    ctx.bind(scope, "tzname", names);
    Ok(())
}

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

/// Format local civil fields with fixed English names and the caller's year width.
pub(super) fn ctime_text(fields: [i64; 9], year_width: usize) -> String {
    // CPython uses fixed English abbreviations regardless of LC_TIME.
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let [year, month, day, hour, minute, second, weekday, ..] = fields;
    let weekday = DAYS[weekday as usize];
    let month = MONTHS[(month - 1) as usize];
    // _asctime formats the calendar year as a C int.
    let year = year as libc::c_int;
    format!("{weekday} {month} {day:2} {hour:02}:{minute:02}:{second:02} {year:0year_width$}")
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
    match ctx.heap.get(ctx.scalar_value(stamp)) {
        Value::None => Ok(ClockParts::from_float(wall_time(), utc)?),
        Value::Float(n) => Ok(ClockParts::from_float(*n, utc)?),
        _ => Ok(ClockParts::from_int(&ctx.index_integer(stamp)?, utc)?),
    }
}

/// Convert the tuple in field order, including any struct_time timezone attributes.
fn tuple_time(ctx: &mut Context, value: Id, name: &str) -> Result<LocalTime> {
    let tuple = ctx.builtin("tuple");
    if !ctx.instance_of(value, tuple) {
        return Err(Error::new(
            "TypeError",
            "Tuple or struct_time argument required",
        ));
    }
    let fields: [Id; 9] = ctx
        .sequence(value)
        .and_then(|fields| fields.try_into().ok())
        .ok_or_else(|| {
            Error::new(
                "TypeError",
                format!("{name}(): illegal time tuple argument"),
            )
        })?;
    let mut parts = [0; 9];
    for (part, field) in parts.iter_mut().zip(fields) {
        *part = Calendar::component(&ctx.index_integer(field)?)?;
    }
    let mut local = LocalTime::new(parts)?;
    if matches!(
        ctx.heap.get(value),
        Value::NamedTuple {
            class: Some("time.struct_time"),
            ..
        }
    ) {
        let zone = ctx.lookup_attr(value, "tm_zone")?;
        if zone != NONE {
            let storage = ctx.string_storage(zone).ok_or_else(|| {
                Error::new("TypeError", "bad argument type for built-in operation")
            })?;
            if let Ok(name) = ctx.str_value(storage) {
                local.set_zone(name);
            } else {
                // A lone surrogate cannot be represented in the native timezone name.
                ctx.encode_utf8_codepoints(storage, false, "strict")?;
            }
        }
        let offset = ctx.lookup_attr(value, "tm_gmtoff")?;
        if offset != NONE {
            local.set_offset(&ctx.index_integer(offset)?)?;
        }
    }
    Ok(local)
}

/// Return a fresh namespace for the clock selected by the platform.
#[cfg(target_os = "linux")]
fn get_clock_info(ctx: &mut Context, args: &[Id]) -> Result<Id> {
    let [arg] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "get_clock_info() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    };
    let text = ctx.string_storage(*arg).ok_or_else(|| {
        let kind = if *arg == NONE {
            "None"
        } else {
            ctx.type_name(*arg)
        };
        Error::new(
            "TypeError",
            format!("get_clock_info() argument 1 must be str, not {kind}"),
        )
    })?;
    // PyArg's 's' format encodes the entire string strictly before checking for NUL.
    let encoded;
    let name = match ctx.heap.get(text) {
        Value::Str(s) => s.as_bytes(),
        _ => {
            encoded = ctx.encode_utf8_codepoints(text, false, "strict")?;
            encoded.as_slice()
        }
    };
    if name.contains(&0) {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    let info = crate::objects::time::clock_info(name)?;
    let class = crate::dynamic::namespace::class(ctx)?;
    let result = ctx.instance(class, None);
    let implementation = ctx.text(info.implementation);
    ctx.set_attr(result, "implementation", implementation)?;
    ctx.set_attr(result, "monotonic", builtins::boolean(info.monotonic))?;
    ctx.set_attr(result, "adjustable", builtins::boolean(!info.monotonic))?;
    let resolution = ctx.alloc(Value::Float(info.resolution));
    ctx.set_attr(result, "resolution", resolution)?;
    Ok(result)
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
    let function = match name {
        "time" => "time.time",
        "time_ns" => "time.time_ns",
        "sleep" => "time.sleep",
        "monotonic" => "time.monotonic",
        "perf_counter" => "time.perf_counter",
        "mktime" => "time.mktime",
        "tzset" => "time.tzset",
        _ => name,
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{function}() takes no keyword arguments"),
        ));
    }
    match name {
        #[cfg(target_os = "linux")]
        "get_clock_info" => get_clock_info(ctx, args),
        "sleep" => {
            let [seconds] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "time.sleep() takes exactly one argument ({} given)",
                        args.len()
                    ),
                ));
            };
            audit::event(ctx, "time.sleep", args)?;
            let nanos = if let Value::Float(value) = ctx.heap.get(ctx.scalar_value(*seconds)) {
                crate::objects::time::sleep_float_ns(*value)?
            } else {
                let overflow = || {
                    Error::new(
                        "OverflowError",
                        "timestamp too large to convert to C PyTime_t",
                    )
                };
                let seconds = match ctx.index_integer(*seconds) {
                    Ok(seconds) => seconds,
                    Err(error) if ctx.is_builtin_error(&error, "TypeError") => {
                        let class = ctx.exact_type(*seconds)?;
                        let name = super::qualified_type(ctx, class)?;
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "'{}' object cannot be interpreted as an integer or float",
                                ctx.str_value(name)?
                            ),
                        ));
                    }
                    Err(error) if ctx.is_builtin_error(&error, "OverflowError") => {
                        return Err(overflow());
                    }
                    Err(error) => return Err(error),
                };
                seconds
                    .to_i64()
                    .and_then(|n| n.checked_mul(1_000_000_000))
                    .ok_or_else(overflow)?
            };
            crate::objects::time::sleep_ns(nanos, || ctx.dispatch_signals())?;
            Ok(NONE)
        }
        "time" if args.is_empty() => Ok(ctx.alloc(Value::Float(wall_time()))),
        "time_ns" if args.is_empty() => {
            let nanos = i64::try_from(wall_time_ns()).map_err(|_| {
                Error::new(
                    "OverflowError",
                    "timestamp too large to convert to C PyTime_t",
                )
            })?;
            Ok(ctx.int(nanos))
        }
        "monotonic" | "perf_counter" if args.is_empty() => {
            let stamp =
                crate::objects::time::monotonic_time().map_err(crate::objects::errno_error)?;
            Ok(ctx.alloc(Value::Float(stamp.as_secs_f64())))
        }
        "tzset" if args.is_empty() => {
            let module = ctx.module("time")?;
            let Value::Module { scope, .. } = *ctx.heap.get(module) else {
                return Err(Error::new("TypeError", "time must be a module"));
            };
            unsafe extern "C" {
                fn tzset();
            }
            // SAFETY: POSIX tzset has no arguments and refreshes libc's local-time rules.
            unsafe { tzset() };
            install_timezone(ctx, scope)?;
            Ok(NONE)
        }
        "time" | "time_ns" | "monotonic" | "perf_counter" | "tzset" => Err(Error::new(
            "TypeError",
            format!("{function}() takes no arguments ({} given)", args.len()),
        )),
        "mktime" => match args {
            [value] => {
                let stamp = tuple_time(ctx, *value, name)?.timestamp()?;
                Ok(ctx.alloc(Value::Float(stamp)))
            }
            _ => Err(Error::new(
                "TypeError",
                format!(
                    "time.mktime() takes exactly one argument ({} given)",
                    args.len()
                ),
            )),
        },
        "strftime" => {
            let format = match args {
                [format] | [format, _] => *format,
                _ => {
                    let bound = if args.is_empty() {
                        "at least 1 argument"
                    } else {
                        "at most 2 arguments"
                    };
                    return Err(Error::new(
                        "TypeError",
                        format!("strftime() takes {bound} ({} given)", args.len()),
                    ));
                }
            };
            let format = ctx.string_storage(format).ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "strftime() argument 1 must be str, not {}",
                        if format == NONE {
                            "None"
                        } else {
                            ctx.type_name(format)
                        }
                    ),
                )
            })?;
            let clock = match args.get(1) {
                Some(&value) => tuple_time(ctx, value, name)?,
                None => LocalTime::now()?,
            };
            let points = clock.strftime(&ctx.string_codepoints(format)?)?;
            Ok(ctx.text_codepoints(points))
        }
        "gmtime" | "localtime" | "ctime" => {
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
            if name == "ctime" {
                return Ok(ctx.text(ctime_text(parts.fields, 0)));
            }
            let fields = parts.fields.map(|n| ctx.int(n));
            let zone = parts.zone.map_or(NONE, |s| ctx.text(s.as_str()));
            let offset = ctx.int(parts.offset);
            Ok(struct_time(ctx, fields.into(), zone, offset))
        }
        _ => unreachable!("unregistered time function"),
    }
}
