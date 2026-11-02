//! BufferedRandom couples a seekable FileIO to a single read/write cursor.
use super::{Context, Error, Id, Result, Value, descriptor_buffer, native_reader, native_writer};
use crate::dynamic::stdlib::{builtins, tempfile};

const OWNER: &str = "BufferedRandom";
pub(super) const METHODS: &[&str] = &[
    "__init__", "raw", "name", "mode", "closed", "readable", "writable", "seekable", "fileno",
    "isatty", "peek", "read", "read1", "readinto", "readline", "write", "seek", "tell", "flush",
    "close", "truncate",
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
        let mut operands = vec![receiver];
        operands.extend_from_slice(args);
        return tempfile::method(ctx, name, &operands, kwargs);
    }
    match name {
        "peek" | "read" | "read1" | "readinto" | "readline" => {
            native_reader::call(ctx, OWNER, receiver, name, args, kwargs)
        }
        _ => native_writer::call(ctx, OWNER, receiver, name, args, kwargs),
    }
}
