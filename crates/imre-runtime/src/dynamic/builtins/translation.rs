//! Native byte translation tables used by Python's standard Base64 wrappers.
use super::{Context, Error, Id, NONE, Result, Value, type_error};

/// Build the byte-to-byte mapping in argument order, including repeated source bytes.
pub(super) fn maketrans(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(format!("{name}() takes no keyword arguments")));
    }
    let [from, to] = args else {
        return Err(type_error(format!(
            "maketrans expected 2 arguments, got {}",
            args.len()
        )));
    };
    let (from, to) = (ctx.bytes_like(*from)?, ctx.bytes_like(*to)?);
    if from.len() != to.len() {
        return Err(Error::new(
            "ValueError",
            "maketrans arguments must have same length",
        ));
    }
    let mut table: Vec<_> = (0..=u8::MAX).collect();
    for (&a, &b) in from.iter().zip(to) {
        table[usize::from(a)] = b;
    }
    Ok(ctx.alloc(Value::Bytes(table)))
}

/// Translate each retained input byte, preserving an unchanged immutable receiver.
pub(super) fn translate(
    ctx: &mut Context,
    owner: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&receiver, args) = args.split_first().ok_or_else(|| {
        type_error(format!(
            "unbound method {owner}.translate() needs an argument"
        ))
    })?;
    if ctx.type_name(receiver) != owner {
        return Err(type_error(format!(
            "descriptor 'translate' for '{owner}' objects doesn't apply to a '{}' object",
            ctx.type_name(receiver)
        )));
    }
    let count = args.len() + kwargs.len();
    if count > 2 {
        return Err(type_error(format!(
            "translate() takes at most 2 arguments ({count} given)"
        )));
    }
    let Some(&table) = args.first() else {
        return Err(type_error(
            "translate() takes at least 1 positional argument (0 given)",
        ));
    };
    let delete = match kwargs.first() {
        Some((name, value)) if name == "delete" => Some(*value),
        Some((name, _)) => {
            return Err(type_error(format!(
                "translate() got an unexpected keyword argument '{name}'"
            )));
        }
        None => args.get(1).copied(),
    };
    let table = if table == NONE {
        None
    } else {
        let table: &[u8; 256] = ctx.bytes_like(table)?.try_into().map_err(|_| {
            Error::new(
                "ValueError",
                "translation table must be 256 characters long",
            )
        })?;
        Some(table)
    };
    let source = ctx.bytes_like(receiver)?;
    let mut deleted = [false; 256];
    if let Some(delete) = delete {
        for &byte in ctx.bytes_like(delete)? {
            deleted[usize::from(byte)] = true;
        }
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(source.len())
        .map_err(|_| Error::new("MemoryError", "translated bytes are too large"))?;
    for &b in source {
        if !deleted[usize::from(b)] {
            result.push(table.map_or(b, |t| t[usize::from(b)]));
        }
    }
    Ok(match ctx.heap.get(receiver) {
        Value::Bytes(_) if result == source => receiver,
        Value::Bytes(_) => ctx.alloc(Value::Bytes(result)),
        _ => ctx.alloc(Value::ByteArray(result)),
    })
}
