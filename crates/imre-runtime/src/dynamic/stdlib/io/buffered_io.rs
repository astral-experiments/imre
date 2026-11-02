//! BufferedIOBase supplies readinto and readinto1 for subclasses implementing read or read1.

use super::{Context, Error, Id, NONE, Result, Value};

pub(super) const METHODS: &[&str] = &["read", "read1", "readinto", "readinto1", "detach"];

pub(super) fn call(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    if matches!(name, "read" | "read1") {
        if args.len() > 1 {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes at most 1 argument ({} given)", args.len()),
            ));
        }
        if let Some(&id) = args.first() {
            ctx.index_integer(id)?
                .to_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C int")
                })?;
        }
        return Err(Error::new("UnsupportedOperation", name));
    }
    let [buffer] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "{}.{name}() takes exactly one argument ({} given)",
                ctx.type_name(receiver),
                args.len()
            ),
        ));
    };
    let Some((target, span)) = ctx.writable_byte_span(*buffer) else {
        let kind = if *buffer == NONE {
            "None"
        } else {
            ctx.type_name(*buffer)
        };
        return Err(Error::new(
            "TypeError",
            format!("{name}() argument must be read-write bytes-like object, not {kind}"),
        ));
    };
    let capacity = span.len();
    // Keep the destination exported while looking up and calling the reader override.
    let data = ctx.with_buffer_export(*buffer, |ctx| {
        let method = if name == "readinto1" { "read1" } else { "read" };
        let read = ctx.lookup_attr(receiver, method)?;
        let size = ctx.int(capacity as i64);
        ctx.call(read, vec![size], Vec::new())
    })?;
    let source = ctx.scalar_value(data);
    let Some((Value::Bytes(bytes), Value::ByteArray(destination))) =
        ctx.heap.source_and_target(source, target)
    else {
        return Err(Error::new("TypeError", "read() should return bytes"));
    };
    let count = bytes.len();
    if count > capacity {
        return Err(Error::new(
            "ValueError",
            format!("read() returned too much data: {capacity} bytes requested, {count} returned"),
        ));
    }
    destination[span.start..span.start + count].copy_from_slice(bytes);
    Ok(ctx.int(count as i64))
}
