//! Formatting and civil-time operations that observe the attached Python tzinfo.
use super::*;

pub(in crate::dynamic) fn custom_zone(ctx: &Context, receiver: Id) -> Option<Id> {
    let Value::Calendar { tzinfo, .. } = *ctx.heap.get(ctx.scalar_value(receiver)) else {
        return None;
    };
    (tzinfo != NONE && !matches!(ctx.heap.get(tzinfo), Value::Calendar { .. })).then_some(tzinfo)
}

pub(super) fn offset_text(ctx: &mut Context, zone: Id, arg: Id, colon: bool) -> Result<String> {
    if let Value::Calendar { value, .. } = ctx.heap.get(zone) {
        return Ok(value.strftime_offset(colon));
    }
    Ok(comparison::offset(ctx, zone, arg)?
        .map_or_else(String::new, |us| Calendar::format_utc_offset(us, colon)))
}

pub(super) fn isdst(ctx: &mut Context, receiver: Id) -> Result<i64> {
    let Some(zone) = custom_zone(ctx, receiver) else {
        return Ok(-1);
    };
    let delta = call_tzinfo(ctx, zone, "dst", receiver)?;
    Ok(calendar(ctx, delta, "timedelta").map_or(-1, |d| i64::from(d.delta_microseconds() != 0)))
}

pub(super) fn timestamp(ctx: &mut Context, receiver: Id, value: &Calendar) -> Result<f64> {
    let Some(zone) = custom_zone(ctx, receiver) else {
        return Ok(value.timestamp()?);
    };
    let offset = comparison::offset(ctx, zone, receiver)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            "can't subtract offset-naive and offset-aware datetimes",
        )
    })?;
    Ok(value.timestamp_with_offset(offset))
}

pub(super) fn repr(
    ctx: &mut Context,
    receiver: Id,
    value: &Calendar,
    class: Id,
    kind: &str,
) -> Result<Id> {
    let Value::Calendar {
        tzinfo,
        tzoffset,
        tzname,
        ..
    } = *ctx.heap.get(ctx.scalar_value(receiver))
    else {
        unreachable!()
    };
    if kind == "timezone" {
        if receiver == utc(ctx) {
            return Ok(ctx.text("datetime.timezone.utc"));
        }
        let mut points: Vec<u32> = "datetime.timezone(".bytes().map(u32::from).collect();
        let offset = crate::dynamic::builtins::call(ctx, "repr", &[tzoffset], &[])?;
        points.extend(ctx.string_codepoints(offset)?.iter().copied());
        if tzname != NONE {
            points.extend(", ".bytes().map(u32::from));
            let name = crate::dynamic::builtins::call(ctx, "repr", &[tzname], &[])?;
            points.extend(ctx.string_codepoints(name)?.iter().copied());
        }
        points.push(u32::from(b')'));
        return Ok(ctx.text_codepoints(points));
    }
    let base = value.subtype_repr(ctx.class_name(class), kind == "date");
    if kind != "date" && tzinfo != NONE {
        let at = if kind == "time" && value.clock_parts()[4] != 0 {
            base.len() - ", fold=1)".len()
        } else {
            base.len() - 1
        };
        let builtin = ctx.builtin("repr");
        let supplied = ctx.call(builtin, vec![tzinfo], vec![])?;
        let mut points: Vec<u32> = base[..at].chars().map(u32::from).collect();
        points.extend(", tzinfo=".bytes().map(u32::from));
        points.extend(
            ctx.string_codepoints(ctx.scalar_value(supplied))?
                .iter()
                .copied(),
        );
        points.extend(base[at..].chars().map(u32::from));
        return Ok(ctx.text_codepoints(points));
    }
    Ok(ctx.text(base))
}

pub(super) fn date_isoformat(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "unbound method date.isoformat() needs an argument",
        ));
    };
    let value = calendar(ctx, receiver, "date").ok_or_else(|| {
        let name = type_name(ctx, receiver);
        Error::new(
            "TypeError",
            format!(
                "descriptor 'isoformat' for 'datetime.date' objects doesn't apply to a '{}' object",
                &name[..name.floor_char_boundary(100)]
            ),
        )
    })?;
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "date.isoformat() takes no keyword arguments",
        ));
    }
    if !rest.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("date.isoformat() takes no arguments ({} given)", rest.len()),
        ));
    }
    let [year, month, day, ..] = value.fields();
    Ok(ctx.text(format!("{year:04}-{month:02}-{day:02}")))
}

pub(super) fn isoformat(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (kind, _) = method.split_once('.').unwrap();
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method datetime.{kind}.isoformat() needs an argument"),
        ));
    };
    let value = calendar(ctx, receiver, kind)
        .ok_or_else(|| field_type_error(ctx, &format!("datetime.{kind}"), "isoformat", receiver))?;
    let (sep, spec) = if kind == "time" {
        let [spec] = crate::dynamic::builtins::bind_method_arguments(
            "isoformat",
            rest,
            kwargs,
            ["timespec"],
            1,
        )?;
        (None, spec)
    } else {
        let [sep, spec] = crate::dynamic::builtins::bind_method_arguments(
            "isoformat",
            rest,
            kwargs,
            ["sep", "timespec"],
            2,
        )?;
        (sep, spec)
    };
    let separator = if let Some(sep) = sep {
        let scalar = ctx.string_storage(sep).ok_or_else(|| {
            Error::new(
                "TypeError",
                format!(
                    "isoformat() argument 1 must be a unicode character, not {}",
                    if sep == NONE {
                        "None"
                    } else {
                        ctx.type_name(sep)
                    }
                ),
            )
        })?;
        let points = ctx.string_codepoints(scalar)?;
        let [ch] = points.as_ref() else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "isoformat() argument 1 must be a unicode character, not a string of length {}",
                    points.len()
                ),
            ));
        };
        *ch
    } else {
        u32::from('T')
    };
    let spec = if let Some(id) = spec {
        let scalar = ctx.string_storage(id).ok_or_else(|| {
            Error::new(
                "TypeError",
                format!(
                    "isoformat() argument {} must be str, not {}",
                    if kind == "time" { 1 } else { 2 },
                    if id == NONE {
                        "None"
                    } else {
                        ctx.type_name(id)
                    }
                ),
            )
        })?;
        if matches!(ctx.heap.get(scalar), Value::SurrogateStr(_)) {
            // The C argument parser converts a timespec to UTF-8 before comparing its spelling.
            super::super::codecs::encode_text(ctx, scalar, None, None)?;
        }
        let text = ctx.str_value(scalar)?;
        if text.as_bytes().contains(&0) {
            return Err(Error::new("ValueError", "embedded null character"));
        }
        text
    } else {
        "auto"
    };
    let (y, m, d, h, min, s, us) = if kind == "time" {
        let [h, min, s, us, _] = value.clock_parts();
        (0, 0, 0, h, min, s, us)
    } else {
        let [y, m, d, h, min, s, us, _] = value.fields().map(|n| n as i64);
        (y, m, d, h, min, s, us)
    };
    use std::fmt::Write;

    let mut text = String::with_capacity(32);
    let scalar_separator = char::from_u32(separator);
    if kind != "time" {
        write!(text, "{y:04}-{m:02}-{d:02}").unwrap();
        text.push(scalar_separator.unwrap_or('T'));
    }
    match spec {
        "hours" => write!(text, "{h:02}"),
        "minutes" => write!(text, "{h:02}:{min:02}"),
        "seconds" => write!(text, "{h:02}:{min:02}:{s:02}"),
        "milliseconds" => write!(text, "{h:02}:{min:02}:{s:02}.{:03}", us / 1000),
        "auto" if us == 0 => write!(text, "{h:02}:{min:02}:{s:02}"),
        "microseconds" | "auto" => write!(text, "{h:02}:{min:02}:{s:02}.{us:06}"),
        _ => return Err(Error::new("ValueError", "Unknown timespec value")),
    }
    .unwrap();
    let Value::Calendar { tzinfo, .. } = *ctx.heap.get(ctx.scalar_value(receiver)) else {
        unreachable!()
    };
    let arg = if kind == "time" { NONE } else { receiver };
    text.push_str(&offset_text(ctx, tzinfo, arg, true)?);
    if scalar_separator.is_none() {
        // A lone surrogate occupies the single separator position after YYYY-MM-DD.
        let mut points: Vec<_> = text.bytes().map(u32::from).collect();
        points[10] = separator;
        return Ok(ctx.text_codepoints(points));
    }
    Ok(ctx.text(text))
}

// Exact calendar values have no Python instance slots for the generic container renderer to call.
pub(in crate::dynamic) fn exact(ctx: &mut Context, id: Id, quoted: bool) -> Result<Option<Id>> {
    let Value::Calendar { value, .. } = ctx.heap.get(id) else {
        return Ok(None);
    };
    let value = value.clone();
    let kind = value.class();
    if quoted && python_repr(ctx, id) {
        let class = ctx.exact_type(id)?;
        return repr(ctx, id, &value, class, kind).map(Some);
    }
    if quoted || custom_zone(ctx, id).is_none() {
        return Ok(None);
    }
    if kind == "time" {
        isoformat(ctx, "time.isoformat", &[id], &[]).map(Some)
    } else {
        let sep = ctx.text(" ");
        isoformat(ctx, "datetime.isoformat", &[id, sep], &[]).map(Some)
    }
}

/// These exact values refer to Python objects with an observable repr.
pub(in crate::dynamic) fn python_repr(ctx: &Context, id: Id) -> bool {
    matches!(ctx.heap.get(id), Value::Calendar { tzinfo, tzoffset, .. } if *tzinfo != NONE || *tzoffset != NONE)
}
