//! Built-ins used by native Python code whose values are known at runtime.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Write};
use std::rc::Rc;

use super::Context;
use super::value::{Error, FALSE, Id, NONE, Result, TRUE, Value};

mod collections;
mod text;
pub(super) use collections::{hashable, view_items};
pub(super) use text::{bytes_from_iterable, format_value, percent, zeroed_bytes};

pub(super) fn is_exception(name: &str) -> bool {
    matches!(
        name,
        "BaseException"
            | "Exception"
            | "RuntimeError"
            | "RecursionError"
            | "NotImplementedError"
            | "TypeError"
            | "ValueError"
            | "UnicodeError"
            | "UnicodeDecodeError"
            | "UnicodeEncodeError"
            | "ArithmeticError"
            | "OverflowError"
            | "ZeroDivisionError"
            | "LookupError"
            | "IndexError"
            | "KeyError"
            | "NameError"
            | "AttributeError"
            | "ImportError"
            | "ModuleNotFoundError"
            | "OSError"
            | "ConnectionError"
            | "ConnectionRefusedError"
            | "ConnectionResetError"
            | "ConnectionAbortedError"
            | "BrokenPipeError"
            | "BlockingIOError"
            | "TimeoutError"
            | "FileNotFoundError"
            | "PermissionError"
            | "AssertionError"
            | "MemoryError"
            | "SystemExit"
            | "StopIteration"
            | "Warning"
            | "RuntimeWarning"
    )
}

pub(super) fn is_builtin(name: &str) -> bool {
    is_builtin_type(name)
        || matches!(
            name,
            "len"
                | "callable"
                | "isinstance"
                | "getattr"
                | "setattr"
                | "hasattr"
                | "open"
                | "print"
                | "__import__"
                | "repr"
                | "format"
        )
}

pub(super) fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "bytearray"
            | "bytes"
            | "dict"
            | "int"
            | "list"
            | "object"
            | "property"
            | "range"
            | "set"
            | "str"
            | "tuple"
            | "type"
    ) || is_exception(name)
}

pub(super) fn method_name(value: &Value, attr: &str) -> Option<String> {
    let owner = match value {
        Value::Str(_)
            if matches!(
                attr,
                "format"
                    | "split"
                    | "rsplit"
                    | "splitlines"
                    | "strip"
                    | "lstrip"
                    | "rstrip"
                    | "startswith"
                    | "endswith"
                    | "replace"
                    | "join"
                    | "lower"
                    | "upper"
                    | "removeprefix"
                    | "removesuffix"
                    | "encode"
            ) =>
        {
            "str"
        }
        Value::Bytes(_) if attr == "decode" => "bytes",
        Value::ByteArray(_) if attr == "extend" => "bytearray",
        Value::NamedTuple {
            class: Some("SplitResult" | "SplitResultBytes"),
            ..
        } if attr == "geturl" => "urllib.parse.SplitResult",
        Value::Hash(_) if matches!(attr, "update" | "copy" | "digest" | "hexdigest") => "hash",
        Value::Hmac(_) if matches!(attr, "update" | "copy" | "digest" | "hexdigest") => "hmac.HMAC",
        Value::Socket(_)
            if matches!(
                attr,
                "bind"
                    | "listen"
                    | "accept"
                    | "connect"
                    | "getsockname"
                    | "fileno"
                    | "close"
                    | "recv"
                    | "recv_into"
                    | "makefile"
                    | "send"
                    | "sendall"
                    | "shutdown"
                    | "setsockopt"
                    | "setblocking"
                    | "getblocking"
                    | "__enter__"
                    | "__exit__"
            ) =>
        {
            "socket.socket"
        }
        Value::List(_) if matches!(attr, "append" | "extend" | "insert") => "list",
        Value::Set(_) if matches!(attr, "add" | "update") => "set",
        Value::Dict(_) | Value::DefaultDict { .. }
            if matches!(
                attr,
                "items" | "values" | "keys" | "get" | "setdefault" | "update"
            ) =>
        {
            "dict"
        }
        Value::Selector(_)
            if matches!(
                attr,
                "register"
                    | "modify"
                    | "unregister"
                    | "select"
                    | "get_key"
                    | "close"
                    | "__enter__"
                    | "__exit__"
            ) =>
        {
            "selectors"
        }
        Value::File(_)
            if matches!(
                attr,
                "read"
                    | "fileno"
                    | "readline"
                    | "readinto"
                    | "seek"
                    | "tell"
                    | "close"
                    | "__enter__"
                    | "__exit__"
            ) =>
        {
            "file"
        }
        Value::RegexPattern(_) if matches!(attr, "findall" | "match") => "pattern",
        Value::RegexMatch { .. } if attr == "group" => "match",
        Value::Function { .. } | Value::Cached { .. } | Value::Partial { .. }
            if attr == "__get__" =>
        {
            "function"
        }
        Value::ContextVar { .. } if matches!(attr, "get" | "set" | "reset") => {
            "contextvars.ContextVar"
        }
        Value::Context(_)
            if matches!(attr, "copy" | "run" | "get" | "keys" | "values" | "items") =>
        {
            "contextvars.Context"
        }
        Value::ContextToken { .. } if matches!(attr, "__enter__" | "__exit__") => {
            "contextvars.Token"
        }
        _ => return None,
    };
    Some(format!("{owner}.{attr}"))
}

fn type_error(message: impl Into<String>) -> Error {
    Error::new("TypeError", message)
}

fn arity(name: &str, args: &[Id], min: usize, max: usize) -> Result<()> {
    if !(min..=max).contains(&args.len()) {
        return Err(type_error(format!(
            "{name} received {} positional arguments; expected {min}..={max}",
            args.len()
        )));
    }
    Ok(())
}

fn positional(name: &str, kwargs: &[(String, Id)]) -> Result<()> {
    if let Some((key, _)) = kwargs.first() {
        Err(type_error(format!(
            "{name} does not accept keyword argument {key:?}"
        )))
    } else {
        Ok(())
    }
}

pub(super) fn bind<const N: usize>(
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    names: [&str; N],
) -> Result<[Option<Id>; N]> {
    arity(name, args, 0, N)?;
    let mut values = [None; N];
    for (slot, &argument) in values.iter_mut().zip(args) {
        *slot = Some(argument);
    }
    for (key, argument) in kwargs {
        let index = names.iter().position(|arg| *arg == key).ok_or_else(|| {
            type_error(format!("{name} got an unexpected keyword argument {key:?}"))
        })?;
        if values[index].replace(*argument).is_some() {
            return Err(type_error(format!(
                "{name} got multiple values for argument {key:?}"
            )));
        }
    }
    Ok(values)
}

fn boolean(value: bool) -> Id {
    if value { TRUE } else { FALSE }
}

fn set_attribute(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let base = name == "object.__setattr__";
    if base && args.is_empty() {
        return Err(type_error(
            "descriptor '__setattr__' of 'object' object needs an argument",
        ));
    }
    if base && matches!(ctx.heap.get(args[0]), Value::Class { .. }) {
        return Err(type_error("can't apply this __setattr__ to type object"));
    }
    if !kwargs.is_empty() {
        return Err(type_error(if base {
            "wrapper __setattr__() takes no keyword arguments"
        } else {
            "setattr() takes no keyword arguments"
        }));
    }
    let [obj, attr, value] = args else {
        return Err(type_error(if base {
            format!("__setattr__ expected 2 arguments, got {}", args.len() - 1)
        } else {
            format!("setattr expected 3 arguments, got {}", args.len())
        }));
    };
    let Value::Str(attr_name) = ctx.heap.get(*attr) else {
        return Err(type_error(format!(
            "attribute name must be string, not '{}'",
            ctx.type_name(*attr)
        )));
    };
    let attr_name = attr_name.clone();
    if base {
        ctx.default_set_attr(*obj, &attr_name, *value)?;
    } else {
        ctx.set_attr(*obj, &attr_name, *value)?;
    }
    Ok(NONE)
}

/// Compute Python's range length without overflowing at either i64 endpoint.
pub(super) fn range_len(start: i64, stop: i64, step: i64) -> Result<usize> {
    if step == 0 {
        return Err(Error::new("ValueError", "range() arg 3 must not be zero"));
    }
    let delta = if step > 0 {
        i128::from(stop) - i128::from(start)
    } else {
        i128::from(start) - i128::from(stop)
    };
    let len = if delta <= 0 {
        0
    } else {
        1 + (delta - 1) / i128::from(step).abs()
    };
    usize::try_from(len).map_err(|_| Error::new("OverflowError", "range length is too large"))
}

pub(super) fn range_items(ctx: &mut Context, start: i64, stop: i64, step: i64) -> Result<Vec<Id>> {
    let size = range_len(start, stop, step)?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(size)
        .map_err(|_| Error::new("MemoryError", "range is too large to materialize"))?;
    for index in 0..size {
        let value = i128::from(start) + i128::from(step) * index as i128;
        result.push(ctx.int(value as i64));
    }
    Ok(result)
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if matches!(name, "setattr" | "object.__setattr__") {
        return set_attribute(ctx, name, args, kwargs);
    }
    if let Some((owner, method)) = name.split_once('.') {
        return match owner {
            "str" | "bytes" => text::method(ctx, method, args, kwargs),
            "bytearray" if method == "extend" => {
                positional(name, kwargs)?;
                arity(name, args, 2, 2)?;
                let data = text::bytes_from_iterable(ctx, args[1])?;
                let Value::ByteArray(target) = ctx.heap.get_mut(args[0]) else {
                    return Err(type_error("expected bytearray"));
                };
                target
                    .try_reserve(data.len())
                    .map_err(|_| Error::new("MemoryError", "bytearray is too large"))?;
                target.extend(data);
                Ok(NONE)
            }
            "list" | "dict" | "set" => collections::method(ctx, name, args, kwargs),
            "function" if method == "__get__" => {
                positional(name, kwargs)?;
                arity(name, args, 2, 3)?;
                let owner = args.get(2).copied().unwrap_or(NONE);
                if args[1] == NONE && owner == NONE {
                    return Err(type_error("__get__(None, None) is invalid"));
                }
                ctx.bind_attribute(args[0], args[1], owner)
            }
            _ => Err(type_error(format!("unknown builtin method {name}"))),
        };
    }
    match name {
        "type" => {
            let [object] = args else {
                return Err(if args.len() == 3 {
                    Error::new(
                        "NotImplementedError",
                        "dynamic class construction with type() is unsupported",
                    )
                } else {
                    type_error("type() takes 1 or 3 arguments")
                });
            };
            if !kwargs.is_empty() {
                return Err(type_error("type() takes no keyword arguments"));
            }
            ctx.exact_type(*object)
        }
        "callable" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            Ok(boolean(ctx.callable(args[0])))
        }
        "getattr" | "hasattr" => {
            positional(name, kwargs)?;
            arity(name, args, 2, if name == "getattr" { 3 } else { 2 })?;
            let attr = ctx.str_value(args[1])?.to_owned();
            match ctx.lookup_attr(args[0], &attr) {
                Ok(_) if name == "hasattr" => Ok(TRUE),
                Err(e) if e.kind == "AttributeError" && name == "hasattr" => Ok(FALSE),
                Err(e) if e.kind == "AttributeError" && args.len() == 3 => Ok(args[2]),
                result => result,
            }
        }
        "isinstance" => {
            positional(name, kwargs)?;
            arity(name, args, 2, 2)?;
            Ok(boolean(ctx.instance_of(args[0], args[1])))
        }
        "len" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            let size = match ctx.heap.get(args[0]) {
                Value::List(v) | Value::Tuple(v) | Value::Set(v) => v.len(),
                Value::NamedTuple { values, .. } => values.len(),
                Value::Context(bindings) => bindings.len(),
                Value::Dict(v) | Value::DefaultDict { entries: v, .. } => v.len(),
                Value::DictView { dict, .. } => collections::entries(ctx, *dict)?.len(),
                Value::Str(v) => v.chars().count(),
                Value::Bytes(v) | Value::ByteArray(v) => v.len(),
                Value::Range { start, stop, step } => range_len(*start, *stop, *step)?,
                Value::Instance { class, .. } if ctx.namedtuple_fields(*class).is_some() => {
                    ctx.items(args[0])?.len()
                }
                Value::Instance { .. } => ctx
                    .instance_len(args[0])?
                    .ok_or_else(|| type_error("object has no len()"))?,
                _ => return Err(type_error("object has no len()")),
            };
            let size = i64::try_from(size).map_err(|_| {
                Error::new("OverflowError", "length does not fit in a Python index")
            })?;
            Ok(ctx.int(size))
        }
        "range" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 3)?;
            let first = ctx.as_integer(args[0])?;
            let (start, stop) = if let Some(&stop) = args.get(1) {
                (first, ctx.as_integer(stop)?)
            } else {
                (0, first)
            };
            let step = args
                .get(2)
                .map(|&x| ctx.as_integer(x))
                .transpose()?
                .unwrap_or(1);
            range_len(start, stop, step)?;
            Ok(ctx.alloc(Value::Range { start, stop, step }))
        }
        "dict" | "list" | "tuple" | "set" => collections::construct(ctx, name, args, kwargs),
        "int" | "bool" | "str" | "bytes" | "bytearray" | "repr" => {
            text::construct(ctx, name, args, kwargs)
        }
        "format" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 2)?;
            let spec = args
                .get(1)
                .map(|&x| ctx.str_value(x))
                .transpose()?
                .unwrap_or("")
                .to_owned();
            let value = format_value(ctx, args[0], &spec)?;
            Ok(ctx.text(value))
        }
        "property" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            Ok(ctx.alloc(Value::Property(args[0])))
        }
        "open" => {
            let [path, mode] = bind(name, args, kwargs, ["file", "mode"])?;
            let path = ctx.str_value(path.ok_or_else(|| type_error("open requires a file"))?)?;
            let mode = mode.map(|id| ctx.str_value(id)).transpose()?.unwrap_or("r");
            if mode != "rb" {
                return Err(Error::new(
                    "ValueError",
                    "only binary read mode is supported for dynamic files",
                ));
            }
            let file = File::open(path).map_err(|e| {
                let kind = match e.kind() {
                    io::ErrorKind::NotFound => "FileNotFoundError",
                    io::ErrorKind::PermissionDenied => "PermissionError",
                    _ => "OSError",
                };
                Error::new(kind, format!("{path}: {e}"))
            })?;
            Ok(ctx.alloc(Value::File(Rc::new(RefCell::new(Some(
                io::BufReader::new(super::stdlib::ReadSource::File(file)),
            ))))))
        }
        "__import__" => {
            let [module, _, _, fromlist, level] = bind(
                name,
                args,
                kwargs,
                ["name", "globals", "locals", "fromlist", "level"],
            )?;
            let name = ctx
                .str_value(module.ok_or_else(|| type_error("__import__ requires a name"))?)?
                .to_owned();
            if level.map(|v| ctx.as_integer(v)).transpose()?.unwrap_or(0) != 0 {
                return Err(Error::new(
                    "ImportError",
                    "explicit relative __import__ is unsupported",
                ));
            }
            let result = ctx.module(&name)?;
            if ctx.optional_truth(fromlist, false)? {
                return Ok(result);
            }
            ctx.module(name.split('.').next().unwrap())
        }
        "print" => {
            let [sep, end, file, flush] = bind(name, &[], kwargs, ["sep", "end", "file", "flush"])?;
            let flush = ctx.optional_truth(flush, false)?;
            if file.is_some_and(|id| id != NONE) {
                return Err(type_error("print supports standard output"));
            }
            let sep = sep
                .filter(|&id| id != NONE)
                .map(|id| ctx.str_value(id))
                .transpose()?
                .unwrap_or(" ");
            let end = end
                .filter(|&id| id != NONE)
                .map(|id| ctx.str_value(id))
                .transpose()?
                .unwrap_or("\n");
            let values = args
                .iter()
                .map(|&id| ctx.display(id))
                .collect::<Vec<_>>()
                .join(sep);
            let mut out = io::stdout().lock();
            write!(out, "{values}{end}")
                .and_then(|()| if flush { out.flush() } else { Ok(()) })
                .map_err(|err| Error::new("OSError", err.to_string()))?;
            Ok(NONE)
        }
        name if is_exception(name) => {
            positional(name, kwargs)?;
            let error = ctx.exception(name, args);
            Ok(ctx.alloc(Value::Raised(error)))
        }
        name => Err(type_error(format!("unsupported builtin {name}"))),
    }
}
