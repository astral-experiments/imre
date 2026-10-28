//! Native I/O classes for runtime-loaded modules.

use super::super::builtins::zeroed_bytes;
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

mod bytes_io;
mod text_wrapper;

#[derive(Clone)]
pub(in crate::dynamic) enum MemoryStream {
    Text(crate::objects::TextFile),
    Bytes(std::rc::Rc<bytes_io::BytesIo>),
    Wrapper(std::rc::Rc<text_wrapper::Wrapper>),
}

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
            "__del__",
            "__iter__",
            "__next__",
            "readlines",
            "writelines",
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
    let buffered = class(ctx, module, "BufferedIOBase", io, &[]);
    let bytes_io = class(ctx, module, "BytesIO", buffered, bytes_io::METHODS);
    ctx.builtins.insert("_io.BytesIO".into(), bytes_io);
    let text = class(ctx, module, "TextIOBase", io, &[]);
    let wrapper = class(ctx, module, "TextIOWrapper", text, text_wrapper::METHODS);
    ctx.builtins.insert("_io.TextIOWrapper".into(), wrapper);
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
    let module_name = ctx.text(
        if matches!(name, "StringIO" | "BytesIO" | "TextIOWrapper") {
            "_io"
        } else {
            "io"
        },
    );
    ctx.bind(scope, "__module__", module_name);
    let qualname = ctx.text(name);
    ctx.bind(scope, "__qualname__", qualname);
    let wrapper = name == "TextIOWrapper";
    let descriptor_owner = match name {
        "TextIOWrapper" => "_io.TextIOWrapper",
        "BytesIO" => "_io.BytesIO",
        _ => "_io._IOBase",
    };
    for &name in methods {
        let method = ctx.alloc(Value::IoMethod { owner, name });
        let attr = if name == "closed" || wrapper && text_wrapper::property(name) {
            let set = (wrapper && name == "_CHUNK_SIZE").then(|| {
                ctx.alloc(Value::IoMethod {
                    owner,
                    name: "set_chunk_size",
                })
            });
            ctx.alloc(Value::GetSetDescriptor {
                get: method,
                set,
                owner: descriptor_owner,
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

fn argument_count(owner: &str, name: &str, n: usize) -> Result<()> {
    let message = match name {
        "read" | "read1" | "readline" | "readlines" | "truncate" if n > 1 => {
            format!("{name} expected at most 1 argument, got {n}")
        }
        "seek" if n == 0 => "seek expected at least 1 argument, got 0".into(),
        "seek" if n > 2 => format!("seek expected at most 2 arguments, got {n}"),
        "write" | "readinto" | "writelines" if n != 1 => {
            format!("{owner}.{name}() takes exactly one argument ({n} given)")
        }
        "read" | "read1" | "readline" | "readlines" | "writelines" | "seek" | "write"
        | "readinto" | "truncate" | "__exit__" => {
            return Ok(());
        }
        _ if n != 0 => format!("{owner}.{name}() takes no arguments ({n} given)"),
        _ => return Ok(()),
    };
    Err(Error::new("TypeError", message))
}

/// IOBase consumes the receiver's actual iterator and each writer override.
fn lines(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("_IOBase.{name}() takes no keyword arguments"),
        ));
    }
    argument_count("_IOBase", name, args.len())?;
    if matches!(name, "__iter__" | "writelines") {
        let closed = ctx.lookup_attr(receiver, "closed")?;
        if ctx.truth(closed)? {
            return Err(Error::new("ValueError", "I/O operation on closed file."));
        }
    }
    match name {
        "__iter__" => Ok(receiver),
        "__next__" => {
            let method = ctx.lookup_attr(receiver, "readline")?;
            let line = ctx.call(method, Vec::new(), Vec::new())?;
            let len = ctx.builtin("len");
            let length = ctx.call(len, vec![line], Vec::new())?;
            if ctx.truth(length)? {
                Ok(line)
            } else {
                Err(Error::new("StopIteration", ""))
            }
        }
        "writelines" => {
            let iterator = ctx.iterator(args[0])?;
            while let Some(line) = ctx.next_item(iterator)? {
                let write = ctx.lookup_attr(receiver, "write")?;
                ctx.call(write, vec![line], Vec::new())?;
            }
            Ok(NONE)
        }
        _ => {
            let mut hint = super::files::read_length(ctx, args.first().copied())?;
            let bounded = hint > 0;
            let iter = ctx.iterator(receiver)?;
            let mut result = Vec::new();
            while let Some(line) = ctx.next_item(iter)? {
                result
                    .try_reserve(1)
                    .map_err(|_| crate::Error::allocation())?;
                result.push(line);
                if bounded {
                    let len = ctx.builtin("len");
                    let length = ctx.call(len, vec![line], Vec::new())?;
                    hint -= ctx.as_integer(length)?;
                    if hint < 0 {
                        break;
                    }
                }
            }
            Ok(ctx.alloc(Value::List(result)))
        }
    }
}

/// Call an inherited I/O method with its bound receiver or an explicit class-level receiver.
pub(in super::super) fn call(
    ctx: &mut Context,
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let bytes_io = ctx.builtins.get("_io.BytesIO").copied();
    let Some((&receiver, rest)) = args.split_first() else {
        let message = if bytes_io == Some(owner) {
            if name == "__init__" {
                "descriptor '__init__' of '_io.BytesIO' object needs an argument".into()
            } else {
                format!("unbound method BytesIO.{name}() needs an argument")
            }
        } else {
            format!("unbound I/O method {name} needs a receiver")
        };
        return Err(Error::new("TypeError", message));
    };
    if !ctx.instance_of(receiver, owner) {
        let message = if bytes_io == Some(owner) {
            let actual = ctx.type_name(receiver);
            if name == "__init__" {
                format!(
                    "descriptor '__init__' requires a '_io.BytesIO' object but received a '{actual}'"
                )
            } else {
                format!(
                    "descriptor '{name}' for '_io.BytesIO' objects doesn't apply to a '{actual}' object"
                )
            }
        } else {
            format!("invalid receiver for {name}")
        };
        return Err(Error::new("TypeError", message));
    }
    if matches!(name, "__iter__" | "__next__" | "readlines" | "writelines") {
        return lines(ctx, receiver, name, rest, kwargs);
    }
    if name == "__del__" {
        if !rest.is_empty() || !kwargs.is_empty() {
            return Err(Error::new("TypeError", "expected no arguments to __del__"));
        }
        // A detached or partially initialized stream can no longer be closed.
        let closed = ctx
            .lookup_attr(receiver, "closed")
            .and_then(|id| ctx.truth(id));
        if matches!(closed, Ok(false)) {
            let _ = ctx.set_attr(receiver, "_finalizing", TRUE);
            if let Err(error) = ctx
                .lookup_attr(receiver, "close")
                .and_then(|f| ctx.call(f, Vec::new(), Vec::new()))
            {
                eprintln!(
                    "Exception ignored while finalizing file {}: {error}",
                    ctx.repr(receiver)
                );
            }
        }
        return Ok(NONE);
    }
    if ctx
        .builtins
        .get("_io.TextIOWrapper")
        .is_some_and(|&class| ctx.instance_of(receiver, class))
    {
        if matches!(ctx.heap.get(receiver), Value::TextFile(_)) {
            return super::files::text_method(ctx, name, args, kwargs);
        }
        return text_wrapper::call(ctx, receiver, name, rest, kwargs);
    }
    if bytes_io == Some(owner)
        || matches!(name, "__enter__" | "__exit__")
            && bytes_io.is_some_and(|class| ctx.instance_of(receiver, class))
    {
        return bytes_io::call(ctx, receiver, name, rest, kwargs);
    }
    if name == "__init__" {
        return string_io_init(ctx, receiver, rest, kwargs);
    }
    if ctx
        .builtins
        .get("_io.StringIO")
        .is_some_and(|&class| ctx.instance_of(receiver, class))
    {
        let Value::Instance { memory_stream, .. } = ctx.heap.get(receiver) else {
            unreachable!("StringIO methods have an instance receiver");
        };
        let file = match memory_stream {
            Some(MemoryStream::Text(file)) => Some(file.clone()),
            _ => None,
        };
        return string_io_method(ctx, file, receiver, name, rest, kwargs);
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
    let Value::Instance { .. } = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected an IOBase instance"));
    };
    let closed = ctx
        .raw_instance_field(receiver, "__IOBase_closed")
        .is_some();
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
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!("StringIO.__init__ has an instance receiver");
    };
    *memory_stream = Some(MemoryStream::Text(file));
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
