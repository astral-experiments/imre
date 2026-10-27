use super::{Context, Error, Id, NONE, Result, Value};

/// Encode an integer with the shortest binary opcode available in pickle protocol 2.
fn write_int(out: &mut Vec<u8>, number: &crate::Int) -> Result<()> {
    match number.to_i64().and_then(|n| i32::try_from(n).ok()) {
        Some(n @ 0..=255) => out.extend([b'K', n as u8]),
        Some(n @ 256..=65535) => {
            out.push(b'M');
            out.extend((n as u16).to_le_bytes());
        }
        Some(n) => {
            out.push(b'J');
            out.extend(n.to_le_bytes());
        }
        None => {
            let signed = number.big().to_signed_bytes_le();
            if let Ok(length) = u8::try_from(signed.len()) {
                out.extend([0x8a, length]); // LONG1
            } else {
                let length = i32::try_from(signed.len())
                    .map_err(|_| Error::new("OverflowError", "int too large to pickle"))?;
                out.push(0x8b); // LONG4
                out.extend(length.to_le_bytes());
            }
            out.extend(signed);
        }
    }
    Ok(())
}

pub(super) fn dumps(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if args.len() > 2 {
        return Err(Error::new(
            "TypeError",
            format!(
                "dumps() takes at most 2 positional arguments ({} given)",
                args.len()
            ),
        ));
    }
    let mut obj = args.first().copied();
    let mut protocol = args.get(1).copied();
    let mut fix_imports = None;
    let mut buffer_callback = NONE;
    for (key, value) in kwargs {
        let (slot, position) = match key.as_str() {
            "obj" => (&mut obj, 1),
            "protocol" => (&mut protocol, 2),
            "fix_imports" => {
                fix_imports = Some(*value);
                continue;
            }
            "buffer_callback" => {
                buffer_callback = *value;
                continue;
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("dumps() got an unexpected keyword argument '{key}'"),
                ));
            }
        };
        if slot.replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("argument for dumps() given by name ('{key}') and position ({position})"),
            ));
        }
    }
    let obj = obj.ok_or_else(|| {
        Error::new(
            "TypeError",
            "dumps() missing required argument 'obj' (pos 1)",
        )
    })?;
    if let Some(value) = fix_imports {
        ctx.truth(value)?;
    }
    let protocol = match protocol {
        None | Some(NONE) => 5,
        Some(id) => ctx.index_integer(id)?.to_i64().ok_or_else(|| {
            Error::new("OverflowError", "Python int too large to convert to C long")
        })?,
    };
    if protocol > 5 {
        return Err(Error::new("ValueError", "pickle protocol must be <= 5"));
    }
    if (0..5).contains(&protocol) && buffer_callback != NONE {
        return Err(Error::new(
            "ValueError",
            "buffer_callback needs protocol >= 5",
        ));
    }
    if protocol != 2 {
        return Err(Error::new(
            "NotImplementedError",
            "native pickle.dumps supports protocol 2",
        ));
    }
    let mut out = vec![0x80, 2];
    match ctx.heap.get(obj) {
        Value::Int(n) => write_int(&mut out, n)?,
        Value::Bool(b) => out.push(if *b { 0x88 } else { 0x89 }),
        _ => {
            return Err(Error::new(
                "NotImplementedError",
                "native protocol-2 pickling supports integers",
            ));
        }
    }
    out.push(b'.');
    Ok(ctx.alloc(Value::Bytes(out)))
}
