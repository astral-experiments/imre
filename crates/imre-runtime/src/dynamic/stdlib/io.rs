//! Native I/O classes for runtime-loaded modules.

use super::super::builtins::zeroed_bytes;
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

const DEFAULT_BUFFER_SIZE: i64 = 128 * 1024;

/// Install canonical class objects so subclasses inherit the same methods across imports.
pub(in super::super) fn install(ctx: &mut Context, module: Id) {
    let unsupported = ctx.builtin("UnsupportedOperation");
    ctx.bind(module, "UnsupportedOperation", unsupported);
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
    let text = class(ctx, module, "TextIOBase", io, &[]);
    let string_io = class(
        ctx,
        module,
        "StringIO",
        text,
        &["__init__", "write", "getvalue", "read", "tell", "isatty"],
    );
    ctx.builtins.insert("_io.StringIO".into(), string_io);
}

fn class(ctx: &mut Context, module: Id, name: &str, base: Id, methods: &[&'static str]) -> Id {
    let scope = ctx.scope("io", Some(module));
    let owner = ctx
        .allocate_class(name.into(), scope, vec![base])
        .expect("a single I/O base has a linearization");
    let module_name = ctx.text(if name == "StringIO" { "_io" } else { "io" });
    ctx.bind(scope, "__module__", module_name);
    let qualname = ctx.text(name);
    ctx.bind(scope, "__qualname__", qualname);
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
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound I/O method {name} needs a receiver"),
        ));
    };
    if !ctx.instance_of(receiver, owner) {
        return Err(Error::new(
            "TypeError",
            format!("invalid receiver for {name}"),
        ));
    }
    if name == "__init__" {
        return string_io_init(ctx, receiver, rest, kwargs);
    }
    if ctx
        .builtins
        .get("_io.StringIO")
        .is_some_and(|&class| ctx.instance_of(receiver, class))
    {
        let Value::Instance { text_memory, .. } = ctx.heap.get(receiver) else {
            unreachable!("StringIO methods have an instance receiver");
        };
        return string_io_method(ctx, text_memory.clone(), receiver, name, rest, kwargs);
    }
    let arity = match name {
        "read" => 1..=2,
        "readinto" => 2..=2,
        "__exit__" => 4..=4,
        _ => 1..=1,
    };
    if !arity.contains(&args.len()) || !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        ));
    }
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

/// Bind the C StringIO signature before installing the owned stream on the Python instance.
fn string_io_init(
    ctx: &mut Context,
    receiver: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let count = args.len() + kwargs.len();
    if count > 2 {
        return Err(Error::new(
            "TypeError",
            format!("StringIO() takes at most 2 arguments ({count} given)"),
        ));
    }
    let mut values = [None; 2];
    for (slot, arg) in values.iter_mut().zip(args) {
        *slot = Some(*arg);
    }
    for (key, value) in kwargs {
        let index = match key.as_str() {
            "initial_value" => 0,
            "newline" => 1,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("StringIO() got an unexpected keyword argument '{key}'"),
                ));
            }
        };
        if values[index].replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "argument for StringIO() given by name ('{key}') and position ({})",
                    index + 1
                ),
            ));
        }
    }
    let text = |id, name| match ctx.heap.get(id) {
        Value::None => Ok(None),
        Value::Str(value) => Ok(Some(value.as_str())),
        _ => Err(Error::new(
            "TypeError",
            format!("{name} must be str or None, not {}", ctx.type_name(id)),
        )),
    };
    let newline = match values[1] {
        None => Some("\n"),
        Some(id) => text(id, "newline")?,
    };
    let file = match text(values[0].unwrap_or(NONE), "initial_value") {
        Ok(initial) => crate::objects::TextFile::string_io(initial, newline)?,
        Err(error) => {
            // The C constructor validates the newline before the initial contents.
            crate::objects::TextFile::string_io(None, newline)?;
            return Err(error);
        }
    };
    let Value::Instance { text_memory, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!("StringIO.__init__ has an instance receiver");
    };
    *text_memory = Some(file);
    Ok(NONE)
}

fn string_io_method(
    ctx: &mut Context,
    file: Option<crate::objects::TextFile>,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("StringIO.{name}() takes no keyword arguments"),
        ));
    }
    let expected = match name {
        "write" if args.len() != 1 => Some("exactly one argument"),
        "read" if args.len() > 1 => {
            return Err(Error::new(
                "TypeError",
                format!("read expected at most 1 argument, got {}", args.len()),
            ));
        }
        "write" | "read" | "__exit__" => None,
        _ if !args.is_empty() => Some("no arguments"),
        _ => None,
    };
    if let Some(expected) = expected {
        return Err(Error::new(
            "TypeError",
            format!("StringIO.{name}() takes {expected} ({} given)", args.len()),
        ));
    }
    if name == "__exit__" {
        let close = ctx.lookup_attr(receiver, "close")?;
        return ctx.call(close, Vec::new(), Vec::new());
    }
    let Some(file) = file else {
        return match name {
            "close" | "flush" => Ok(NONE),
            _ => Err(Error::new(
                "ValueError",
                "I/O operation on uninitialized object",
            )),
        };
    };
    match name {
        "write" => {
            let Value::Str(text) = ctx.heap.get(args[0]) else {
                return Err(Error::new(
                    "TypeError",
                    format!("string argument expected, got '{}'", ctx.type_name(args[0])),
                ));
            };
            file.write(text)?;
            Ok(ctx.int(text.chars().count() as i64))
        }
        "getvalue" => Ok(ctx.text(file.getvalue()?.into_string())),
        "tell" => Ok(ctx.int(file.memory_position()? as i64)),
        "read" => {
            let size = match args.first().copied() {
                None | Some(NONE) => None,
                Some(id) => {
                    let integer = match ctx.heap.get(id) {
                        Value::Int(_) | Value::Bool(_) => id,
                        _ => {
                            let index = ctx.bound_slot(id, "__index__")?.ok_or_else(|| {
                                Error::new(
                                    "TypeError",
                                    format!(
                                        "argument should be integer or None, not '{}'",
                                        ctx.type_name(id)
                                    ),
                                )
                            })?;
                            ctx.call(index, vec![], vec![])?
                        }
                    };
                    Some(ctx.as_int(integer).map_err(|_| {
                        Error::new(
                            "TypeError",
                            format!(
                                "__index__ returned non-int (type {})",
                                ctx.type_name(integer)
                            ),
                        )
                    })?)
                }
            };
            Ok(ctx.text(file.read_memory(size.as_ref())?.into_string()))
        }
        "closed" => Ok(if file.closed() { TRUE } else { FALSE }),
        "flush" => {
            file.flush()?;
            Ok(NONE)
        }
        "close" => {
            file.close()?;
            Ok(NONE)
        }
        "__enter__" | "isatty" if file.closed() => {
            Err(Error::new("ValueError", "I/O operation on closed file."))
        }
        "__enter__" => Ok(receiver),
        "isatty" => Ok(FALSE),
        "readable" | "writable" | "seekable" if file.closed() => {
            Err(Error::new("ValueError", "I/O operation on closed file"))
        }
        "readable" | "writable" | "seekable" => Ok(TRUE),
        _ => unreachable!("registered StringIO method"),
    }
}
