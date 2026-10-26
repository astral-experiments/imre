//! Built-ins used by native Python code whose values are known at runtime.

use super::Context;
use super::value::{Error, FALSE, Id, NONE, Result, TRUE, Value};

mod collections;
mod float;
pub(super) use float::from_numeric;
mod integer;
mod print;
mod text;
mod translation;
pub(super) use collections::{hashable, remove_dict_item, set_add, set_member};
pub(super) use text::{bytes_from_iterable, bytes_percent, format_value, percent, zeroed_bytes};

macro_rules! exception_kinds {
    ($($kind:ident),* $(,)?) => {
        const EXCEPTIONS: &[&str] = &[$(stringify!($kind)),*];
        pub(super) fn exception_kind(name: &str) -> Option<crate::objects::exceptions::Kind> {
            use crate::objects::exceptions::Kind;
            match name {
                $(stringify!($kind) => Some(Kind::$kind),)*
                "UnsupportedOperation" => Some(Kind::UnsupportedOperation),
                "binascii.Error" => Some(Kind::BinasciiError),
                "binascii.Incomplete" => Some(Kind::BinasciiIncomplete),
                _ => None,
            }
        }
    };
}

exception_kinds!(
    BaseException,
    Exception,
    RuntimeError,
    RecursionError,
    NotImplementedError,
    TypeError,
    ValueError,
    EOFError,
    UnicodeError,
    UnicodeDecodeError,
    UnicodeEncodeError,
    ArithmeticError,
    OverflowError,
    ZeroDivisionError,
    LookupError,
    IndexError,
    KeyError,
    NameError,
    UnboundLocalError,
    AttributeError,
    ImportError,
    ModuleNotFoundError,
    OSError,
    ConnectionError,
    ConnectionRefusedError,
    ConnectionResetError,
    ConnectionAbortedError,
    BrokenPipeError,
    BlockingIOError,
    TimeoutError,
    FileNotFoundError,
    FileExistsError,
    IsADirectoryError,
    NotADirectoryError,
    PermissionError,
    AssertionError,
    MemoryError,
    BufferError,
    SystemExit,
    StopIteration,
    Warning,
    RuntimeWarning,
);

const FUNCTIONS: &[&str] = &[
    "len",
    "reversed",
    "sorted",
    "ord",
    "chr",
    "divmod",
    "callable",
    "all",
    "any",
    "isinstance",
    "issubclass",
    "getattr",
    "globals",
    "setattr",
    "delattr",
    "hasattr",
    "open",
    "print",
    "sum",
    "__import__",
    "repr",
    "format",
    "compile",
    "exec",
    "iter",
    "next",
];

const TYPES: &[&str] = &[
    "bool",
    "bytearray",
    "bytes",
    "complex",
    "dict",
    "enumerate",
    "float",
    "frozenset",
    "int",
    "list",
    "map",
    "object",
    "property",
    "classmethod",
    "staticmethod",
    "range",
    "set",
    "slice",
    "str",
    "tuple",
    "type",
    "super",
    "zip",
];

pub(super) fn names() -> impl Iterator<Item = &'static str> {
    FUNCTIONS.iter().chain(TYPES).chain(EXCEPTIONS).copied()
}

pub(super) fn is_exception(name: &str) -> bool {
    exception_kind(name).is_some()
}

/// Consult the native hierarchy when both names denote known exception types.
pub(super) fn exception_subclass(child: &str, parent: &str) -> Option<bool> {
    Some(crate::objects::exceptions::imre_builtin_exception_subclass(
        exception_kind(child)? as u32,
        exception_kind(parent)? as u32,
    ))
}

pub(super) fn is_builtin(name: &str) -> bool {
    (name != "UnsupportedOperation" && is_builtin_type(name)) || FUNCTIONS.contains(&name)
}

/// Static constructors retain their declaring type as `__self__` even on instances.
pub(super) fn constructor_owner(name: &str) -> Option<&'static str> {
    match name {
        "float.__new__" => Some("float"),
        "int.__new__" => Some("int"),
        "str.__new__" => Some("str"),
        _ => None,
    }
}

pub(super) fn is_builtin_type(name: &str) -> bool {
    TYPES.contains(&name)
        || is_exception(name)
        || matches!(name, "enum.Enum" | "enum.IntEnum" | "enum.StrEnum")
}

pub(super) const DICT_METHODS: &[&str] = &[
    "__init__",
    "__len__",
    "__iter__",
    "__contains__",
    "__getitem__",
    "__setitem__",
    "get",
    "items",
    "keys",
    "pop",
    "setdefault",
    "update",
    "values",
];

pub(super) const STR_METHODS: &[&str] = &[
    "capitalize",
    "encode",
    "endswith",
    "expandtabs",
    "find",
    "format",
    "isspace",
    "join",
    "lower",
    "lstrip",
    "partition",
    "removeprefix",
    "removesuffix",
    "replace",
    "rfind",
    "rpartition",
    "rsplit",
    "rstrip",
    "split",
    "splitlines",
    "startswith",
    "strip",
    "translate",
    "upper",
];

pub(super) fn method_name(value: &Value, attr: &str) -> Option<String> {
    let owner = match value {
        Value::PrimitiveLock(_) if super::thread_lock::is_method(attr) => super::thread_lock::CLASS,
        Value::Int(_) | Value::Bool(_) if attr == "to_bytes" => "int",
        Value::Str(_) if STR_METHODS.contains(&attr) => "str",
        Value::Slice(_) if matches!(attr, "indices" | "__reduce__") => "slice",
        Value::Property(_) if attr == "setter" => "property",
        Value::StaticMethod { .. } if attr == "__get__" => "staticmethod",
        Value::Bytes(_)
            if matches!(attr, "decode" | "rfind" | "rsplit" | "rstrip" | "translate") =>
        {
            "bytes"
        }
        Value::ByteArray(_) if matches!(attr, "extend" | "translate") => "bytearray",
        Value::NamedTuple {
            class: Some("SplitResult" | "SplitResultBytes"),
            ..
        } if attr == "geturl" => "urllib.parse.SplitResult",
        Value::Struct { .. } if matches!(attr, "pack" | "unpack") => "struct.Struct",
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
        Value::TextFile(_)
            if matches!(
                attr,
                "read"
                    | "readline"
                    | "write"
                    | "flush"
                    | "close"
                    | "isatty"
                    | "__enter__"
                    | "__exit__"
            ) =>
        {
            "TextIOWrapper"
        }
        Value::List(_)
            if matches!(
                attr,
                "append" | "clear" | "extend" | "insert" | "pop" | "reverse" | "sort"
            ) =>
        {
            "list"
        }
        Value::Set { frozen: false, .. }
            if matches!(attr, "add" | "update" | "issuperset" | "__contains__") =>
        {
            "set"
        }
        Value::Set { frozen: true, .. }
            if matches!(attr, "copy" | "issuperset" | "__contains__") =>
        {
            "frozenset"
        }
        Value::Dict(_) | Value::DefaultDict { .. } if DICT_METHODS.contains(&attr) => "dict",
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
                    | "write"
                    | "flush"
                    | "close"
                    | "__enter__"
                    | "__exit__"
            ) =>
        {
            "file"
        }
        Value::File(file)
            if matches!(attr, "seek" | "tell")
                && matches!(file.as_ref(), super::stdlib::BinaryFile::Reader(_)) =>
        {
            "file"
        }
        Value::RegexPattern(_) if matches!(attr, "findall" | "match" | "fullmatch" | "search") => {
            "pattern"
        }
        Value::RegexMatch { .. } if matches!(attr, "group" | "groups") => "match",
        Value::Map { .. } if matches!(attr, "__iter__" | "__next__") => "map",
        Value::Enumerate { .. } if matches!(attr, "__iter__" | "__next__") => "enumerate",
        Value::Count { .. } if matches!(attr, "__iter__" | "__next__") => "itertools.count",
        Value::Repeat(_) if matches!(attr, "__iter__" | "__next__" | "__length_hint__") => {
            "itertools.repeat"
        }
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

/// Bind positional and keyword method arguments using Argument Clinic's diagnostics.
pub(super) fn bind_method_arguments<const N: usize>(
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    names: [&str; N],
    max_positional: usize,
) -> Result<[Option<Id>; N]> {
    let count = args.len() + kwargs.len();
    if count > N {
        let kind = if args.is_empty() { "keyword " } else { "" };
        return Err(type_error(format!(
            "{name}() takes at most {N} {kind}arguments ({count} given)"
        )));
    }
    if args.len() > max_positional {
        return Err(type_error(format!(
            "{name}() takes at most {max_positional} positional arguments ({} given)",
            args.len()
        )));
    }
    for (i, key) in names[..args.len()].iter().enumerate() {
        if kwargs.iter().any(|(provided, _)| provided == key) {
            return Err(type_error(format!(
                "argument for {name}() given by name ('{key}') and position ({})",
                i + 1
            )));
        }
    }
    let mut values = std::array::from_fn(|i| args.get(i).copied());
    for (key, value) in kwargs {
        let index = names.iter().position(|&arg| arg == key).ok_or_else(|| {
            type_error(format!(
                "{name}() got an unexpected keyword argument '{key}'"
            ))
        })?;
        values[index] = Some(*value);
    }
    Ok(values)
}

pub(super) fn boolean(value: bool) -> Id {
    if value { TRUE } else { FALSE }
}

fn attribute_name(ctx: &Context, value: Id) -> Result<String> {
    ctx.str_value(value).map(str::to_owned).map_err(|_| {
        type_error(format!(
            "attribute name must be string, not '{}'",
            ctx.type_name(value)
        ))
    })
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
    let attr_name = attribute_name(ctx, *attr)?;
    if base {
        ctx.default_set_attr(*obj, &attr_name, *value)?;
    } else {
        ctx.set_attr(*obj, &attr_name, *value)?;
    }
    Ok(NONE)
}

fn delete_attribute(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let base = name == "object.__delattr__";
    if base && args.is_empty() {
        return Err(type_error(
            "descriptor '__delattr__' of 'object' object needs an argument",
        ));
    }
    if !kwargs.is_empty() {
        return Err(type_error(if base {
            "wrapper __delattr__() takes no keyword arguments"
        } else {
            "delattr() takes no keyword arguments"
        }));
    }
    let [obj, attr] = args else {
        return Err(type_error(if base {
            format!("expected 1 argument, got {}", args.len() - 1)
        } else {
            format!("delattr expected 2 arguments, got {}", args.len())
        }));
    };
    if base && ctx.builtin_type_name(*obj) == Some("type") {
        return Err(type_error("can't apply this __delattr__ to type object"));
    }
    let attr = attribute_name(ctx, *attr)?;
    if base {
        ctx.default_delete_attr(*obj, &attr)?;
    } else {
        ctx.delete_attr(*obj, &attr)?;
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
    if name == "object.__getattribute__" {
        let Some((&object, values)) = args.split_first() else {
            return Err(type_error(
                "descriptor '__getattribute__' of 'object' object needs an argument",
            ));
        };
        if !kwargs.is_empty() {
            return Err(type_error(
                "wrapper __getattribute__() takes no keyword arguments",
            ));
        }
        let [key] = values else {
            return Err(type_error(format!(
                "expected 1 argument, got {}",
                values.len()
            )));
        };
        let attr = attribute_name(ctx, *key)?;
        return ctx.default_get_attr(object, &attr);
    }
    if matches!(name, "setattr" | "object.__setattr__") {
        return set_attribute(ctx, name, args, kwargs);
    }
    if matches!(name, "delattr" | "object.__delattr__") {
        return delete_attribute(ctx, name, args, kwargs);
    }
    if name == "object.__init__" {
        if args.len() != 1 || !kwargs.is_empty() {
            return Err(type_error(
                "object.__init__() takes exactly one argument (the instance to initialize)",
            ));
        }
        return Ok(NONE);
    }
    if let Some((owner, method)) = name.split_once('.') {
        return match owner {
            "int" if method == "to_bytes" => integer::to_bytes(ctx, args, kwargs),
            "bytes" | "bytearray" if method == "maketrans" => {
                translation::maketrans(ctx, name, args, kwargs)
            }
            "bytes" | "bytearray" if method == "translate" => {
                translation::translate(ctx, owner, args, kwargs)
            }
            "str" | "bytes" => text::method(ctx, owner, method, args, kwargs),
            "bytearray" if method == "extend" => {
                positional(name, kwargs)?;
                arity(name, args, 2, 2)?;
                let data = text::bytes_from_iterable(ctx, args[1])?;
                if !data.is_empty() {
                    ctx.check_bytearray_resize(args[0])?;
                }
                let Value::ByteArray(target) = ctx.heap.get_mut(args[0]) else {
                    return Err(type_error("expected bytearray"));
                };
                target
                    .try_reserve(data.len())
                    .map_err(|_| Error::new("MemoryError", "bytearray is too large"))?;
                target.extend(data);
                Ok(NONE)
            }
            "list" | "dict" | "set" | "frozenset" => collections::method(ctx, name, args, kwargs),
            "slice" => super::slice::method(ctx, method, args, kwargs),
            "map" | "enumerate" => ctx.iterator_method(owner, method, args, kwargs),
            "property" if method == "setter" => super::property::setter(ctx, args, kwargs),
            "function" | "staticmethod" if method == "__get__" => {
                positional(name, kwargs)?;
                arity(name, args, 2, 3)?;
                if owner == "staticmethod"
                    && !matches!(ctx.heap.get(args[0]), Value::StaticMethod { .. })
                {
                    return Err(type_error(format!(
                        "descriptor '__get__' requires a 'staticmethod' object but received a '{}'",
                        ctx.type_name(args[0])
                    )));
                }
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
        "method" => {
            if !kwargs.is_empty() {
                return Err(type_error("method() takes no keyword arguments"));
            }
            let &[callable, receiver] = args else {
                return Err(type_error(format!(
                    "method expected 2 arguments, got {}",
                    args.len()
                )));
            };
            if !ctx.callable(callable) {
                return Err(type_error("first argument must be callable"));
            }
            if receiver == NONE {
                return Err(type_error("instance must not be None"));
            }
            Ok(ctx.alloc(Value::Method {
                callable,
                receiver,
                explicit: true,
            }))
        }
        "object" => {
            if !args.is_empty() || !kwargs.is_empty() {
                return Err(type_error("object() takes no arguments"));
            }
            let class = ctx.builtin("object");
            Ok(ctx.instance(class, None))
        }
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
        "super" => {
            if !kwargs.is_empty() {
                return Err(type_error("super() takes no keyword arguments"));
            }
            ctx.make_super(args)
        }
        "compile" => {
            positional(name, kwargs)?;
            arity(name, args, 3, 3)?;
            ctx.compile_source(args[0], args[1], args[2])
        }
        "exec" => {
            positional(name, kwargs)?;
            arity(name, args, 2, 3)?;
            ctx.exec_source(args[0], args[1], args.get(2).copied())
        }
        "callable" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            Ok(boolean(ctx.callable(args[0])))
        }
        "globals" => {
            if !kwargs.is_empty() {
                return Err(type_error("globals() takes no keyword arguments"));
            }
            if !args.is_empty() {
                return Err(type_error(format!(
                    "globals() takes no arguments ({} given)",
                    args.len()
                )));
            }
            Ok(ctx.module_globals(ctx.heap.current_scope()))
        }
        "getattr" | "hasattr" => {
            positional(name, kwargs)?;
            arity(name, args, 2, if name == "getattr" { 3 } else { 2 })?;
            let attr = attribute_name(ctx, args[1])?;
            match ctx.lookup_attr(args[0], &attr) {
                Ok(_) if name == "hasattr" => Ok(TRUE),
                Err(e) if name == "hasattr" && ctx.is_attribute_error(&e) => Ok(FALSE),
                Err(e) if args.len() == 3 && ctx.is_attribute_error(&e) => Ok(args[2]),
                result => result,
            }
        }
        "issubclass" => {
            positional(name, kwargs)?;
            arity(name, args, 2, 2)?;
            Ok(boolean(ctx.python_subclass_check(args[0], args[1])?))
        }
        "isinstance" => {
            positional(name, kwargs)?;
            arity(name, args, 2, 2)?;
            Ok(boolean(ctx.python_instance_check(args[0], args[1])?))
        }
        "divmod" => {
            if !kwargs.is_empty() {
                return Err(type_error("divmod() takes no keyword arguments"));
            }
            let [a, b] = args else {
                return Err(type_error(format!(
                    "divmod expected 2 arguments, got {}",
                    args.len()
                )));
            };
            let (Ok(left), Ok(right)) = (ctx.as_int(*a), ctx.as_int(*b)) else {
                return Err(type_error(format!(
                    "unsupported operand type(s) for divmod(): '{}' and '{}'",
                    ctx.type_name(*a),
                    ctx.type_name(*b)
                )));
            };
            let quotient = ctx.alloc(Value::Int(crate::integer_binary(&left, &right, 3)?));
            let remainder = ctx.alloc(Value::Int(crate::integer_binary(&left, &right, 4)?));
            Ok(ctx.alloc(Value::Tuple(vec![quotient, remainder])))
        }
        "chr" | "ord" => {
            if !kwargs.is_empty() {
                return Err(type_error(format!("{name}() takes no keyword arguments")));
            }
            let [value] = args else {
                return Err(type_error(format!(
                    "{name}() takes exactly one argument ({} given)",
                    args.len()
                )));
            };
            if name == "chr" {
                let code = ctx.index_integer(*value)?;
                return Ok(ctx.text(crate::objects::chr_scalar(&code)?.to_string()));
            }
            let code = match ctx.heap.get(*value) {
                Value::Bytes(b) | Value::ByteArray(b) => crate::objects::ord_byte(b)?,
                _ => {
                    let text = ctx.str_value(*value).map_err(|_| {
                        type_error(format!(
                            "ord() expected string of length 1, but {} found",
                            ctx.type_name(*value)
                        ))
                    })?;
                    crate::objects::ord_char(text)?
                }
            };
            Ok(ctx.int(i64::from(code)))
        }
        "iter" | "next" => {
            if !kwargs.is_empty() {
                return Err(type_error(format!("{name}() takes no keyword arguments")));
            }
            match args {
                [] => Err(type_error(format!(
                    "{name} expected at least 1 argument, got 0"
                ))),
                [iterable] if name == "iter" => ctx.iterator(*iterable),
                [iterator] => ctx
                    .next_result(*iterator)?
                    .ok_or_else(|| Error::new("StopIteration", "")),
                [callable, sentinel] if name == "iter" => {
                    if !ctx.callable(*callable) {
                        return Err(type_error("iter(v, w): v must be callable"));
                    }
                    Ok(ctx.alloc(Value::CallableIterator(Some([*callable, *sentinel]))))
                }
                [iterator, default] => Ok(ctx.next_item(*iterator)?.unwrap_or(*default)),
                _ => Err(type_error(format!(
                    "{name} expected at most 2 arguments, got {}",
                    args.len()
                ))),
            }
        }
        "sorted" => collections::sorted(ctx, args, kwargs),
        "enumerate" => {
            let [iterable, start] = bind(name, args, kwargs, ["iterable", "start"])?;
            let iterable = iterable
                .ok_or_else(|| type_error("enumerate() missing required argument 'iterable'"))?;
            let next_index = match start {
                Some(value) => ctx.index_integer(value)?,
                None => crate::Int::from(0),
            };
            let input = ctx.iterator(iterable)?;
            Ok(ctx.alloc(Value::Enumerate { input, next_index }))
        }
        "reversed" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            let object = args[0];
            if let Value::List(items) | Value::Tuple(items) = ctx.heap.get(object) {
                return Ok(ctx.alloc(Value::Iterator {
                    sequence: Some(object),
                    offset: items.len(),
                    reverse: true,
                    kind: "list_reverseiterator",
                }));
            }
            if let Some(hook) = ctx.bound_slot(object, "__reversed__")?
                && hook != NONE
            {
                return ctx.call(hook, Vec::new(), Vec::new());
            }
            Err(type_error(format!(
                "'{}' object is not reversible",
                ctx.type_name(object)
            )))
        }
        "sum" => {
            let Some((&source, rest)) = args.split_first() else {
                return Err(type_error("sum requires a positional iterable"));
            };
            let [start] = bind(name, rest, kwargs, ["start"])?;
            let iterator = ctx.iterator(source)?;
            let mut total = start
                .map(|id| ctx.as_int(id))
                .transpose()?
                .unwrap_or_else(|| crate::Int::from(0));
            let mut item = ctx.next_item(iterator)?;
            if item.is_none() {
                return Ok(start.unwrap_or_else(|| ctx.int(0)));
            }
            while let Some(value) = item {
                total = crate::integer_binary(&total, &ctx.as_int(value)?, 0)?;
                item = ctx.next_item(iterator)?;
            }
            Ok(ctx.alloc(Value::Int(total)))
        }
        "len" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            let no_length = |ctx: &Context| {
                type_error(format!(
                    "object of type '{}' has no len()",
                    ctx.type_name(args[0])
                ))
            };
            let size = match ctx.heap.get(args[0]) {
                Value::List(v) | Value::Tuple(v) | Value::Set { items: v, .. } => v.len(),
                Value::NamedTuple { values, .. } => values.len(),
                Value::Context(bindings) => bindings.len(),
                Value::Dict(v) | Value::DefaultDict { entries: v, .. } => v.len(),
                Value::DictView { dict, .. } | Value::MappingProxy(dict) => {
                    collections::entries(ctx, *dict)?.len()
                }
                Value::Class { .. } => {
                    let values = ctx.enum_values(args[0]).ok_or_else(|| no_length(ctx))?;
                    collections::entries(ctx, values)?.len()
                }
                Value::Str(v) => v.chars().count(),
                Value::Bytes(v) | Value::ByteArray(v) => v.len(),
                Value::Range { start, stop, step } => range_len(*start, *stop, *step)?,
                Value::Instance { class, .. } if ctx.namedtuple_fields(*class).is_some() => {
                    ctx.items(args[0])?.len()
                }
                Value::Instance { .. } => {
                    ctx.instance_len(args[0])?.ok_or_else(|| no_length(ctx))?
                }
                _ => return Err(no_length(ctx)),
            };
            let size = i64::try_from(size).map_err(|_| {
                Error::new("OverflowError", "length does not fit in a Python index")
            })?;
            Ok(ctx.int(size))
        }
        "map" | "zip" => ctx.zip_or_map(name, args, kwargs),
        "slice" => super::slice::construct(ctx, args, kwargs),
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
        "dict" | "list" | "tuple" | "set" | "frozenset" => {
            collections::construct(ctx, name, args, kwargs)
        }
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
        "classmethod" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            Ok(ctx.alloc(Value::ClassMethod(args[0])))
        }
        "staticmethod" => {
            if !kwargs.is_empty() {
                return Err(type_error("staticmethod() takes no keyword arguments"));
            }
            let [callable] = args else {
                return Err(type_error(format!(
                    "staticmethod expected 1 argument, got {}",
                    args.len()
                )));
            };
            super::static_method::construct(ctx, *callable)
        }
        "property" => {
            let fields = bind(name, args, kwargs, ["fget", "fset", "fdel", "doc"])?;
            super::property::construct(ctx, fields)
        }
        "open" => {
            arity(name, args, 0, 2)?;
            let [path, mode, encoding] = bind(name, args, kwargs, ["file", "mode", "encoding"])?;
            let path = ctx.str_value(path.ok_or_else(|| type_error("open requires a file"))?)?;
            let mode = mode.map(|id| ctx.str_value(id)).transpose()?.unwrap_or("r");
            if let Some(encoding) = encoding.filter(|&value| value != NONE) {
                if mode.contains('b') {
                    return Err(Error::new(
                        "ValueError",
                        "binary mode doesn't take an encoding argument",
                    ));
                }
                let encoding = ctx.str_value(encoding)?;
                if !matches!(
                    crate::objects::Codec::lookup(encoding)?,
                    crate::objects::Codec::Utf8 { signature: false }
                ) {
                    return Err(Error::new(
                        "ValueError",
                        "text files require UTF-8 encoding",
                    ));
                }
            }
            let text_mode = match mode {
                "r" | "rt" | "tr" => Some(0),
                "w" | "wt" | "tw" => Some(1),
                _ => None,
            };
            if let Some(text_mode) = text_mode {
                let file = crate::objects::TextFile::open(path, text_mode)?;
                return Ok(ctx.alloc(Value::TextFile(file)));
            }
            let file = super::stdlib::BinaryFile::open(path, mode)?;
            Ok(ctx.alloc(file.into_value()?))
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
        "complex" => ctx.new_complex(args, kwargs),
        "float" => float::construct(ctx, args, kwargs),
        "all" | "any" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            let iter = ctx.iterator(args[0])?;
            let stop_value = name == "any";
            while let Some(item) = ctx.next_item(iter)? {
                if ctx.truth(item)? == stop_value {
                    return Ok(boolean(stop_value));
                }
            }
            Ok(boolean(!stop_value))
        }
        "print" => print::call(ctx, args, kwargs),
        name if is_exception(name) => {
            positional(name, kwargs)?;
            let error = ctx.exception(name, args);
            Ok(ctx.alloc(Value::Raised(error)))
        }
        name => Err(type_error(format!("unsupported builtin {name}"))),
    }
}
