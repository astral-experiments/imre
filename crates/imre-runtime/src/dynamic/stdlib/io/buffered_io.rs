//! BufferedIOBase supplies readinto and readinto1 for subclasses implementing read or read1.

use super::{Context, Error, Id, NONE, Result, Value};

pub(super) const METHODS: &[&str] = &["read", "read1", "readinto", "readinto1", "write", "detach"];

/// Render the live name as CPython does in buffered_repr.
pub(super) fn repr(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let name = match ctx.lookup_attr(receiver, "name") {
        Ok(name) => Some(name),
        Err(error)
            if ctx.is_builtin_error(&error, "AttributeError")
                || ctx.is_builtin_error(&error, "ValueError") =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    let class = ctx.exact_type(receiver)?;
    let class_name = match ctx.heap.get(class).io_kind() {
        Some("BufferedReader") => "_io.BufferedReader",
        Some("BufferedWriter") => "_io.BufferedWriter",
        _ => ctx.class_name(class),
    }
    .to_owned();
    let Some(name) = name else {
        return Ok(ctx.text(format!("<{class_name}>")));
    };
    if ctx.repr_ancestors.contains(&receiver) {
        return Err(Error::new(
            "RuntimeError",
            format!("reentrant call inside {class_name}.__repr__"),
        ));
    }
    ctx.repr_ancestors
        .try_reserve(1)
        .map_err(|_| crate::Error::allocation())?;
    ctx.repr_ancestors.push(receiver);
    let repr = ctx.builtin("repr");
    let shown = ctx.call(repr, vec![name], Vec::new());
    ctx.repr_ancestors.pop();
    let shown = shown?;
    let prefix = ctx.text(format!("<{class_name} name="));
    let text = ctx.concat_strings(prefix, shown)?;
    let closing = ctx.text(">");
    ctx.concat_strings(text, closing)
}

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
    let Some((target, range)) = ctx.writable_byte_span(*buffer)? else {
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
    let capacity = range.len();
    // Keep the destination exported while looking up and calling the reader override.
    let data = ctx.with_buffer_export(*buffer, |ctx| {
        ctx.with_buffer_export(target, |ctx| {
            let method = if name == "readinto1" { "read1" } else { "read" };
            let read = ctx.lookup_attr(receiver, method)?;
            let size = ctx.int(capacity as i64);
            ctx.call(read, vec![size], Vec::new())
        })
    })?;
    let source = ctx.scalar_value(data);
    let Some((Value::Bytes(bytes), destination)) = ctx.heap.source_and_target(source, target)
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
    let destination = Context::mutable_storage_bytes(destination)?;
    destination[range.start..range.start + count].copy_from_slice(bytes);
    Ok(ctx.int(count as i64))
}

/// A missing or failing raw warning hook must not interrupt flush or close.
pub(super) fn dealloc_warn(ctx: &mut Context, receiver: Id, source: Id) {
    let raw = if let Some(buffer) = super::descriptor_buffer::backing(ctx, receiver)
        && let Value::File(handle) = ctx.heap.get(buffer)
        && let super::super::files::BinaryFile::Descriptor(fd) = handle.as_ref()
    {
        Some(fd.raw)
    } else if let Some(writer) = super::bytes_writer::state(ctx, receiver) {
        writer.raw.get().filter(|_| writer.ready.get())
    } else {
        super::buffered_reader::initialized_raw(ctx, receiver)
    };
    if let Some(raw) = raw {
        let _ = super::text_wrapper::invoke(ctx, raw, "_dealloc_warn", vec![source]);
    }
}
