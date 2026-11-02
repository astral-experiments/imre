//! Calendar-specific directives layered on the platform time.strftime.
use super::*;

// Some libcs already format an early year with its full four digits.
static NORMALIZE_CENTURY: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    crate::objects::time::LocalTime::new([99, 1, 1, 0, 0, 0, 0, 1, -1])
        .and_then(|clock| clock.strftime(&(*b"%Y").map(u32::from)))
        .is_ok_and(|year| !year.is_empty() && year != (*b"0099").map(u32::from))
});

/// Format the receiver using its native calendar fields and its timetuple protocol.
pub(super) fn format(
    ctx: &mut Context,
    kind: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method datetime.{kind}.strftime() needs an argument"),
        ));
    };
    let value = calendar(ctx, receiver, kind).ok_or_else(|| {
        Error::new("TypeError",format!("descriptor 'strftime' for 'datetime.{kind}' objects doesn't apply to a '{}' object",ctx.type_name(receiver)))
    })?;
    let [Some(format)] =
        crate::dynamic::builtins::bind_builtin_arguments("strftime", args, kwargs, ["format"], 1)?
    else {
        unreachable!("format is required")
    };
    let storage = ctx.string_storage(format).ok_or_else(|| {
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
    let (tuple, micros) = if kind == "time" {
        let [h, m, s, us, _] = value.clock_parts();
        let parts = [1900, 1, 1, h, m, s, 0, 1, -1].map(|n| ctx.int(n));
        (ctx.alloc(Value::Tuple(parts.into())), us)
    } else {
        let method = ctx.lookup_attr(receiver, "timetuple")?;
        (ctx.call(method, vec![], vec![])?, value.fields()[6] as i64)
    };
    let Value::Calendar { tzinfo, .. } = *ctx.heap.get(ctx.scalar_value(receiver)) else {
        unreachable!()
    };
    ctx.heap.keep(tuple);
    let time = ctx.module("time")?;
    let strftime = ctx.lookup_attr(time, "strftime")?;
    ctx.heap.keep(strftime);

    let points = ctx.string_codepoints(storage)?.into_owned();
    let mut pattern = Vec::new();
    let mut zone_name = None;
    let mut offsets = [None, None];
    let arg = if kind == "time" { NONE } else { receiver };
    let mut copied = 0;
    let mut i = 0;
    while i + 1 < points.len() {
        let start = i;
        i += 1;
        if points[start] != u32::from(b'%') {
            continue;
        }
        let code = char::from_u32(points[i]);
        i += 1;
        let replacement = match code {
            Some('z' | ':') if code == Some('z') || points.get(i) == Some(&u32::from(b'z')) => {
                let colon = code == Some(':');
                i += usize::from(colon);
                let slot = &mut offsets[usize::from(colon)];
                if slot.is_none() {
                    *slot = Some(render::offset_text(ctx, tzinfo, arg, colon)?);
                }
                slot.as_ref().unwrap().clone()
            }
            Some('Z') => {
                if zone_name.is_none() {
                    zone_name = Some(escaped_name(ctx, tzinfo, arg)?);
                }
                pattern.extend_from_slice(&points[copied..start]);
                pattern.extend_from_slice(zone_name.as_ref().unwrap());
                copied = i;
                continue;
            }
            Some('f') => format!("{micros:06}"),
            Some(ch @ ('Y' | 'G' | 'F' | 'C')) if *NORMALIZE_CENTURY => {
                let zero = ctx.int(0);
                let item = ctx.get_item(tuple, zero)?;
                let mut year = ctx.index_c_long(item)?;
                if year >= 1000 {
                    continue;
                }
                if ch == 'G' {
                    let iso = ctx.text("%G");
                    let text = ctx.call(strftime, vec![iso, tuple], vec![])?;
                    let int = ctx.builtin("int");
                    let number = ctx.call(int, vec![text], vec![])?;
                    year = ctx.index_c_long(number)?;
                }
                let mut year = format!("{year:04}");
                if ch == 'C' {
                    year.truncate(year.len() - 2);
                } else if ch == 'F' {
                    year.push_str("-%m-%d");
                }
                year
            }
            _ => continue,
        };
        pattern.extend_from_slice(&points[copied..start]);
        pattern.extend(replacement.bytes().map(u32::from));
        copied = i;
    }
    let pattern = if copied == 0 {
        format
    } else {
        pattern.extend_from_slice(&points[copied..]);
        ctx.text_codepoints(pattern)
    };
    ctx.call(strftime, vec![pattern, tuple], vec![])
}

fn escaped_name(ctx: &mut Context, zone: Id, arg: Id) -> Result<Vec<u32>> {
    let name = if let Value::Calendar { value, tzname, .. } = ctx.heap.get(zone) {
        if *tzname == NONE {
            ctx.text(value.text())
        } else {
            *tzname
        }
    } else {
        call_tzinfo(ctx, zone, "tzname", arg)?
    };
    if name == NONE {
        return Ok(Vec::new());
    }
    let replace = ctx.lookup_attr(name, "replace")?;
    let percent = ctx.text("%");
    let escaped = ctx.text("%%");
    let name = ctx.call(replace, vec![percent, escaped], Vec::new())?;
    if ctx.string_storage(name).is_none() {
        return Err(Error::new(
            "TypeError",
            "tzname.replace() did not return a string",
        ));
    }
    Ok(ctx.string_codepoints(name)?.into_owned())
}
