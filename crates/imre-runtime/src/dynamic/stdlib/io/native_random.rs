//! BufferedRandom couples a seekable FileIO to a single read/write cursor.
use super::{
    Context, Error, Id, Result, Value, buffered_reader, descriptor_buffer, native_reader,
    native_writer,
};
use crate::dynamic::stdlib::{builtins, tempfile};

const OWNER: &str = "BufferedRandom";
pub(super) const METHODS: &[&str] = &[
    "__init__",
    "__getstate__",
    "__repr__",
    "raw",
    "name",
    "mode",
    "closed",
    "readable",
    "writable",
    "seekable",
    "fileno",
    "isatty",
    "_dealloc_warn",
    "detach",
    "readinto1",
    "peek",
    "read",
    "read1",
    "readinto",
    "readline",
    "write",
    "seek",
    "tell",
    "flush",
    "close",
    "truncate",
];

pub(super) fn call(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "__init__" {
        if matches!(ctx.heap.get(receiver), Value::BufferedRandom(..)) {
            return Err(Error::new(
                "NotImplementedError",
                "BufferedRandom reinitialization is unsupported",
            ));
        }
        return descriptor_buffer::initialize(ctx, receiver, args, kwargs, OWNER);
    }
    if matches!(name, "__repr__" | "__getstate__") {
        return native_reader::call(ctx, OWNER, receiver, name, args, kwargs);
    }
    if buffered_reader::has_state(ctx, receiver) {
        if name == "write" {
            super::argument_count(OWNER, name, args.len())?;
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "BufferedRandom.write() takes no keyword arguments",
                ));
            }
            super::super::files::output_bytes(ctx, args[0])?;
            return ctx.with_buffer_export(args[0], |ctx| {
                buffered_reader::call(ctx, receiver, name, args)
            });
        }
        return native_reader::call(ctx, OWNER, receiver, name, args, kwargs);
    }
    if name == "readinto1" {
        return super::buffered_io::call(ctx, receiver, name, args, kwargs);
    }
    if name == "_dealloc_warn" {
        return native_reader::call(ctx, OWNER, receiver, name, args, kwargs);
    }
    if let Value::BufferedRandom(file, path) = ctx.heap.get(receiver) {
        let file = file.clone();
        match name {
            "read1" | "peek" => {
                super::argument_count(OWNER, name, args.len())?;
                if !kwargs.is_empty() {
                    return Err(Error::new(
                        "TypeError",
                        format!("BufferedRandom.{name}() takes no keyword arguments"),
                    ));
                }
                let size = args
                    .first()
                    .map(|&v| descriptor_buffer::ssize(ctx, v))
                    .transpose()?
                    .and_then(|n| usize::try_from(n).ok());
                let data = if name == "peek" {
                    file.borrow_mut().random_peek()?
                } else {
                    file.borrow_mut().random_read_once(size)?
                };
                return Ok(ctx.alloc(Value::Bytes(data)));
            }
            "detach" => {
                if !args.is_empty() || !kwargs.is_empty() {
                    return Err(Error::new("TypeError", "detach() takes no arguments"));
                }
                return Err(Error::new("UnsupportedOperation", "detach"));
            }
            "closed" => return Ok(builtins::boolean(file.borrow().closed())),
            "name" => return Ok(*path),
            "mode" => return Ok(ctx.text("rb+")),
            "raw" => {
                return Err(Error::new(
                    "AttributeError",
                    "'_io.BufferedRandom' object has no attribute 'raw'",
                ));
            }
            _ => {}
        }
        return tempfile::method(ctx, receiver, name, args, kwargs);
    }
    match name {
        "peek" | "read" | "read1" | "readinto" | "readline" => {
            native_reader::call(ctx, OWNER, receiver, name, args, kwargs)
        }
        _ => native_writer::call(ctx, OWNER, receiver, name, args, kwargs),
    }
}
