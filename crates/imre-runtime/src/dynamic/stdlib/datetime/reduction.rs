//! CPython's compact constructor arguments for immutable calendar values.
use super::*;

/// Reduce the descriptor's native fields, retaining the actual receiver class.
pub(super) fn reduce(
    ctx: &mut Context,
    kind: &str,
    operation: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method datetime.{kind}.{operation}() needs an argument"),
        ));
    };
    let value = calendar(ctx, receiver, kind).ok_or_else(|| {
        Error::new("TypeError", format!("descriptor '{operation}' for 'datetime.{kind}' objects doesn't apply to a '{}' object", ctx.type_name(receiver)))
    })?;
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!(
                "{}.{operation}() takes no keyword arguments",
                ctx.type_name(receiver)
            ),
        ));
    }
    let protocol = if operation == "__reduce_ex__" {
        let [protocol] = rest else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "__reduce_ex__() takes exactly 1 argument ({} given)",
                    rest.len()
                ),
            ));
        };
        Calendar::component(&ctx.index_integer(*protocol)?)?
    } else {
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{}.__reduce__() takes no arguments ({} given)",
                    ctx.type_name(receiver),
                    rest.len()
                ),
            ));
        }
        2
    };
    let state = if kind == "timedelta" {
        let (days, seconds, micros) = value.delta_parts();
        vec![
            ctx.int(days as i64),
            ctx.int(seconds as i64),
            ctx.int(micros as i64),
        ]
    } else {
        let mut bytes = Vec::new();
        if kind != "time" {
            let [year, month, day, ..] = value.fields();
            bytes.extend((year as u16).to_be_bytes());
            bytes.extend([month as u8, day as u8]);
        }
        if kind != "date" {
            let [hour, minute, second, micros, fold] = if kind == "time" {
                value.clock_parts()
            } else {
                let [_, _, _, h, m, s, us, f] = value.fields();
                [h, m, s, us, f].map(|n| n as i64)
            };
            bytes.extend([hour as u8, minute as u8, second as u8]);
            bytes.extend_from_slice(&(micros as u32).to_be_bytes()[1..]);
            if protocol > 3 && fold != 0 {
                bytes[if kind == "time" { 0 } else { 2 }] |= 0x80;
            }
        }
        let mut fields = vec![ctx.alloc(Value::Bytes(bytes))];
        if kind != "date"
            && let Value::Calendar { tzinfo, .. } = ctx.heap.get(ctx.scalar_value(receiver))
            && *tzinfo != NONE
        {
            fields.push(*tzinfo);
        }
        fields
    };
    let arguments = ctx.alloc(Value::Tuple(state));
    let class = ctx.exact_type(receiver)?;
    Ok(ctx.alloc(Value::Tuple(vec![class, arguments])))
}

/// Recognize the packed positional form before parsing ordinary date/time arguments.
pub(super) fn from_state(ctx: &mut Context, kind: &str, args: &[Id]) -> Result<Option<Id>> {
    let len = match (kind, args.len()) {
        ("date", 1) => 4,
        ("time", 1 | 2) => 6,
        ("datetime", 1 | 2) => 10,
        _ => return Ok(None),
    };
    let data = ctx.scalar_value(args[0]);
    let accepted = |field: u32| match kind {
        "date" => (1..=12).contains(&field),
        "datetime" => (1..=12).contains(&(field & 0x7f)),
        _ => field & 0x7f < 24,
    };
    let discriminant = if kind == "time" { 0 } else { 2 };
    let bytes = match ctx.heap.get(data) {
        Value::Bytes(bytes) if bytes.len() == len && accepted(u32::from(bytes[discriminant])) => {
            std::borrow::Cow::Borrowed(bytes.as_slice())
        }
        Value::Str(text) if text.len() > len * 4 => return Ok(None),
        Value::Str(_) | Value::SurrogateStr(_) => {
            let points = ctx.string_codepoints(data)?;
            if points.len() != len || !accepted(points[discriminant]) {
                return Ok(None);
            }
            let bytes = points.iter().copied().map(u8::try_from)
                .collect::<std::result::Result<Vec<_>,_>>()
                .map_err(|_| Error::new("ValueError", format!(
                    "Failed to encode latin1 string when unpickling a {kind} object. pickle.load(data, encoding='latin1') is assumed."
                )))?;
            std::borrow::Cow::Owned(bytes)
        }
        _ => return Ok(None),
    };
    let tzinfo = args.get(1).copied().unwrap_or(NONE);
    let zone = zone(ctx, tzinfo).map_err(|_| Error::new("TypeError", "bad tzinfo state arg"))?;
    let mut fields = [0; 8];
    if kind != "time" {
        fields[0] = i32::from(u16::from_be_bytes([bytes[0], bytes[1]]));
        fields[1] = i32::from(bytes[2] & 0x7f);
        fields[2] = i32::from(bytes[3]);
    }
    if kind != "date" {
        let clock = &bytes[len - 6..];
        fields[3] = i32::from(if kind == "time" {
            clock[0] & 0x7f
        } else {
            clock[0]
        });
        fields[4] = i32::from(clock[1]);
        fields[5] = i32::from(clock[2]);
        fields[6] = i32::from_be_bytes([0, clock[3], clock[4], clock[5]]);
        fields[7] = i32::from(bytes[discriminant] >> 7);
    }
    // The native scalar is an ordinal or a count of microseconds. Validate the
    // component ranges so malformed fields cannot silently normalize to another date/time.
    let value = match kind {
        "date" => Calendar::date(fields[0], fields[1], fields[2])?,
        "time" => {
            let clock = [fields[3], fields[4], fields[5], fields[6], fields[7]];
            Calendar::time(Calendar::time_micros(clock)?, fields[7], zone.as_ref())?
        }
        _ => Calendar::datetime(fields, zone.as_ref())?,
    };
    Ok(Some(store(ctx, value, tzinfo)))
}
