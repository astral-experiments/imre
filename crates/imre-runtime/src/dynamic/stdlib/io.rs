//! Native base classes for binary input streams defined by runtime-loaded modules.

use super::super::builtins::zeroed_bytes;
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

const DEFAULT_BUFFER_SIZE: i64 = 128 * 1024;

/// Install canonical class objects so subclasses inherit the same methods across imports.
pub(in super::super) fn install(ctx: &mut Context, module: Id) {
    let size = ctx.int(DEFAULT_BUFFER_SIZE);
    ctx.bind(module, "DEFAULT_BUFFER_SIZE", size);
    let object = ctx.builtin("object");
    let io = class(
        ctx,
        module,
        "IOBase",
        object,
        &[
            "closed",
            "close",
            "flush",
            "readable",
            "writable",
            "seekable",
            "__enter__",
            "__exit__",
        ],
    );
    class(
        ctx,
        module,
        "RawIOBase",
        io,
        &["read", "readall", "readinto"],
    );
}

fn class(ctx: &mut Context, module: Id, name: &str, base: Id, methods: &[&'static str]) -> Id {
    let scope = ctx.scope("io", Some(module));
    let owner = ctx
        .allocate_class(format!("io.{name}"), scope, vec![base])
        .expect("a single I/O base has a linearization");
    for &name in methods {
        let method = ctx.alloc(Value::IoMethod { owner, name });
        let attr = if name == "closed" {
            ctx.alloc(Value::ReadOnlyDescriptor {
                get: method,
                owner: "_io._IOBase",
                name,
            })
        } else {
            method
        };
        ctx.bind(scope, name, attr);
    }
    ctx.bind(module, name, owner);
    owner
}

/// Call an inherited I/O method with its bound receiver or an explicit class-level receiver.
pub(in super::super) fn call(
    ctx: &mut Context,
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let arity = match name {
        "read" => 1..=2,
        "readinto" => 2..=2,
        "__exit__" => 4..=4,
        _ => 1..=1,
    };
    if !arity.contains(&args.len()) || !kwargs.is_empty() || !ctx.instance_of(args[0], owner) {
        return Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        ));
    }
    let receiver = args[0];
    let Value::Instance { fields, .. } = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected an IOBase instance"));
    };
    let closed = fields.contains_key("__IOBase_closed");
    match name {
        "closed" => Ok(if closed { TRUE } else { FALSE }),
        "readable" | "writable" | "seekable" => Ok(FALSE),
        "close" if closed => Ok(NONE),
        "close" => {
            let result = ctx
                .lookup_attr(receiver, "flush")
                .and_then(|f| ctx.call(f, Vec::new(), Vec::new()));
            ctx.set_attr(receiver, "__IOBase_closed", TRUE)?;
            result.map(|_| NONE)
        }
        "flush" | "__enter__" if closed => {
            Err(Error::new("ValueError", "I/O operation on closed file."))
        }
        "flush" => Ok(NONE),
        "__enter__" => Ok(receiver),
        "__exit__" => {
            let close = ctx.lookup_attr(receiver, "close")?;
            ctx.call(close, Vec::new(), Vec::new())
        }
        "read" => {
            let size = args
                .get(1)
                .map(|&id| ctx.as_integer(id))
                .transpose()?
                .unwrap_or(-1);
            if size < 0 {
                let readall = ctx.lookup_attr(receiver, "readall")?;
                return ctx.call(readall, Vec::new(), Vec::new());
            }
            let buffer = ctx.alloc(Value::ByteArray(zeroed_bytes(size)?));
            let readinto = ctx.lookup_attr(receiver, "readinto")?;
            let count = ctx.call(readinto, vec![buffer], Vec::new())?;
            if count == NONE {
                return Ok(NONE);
            }
            let count = ctx.as_integer(count)?;
            let Value::ByteArray(data) = ctx.heap.get(buffer) else {
                unreachable!()
            };
            if count < 0 || count > size || count as usize > data.len() {
                return Err(Error::new(
                    "ValueError",
                    format!("readinto returned {count} outside buffer size {size}"),
                ));
            }
            Ok(ctx.alloc(Value::Bytes(data[..count as usize].to_vec())))
        }
        "readall" => {
            let mut out = Vec::new();
            let size = ctx.int(DEFAULT_BUFFER_SIZE);
            loop {
                let read = ctx.lookup_attr(receiver, "read")?;
                let data = ctx.call(read, vec![size], Vec::new())?;
                if data == NONE {
                    if out.is_empty() {
                        return Ok(NONE);
                    }
                    break;
                }
                let Value::Bytes(data) = ctx.heap.get(data) else {
                    return Err(Error::new("TypeError", "read() should return bytes"));
                };
                if data.is_empty() {
                    break;
                }
                out.try_reserve(data.len())
                    .map_err(|_| Error::new("MemoryError", "read result is too large"))?;
                out.extend_from_slice(data);
            }
            Ok(ctx.alloc(Value::Bytes(out)))
        }
        "readinto" => Err(Error::new("NotImplementedError", "")),
        _ => unreachable!("registered IOBase method"),
    }
}
