//! Built-ins used by native Python code whose values are known at runtime.

use super::Context;
use super::value::{ELLIPSIS, Error, FALSE, Id, NONE, NOT_IMPLEMENTED, Result, TRUE, Value};

mod collections;
mod dir;
mod exceptions;
mod float;
mod round;
pub(super) use float::from_numeric;
mod integer;
mod print;
mod text;
mod translation;
pub(super) use collections::{
    clear_container, copy_dict, hashable, remove_dict_item, remove_dict_item_hashed, set_add,
    set_member, update as update_dict, update_set,
};
pub(super) use text::{
    bytes_from_iterable, bytes_percent, format_conversion, format_value, percent, zeroed_bytes,
};

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
    SyntaxError,
    IndentationError,
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
    InterruptedError,
    ChildProcessError,
    ProcessLookupError,
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
    ReferenceError,
    SystemExit,
    StopIteration,
    Warning,
    RuntimeWarning,
);

const FUNCTIONS: &[&str] = &[
    "len",
    "hash",
    "reversed",
    "dir",
    "sorted",
    "min",
    "max",
    "ord",
    "chr",
    "divmod",
    "round",
    "callable",
    "id",
    "abs",
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
    "input",
    "print",
    "pow",
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
    "filter",
    "float",
    "frozenset",
    "int",
    "list",
    "map",
    "memoryview",
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

pub(super) const IMPORT_KEYWORDS: [&str; 3] = ["name", "path", "name_from"];

/// Return the initializer inherited from the built-in exception class.
pub(super) fn exception_initializer(kind: &str) -> &'static str {
    match kind {
        "StopIteration" => "StopIteration.__init__",
        "ImportError" | "ModuleNotFoundError" => "ImportError.__init__",
        kind if exception_subclass(kind, "OSError") == Some(true) => "OSError.__init__",
        _ => "BaseException.__init__",
    }
}

/// Return the exception that declares the inherited CPython allocation slot.
pub(super) fn exception_new_owner(name: &str) -> &str {
    match name {
        "KeyError" => "LookupError",
        "SystemExit" => "BaseException",
        "NameError"
        | "UnboundLocalError"
        | "AttributeError"
        | "ImportError"
        | "ModuleNotFoundError"
        | "StopIteration"
        | "binascii.Incomplete" => "Exception",
        "binascii.Error" => "ValueError",
        name if exception_subclass(name, "OSError") == Some(true) => "OSError",
        _ => name,
    }
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
pub(super) fn constructor_owner(name: &str) -> Option<&str> {
    let owner = name.strip_suffix(".__new__")?;
    (matches!(owner, "float" | "int" | "str") || is_exception(owner)).then_some(owner)
}

pub(super) fn is_builtin_type(name: &str) -> bool {
    TYPES.contains(&name) || is_exception(name) || is_enum_base(name)
}

/// Native iterator classes implement both iterator slots in the runtime.
pub(super) fn is_iterator_type(name: &str) -> bool {
    matches!(
        name,
        "iterator"
            | "generator"
            | "list_iterator"
            | "list_reverseiterator"
            | "tuple_iterator"
            | "bytes_iterator"
            | "bytearray_iterator"
            | "range_iterator"
            | "longrange_iterator"
            | "dict_keyiterator"
            | "dict_valueiterator"
            | "dict_itemiterator"
            | "callable_iterator"
            | "fieldnameiterator"
            | "formatteriterator"
            | "keys"
            | "values"
            | "items"
            | "reversed"
            | "enumerate"
            | "map"
            | "filter"
            | "zip"
            | "itertools.count"
            | "itertools.repeat"
            | "_io.BufferedReader"
            | "_io.BufferedWriter"
            | super::deque::ITERATOR
            | super::deque::REVERSE_ITERATOR
    )
}

/// Recognize the native classes that provide Enum's member methods.
pub(super) fn is_enum_base(name: &str) -> bool {
    matches!(
        name,
        "enum.Enum" | "enum.IntEnum" | "enum.IntFlag" | "enum.StrEnum"
    )
}

pub(super) const DICT_METHODS: &[&str] = &[
    "__repr__",
    "__init__",
    "__len__",
    "__iter__",
    "__contains__",
    "__getitem__",
    "__setitem__",
    "__delitem__",
    "clear",
    "copy",
    "fromkeys",
    "get",
    "items",
    "keys",
    "pop",
    "setdefault",
    "update",
    "values",
];

pub(super) const LIST_METHODS: &[&str] = &[
    "__add__", "__iadd__", "__init__", "__repr__", "append", "clear", "copy", "count", "extend",
    "index", "insert", "pop", "remove", "reverse", "sort",
];

pub(super) const BYTES_METHODS: &[&str] = &[
    "__mod__",
    "__repr__",
    "decode",
    "rfind",
    "rsplit",
    "rstrip",
    "translate",
];

pub(super) const STR_METHODS: &[&str] = &[
    "__repr__",
    "capitalize",
    "encode",
    "endswith",
    "expandtabs",
    "find",
    "format",
    "isalnum",
    "isspace",
    "isupper",
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
        Value::ThreadLocal(_) if super::thread_local::is_method(attr) => super::thread_local::CLASS,
        Value::PrimitiveLock(_) if super::thread_lock::is_method(attr) => super::thread_lock::CLASS,
        Value::Int(_) | Value::Bool(_) if matches!(attr, "bit_length" | "to_bytes") => "int",
        Value::Str(_) if STR_METHODS.contains(&attr) => "str",
        Value::Range(_) if super::range::METHODS.contains(&attr) => "range",
        Value::RangeIterator(iter)
            if matches!(attr, "__iter__" | "__next__" | "__length_hint__") =>
        {
            iter.kind
        }
        Value::Slice(_) if super::slice::METHODS.contains(&attr) => "slice",
        Value::TupleGetterDoc if super::namedtuple::GETTER_METHODS.contains(&attr) => {
            "member_descriptor"
        }
        Value::TupleGetter { .. } if super::namedtuple::GETTER_METHODS.contains(&attr) => {
            super::namedtuple::GETTER
        }
        Value::ItemGetter(_) if attr == "__reduce__" => "operator.itemgetter",
        Value::Property(_) if super::property::METHODS.contains(&attr) => "property",
        Value::StaticMethod { .. } if attr == "__get__" => "staticmethod",
        Value::Bytes(_) if BYTES_METHODS.contains(&attr) => "bytes",
        Value::ByteArray(_) if matches!(attr, "__repr__" | "decode" | "extend" | "translate") => {
            "bytearray"
        }
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
        Value::BufferedRandom(_) if super::stdlib::tempfile::METHODS.contains(&attr) => {
            "BufferedRandom"
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
        Value::List(_) if LIST_METHODS.contains(&attr) => "list",
        Value::Tuple(_) if matches!(attr, "__getitem__" | "__repr__") => "tuple",
        Value::Set { frozen: false, .. }
            if matches!(
                attr,
                "add"
                    | "copy"
                    | "discard"
                    | "update"
                    | "issuperset"
                    | "__contains__"
                    | "__reduce__"
                    | "__repr__"
            ) =>
        {
            "set"
        }
        Value::Set { frozen: true, .. }
            if matches!(
                attr,
                "copy" | "issuperset" | "__contains__" | "__reduce__" | "__repr__"
            ) =>
        {
            "frozenset"
        }
        Value::DefaultDict { .. }
            if matches!(
                attr,
                "copy" | "__copy__" | "__missing__" | "__repr__" | "__reduce__"
            ) =>
        {
            "collections.defaultdict"
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
                "readable"
                    | "writable"
                    | "seekable"
                    | "isatty"
                    | "seek"
                    | "tell"
                    | "read"
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
        Value::RegexPattern(_)
            if matches!(attr, "findall" | "match" | "fullmatch" | "search" | "split") =>
        {
            "pattern"
        }
        Value::RegexMatch { .. } if matches!(attr, "group" | "groups") => "match",
        Value::Map { .. } if matches!(attr, "__iter__" | "__next__") => "map",
        Value::Filter { .. } if matches!(attr, "__iter__" | "__next__") => "filter",
        Value::Enumerate { .. } if matches!(attr, "__iter__" | "__next__") => "enumerate",
        Value::Count { .. } if matches!(attr, "__iter__" | "__next__") => "itertools.count",
        Value::Deque(_) if super::deque::METHODS.contains(&attr) => super::deque::CLASS,
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

/// Read the sole positional argument of a CPython `METH_O` builtin.
fn one_positional_arg(name: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error(format!("{name}() takes no keyword arguments")));
    }
    let [value] = args else {
        return Err(type_error(format!(
            "{name}() takes exactly one argument ({} given)",
            args.len()
        )));
    };
    Ok(*value)
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
    let arguments = |n| if n == 1 { "argument" } else { "arguments" };
    let count = args.len() + kwargs.len();
    if count > N {
        let kind = if args.is_empty() { "keyword " } else { "" };
        return Err(type_error(format!(
            "{name}() takes at most {N} {kind}{} ({count} given)",
            arguments(N)
        )));
    }
    if args.len() > max_positional {
        if max_positional == 0 {
            return Err(type_error(format!(
                "{name}() takes no positional arguments"
            )));
        }
        return Err(type_error(format!(
            "{name}() takes at most {max_positional} positional {} ({} given)",
            arguments(max_positional),
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

/// Bind built-in arguments in the order Argument Clinic reports missing and invalid names.
pub(super) fn bind_builtin_arguments<const N: usize>(
    builtin: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    names: [&str; N],
    required: usize,
) -> Result<[Option<Id>; N]> {
    if args.len() + kwargs.len() <= names.len() {
        // Argument Clinic diagnoses missing required operands before duplicate or unknown names.
        for (index, &name) in names[..required].iter().enumerate().skip(args.len()) {
            if !kwargs.iter().any(|(key, _)| key == name) {
                return Err(type_error(format!(
                    "{builtin}() missing required argument '{name}' (pos {})",
                    index + 1
                )));
            }
        }
        if let Some((key, _)) = kwargs
            .iter()
            .find(|(key, _)| !names.contains(&key.as_str()))
            && let Some(hint) = crate::objects::exceptions::keyword_suggestion(key, &names)
        {
            return Err(type_error(format!(
                "{builtin}() got an unexpected keyword argument '{key}'. Did you mean '{hint}'?"
            )));
        }
    }
    bind_method_arguments(builtin, args, kwargs, names, N)
}

/// Bind the positional or named operands of `pow` before selecting its numeric operation.
fn power(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(base), Some(exp), modulus] =
        bind_builtin_arguments("pow", args, kwargs, ["base", "exp", "mod"], 2)?
    else {
        unreachable!("required pow operands were checked above")
    };
    let Some(modulus) = modulus.filter(|&id| id != NONE) else {
        return ctx.binary("**", base, exp);
    };
    let (Ok(x), Ok(y), Ok(m)) = (ctx.as_int(base), ctx.as_int(exp), ctx.as_int(modulus)) else {
        if [base, exp, modulus]
            .into_iter()
            .any(|id| matches!(ctx.heap.get(ctx.scalar_value(id)), Value::Float(_)))
        {
            return Err(type_error(
                "pow() 3rd argument not allowed unless all arguments are integers",
            ));
        }
        return Err(type_error(format!(
            "unsupported operand type(s) for ** or pow(): '{}', '{}', '{}'",
            ctx.type_name(base),
            ctx.type_name(exp),
            ctx.type_name(modulus)
        )));
    };
    Ok(ctx.alloc(Value::Int(crate::integer_power::modular_power(&x, &y, &m)?)))
}

pub(super) fn boolean(value: bool) -> Id {
    if value { TRUE } else { FALSE }
}

pub(super) fn attribute_name(ctx: &Context, value: Id) -> Result<String> {
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
        ctx.set_named_attr(*obj, &attr_name, Some(*attr), *value)?;
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
    let name = attribute_name(ctx, *attr)?;
    if base {
        ctx.default_delete_attr(*obj, &name)?;
    } else {
        ctx.delete_named_attr(*obj, &name, Some(*attr))?;
    }
    Ok(NONE)
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
    if let Some(owner) = name.strip_suffix(".__new__")
        && is_exception(owner)
    {
        return exceptions::allocate(ctx, owner, args);
    }
    if let Some(owner @ ("BaseException" | "StopIteration" | "OSError" | "ImportError")) =
        name.strip_suffix(".__init__")
    {
        return exceptions::initialize(ctx, owner, args, kwargs);
    }
    if name == "object.__hash__" {
        let Some((&receiver, rest)) = args.split_first() else {
            return Err(type_error(
                "descriptor '__hash__' of 'object' object needs an argument",
            ));
        };
        if !kwargs.is_empty() {
            return Err(type_error("wrapper __hash__() takes no keyword arguments"));
        }
        if !rest.is_empty() {
            return Err(type_error(format!(
                "expected 0 arguments, got {}",
                rest.len()
            )));
        }
        return Ok(ctx.int(Context::identity_hash(receiver)));
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
            "module" => ctx.module_method(method, args, kwargs),
            "int" if method == "bit_length" => integer::bit_length(ctx, args, kwargs),
            "int" if method == "to_bytes" => integer::to_bytes(ctx, args, kwargs),
            "bytes" | "bytearray" if method == "maketrans" => {
                translation::maketrans(ctx, name, args, kwargs)
            }
            "bytes" | "bytearray" if method == "translate" => {
                translation::translate(ctx, owner, args, kwargs)
            }
            "str" | "bytes" | "bytearray" if method == "__repr__" => {
                collections::builtin_slot(ctx, name, args, kwargs)
            }
            "bytearray" if method == "decode" => text::method(ctx, owner, method, args, kwargs),
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
            "tuple" if method == "__getitem__" => {
                let (&receiver, args) = args.split_first().ok_or_else(|| {
                    type_error("descriptor '__getitem__' of 'tuple' object needs an argument")
                })?;
                if !ctx.builtin_slot_accepts(receiver, "tuple") {
                    return Err(type_error(format!(
                        "descriptor '__getitem__' requires a 'tuple' object but received a '{}'",
                        ctx.type_name(receiver)
                    )));
                }
                if !kwargs.is_empty() {
                    return Err(type_error(
                        "wrapper __getitem__() takes no keyword arguments",
                    ));
                }
                let [key] = args else {
                    return Err(type_error(format!(
                        "expected 1 argument, got {}",
                        args.len()
                    )));
                };
                ctx.sequence_subscript(receiver, *key)
            }
            "list" | "tuple" | "dict" | "set" | "frozenset" => {
                collections::method(ctx, name, args, kwargs)
            }
            "slice" => super::slice::method(ctx, method, args, kwargs),
            "range" | "range_iterator" | "longrange_iterator" => {
                super::range::method(ctx, owner, method, args, kwargs)
            }
            "map" | "filter" | "enumerate" => ctx.iterator_method(owner, method, args, kwargs),
            "property" => super::property::method(ctx, method, args, kwargs),
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
        "module" => {
            let class = ctx.builtin("module");
            let instance = ctx.allocate_module(class);
            ctx.init_module(instance, args, kwargs)?;
            Ok(instance)
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
            ctx.exec_source(args[0], args[1], args.get(2).copied(), None)
        }
        "abs" => {
            if !kwargs.is_empty() {
                return Err(type_error("abs() takes no keyword arguments"));
            }
            let &[value] = args else {
                return Err(type_error(format!(
                    "abs() takes exactly one argument ({} given)",
                    args.len()
                )));
            };
            if let Some(method) = ctx.bound_slot(value, "__abs__")? {
                return ctx.call(method, vec![], vec![]);
            }
            let scalar = ctx.scalar_value(value);
            match ctx.heap.get(scalar) {
                Value::Float(number) => Ok(ctx.alloc(Value::Float(number.abs()))),
                Value::Complex(number) => {
                    let magnitude = number.real.hypot(number.imag);
                    if magnitude.is_infinite() && number.real.is_finite() && number.imag.is_finite()
                    {
                        return Err(Error::new("OverflowError", "absolute value too large"));
                    }
                    Ok(ctx.alloc(Value::Float(magnitude)))
                }
                Value::Bool(value) => Ok(ctx.int(i64::from(*value))),
                Value::Int(number) => {
                    let zero = crate::Int::from(0);
                    if number.cmp(&zero).is_lt() {
                        let magnitude = crate::integer_binary(&zero, number, 1)?;
                        Ok(ctx.alloc(Value::Int(magnitude)))
                    } else {
                        Ok(scalar)
                    }
                }
                _ => Err(type_error(format!(
                    "bad operand type for abs(): '{}'",
                    ctx.type_name(value)
                ))),
            }
        }
        "round" => round::call(ctx, args, kwargs),
        "id" => {
            let object = one_positional_arg(name, args, kwargs)?;
            Ok(ctx.alloc(Value::Int(crate::Int::from_usize(object as usize))))
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
            match ctx.lookup_named_attr(args[0], &attr, Some(args[1])) {
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
        "hash" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            let hash = ctx.dictionary_hash(args[0])?;
            Ok(ctx.int(hash))
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
        "pow" => power(ctx, args, kwargs),
        "dir" => dir::call(ctx, args, kwargs),
        "sorted" => collections::sorted(ctx, args, kwargs),
        "min" | "max" => collections::extremum(ctx, name, args, kwargs),
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
            if let Value::Range(range) = ctx.heap.get(object) {
                return Ok(ctx.alloc(Value::RangeIterator(range.iter(true))));
            }
            if let Value::Deque(ring) = ctx.heap.get(object) {
                let iter = super::deque::iterator(object, ring, true);
                return Ok(ctx.alloc(iter));
            }
            let start = match ctx.heap.get(object) {
                Value::List(items) => Some((items.len(), "list_reverseiterator")),
                Value::Tuple(items) => Some((items.len(), "reversed")),
                Value::Str(text) => Some((text.len(), "reversed")),
                Value::Bytes(bytes) => Some((bytes.len(), "reversed")),
                _ => None,
            };
            if let Some((offset, kind)) = start {
                return Ok(ctx.alloc(Value::Iterator {
                    sequence: Some(object),
                    offset,
                    reverse: true,
                    kind,
                    context: None,
                }));
            }
            if let Some(hook) = ctx.bound_slot(object, "__reversed__")? {
                if hook != NONE {
                    return ctx.call(hook, Vec::new(), Vec::new());
                }
            } else if let Value::DictView {
                dict,
                kind,
                ordered,
            } = *ctx.heap.get(object)
            {
                return Ok(ctx.dict_iterator(dict, kind, ordered, true));
            } else if ctx.dict_storage(object).is_some() {
                return Ok(ctx.dict_iterator(
                    object,
                    super::value::DictViewKind::Keys,
                    false,
                    true,
                ));
            } else if matches!(ctx.heap.get(object), Value::Instance { .. })
                && (ctx.slot(object, "__getitem__").is_some()
                    || matches!(
                        ctx.heap.get(ctx.scalar_value(object)),
                        Value::Str(_) | Value::Bytes(_) | Value::List(_) | Value::Tuple(_)
                    ))
            {
                let remaining = ctx.reverse_sequence_len(object)?;
                return Ok(ctx.alloc(Value::ReverseSequence {
                    source: Some(object),
                    remaining,
                }));
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
            float::sum(ctx, source, start)
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
                Value::List(v) | Value::Tuple(v) => v.len(),
                Value::Set { entries, .. } => entries.len(),
                Value::NamedTuple { values, .. } => values.len(),
                Value::Context(bindings)
                | Value::Iterator {
                    context: Some(bindings),
                    ..
                } => bindings.len(),
                Value::Dict(v) | Value::DefaultDict { entries: v, .. } => v.len(),
                Value::MappingView { mapping, .. } => {
                    let mapping = *mapping;
                    return call(ctx, "len", &[mapping], &[]);
                }
                Value::DictView { dict, .. } | Value::MappingProxy(dict) => {
                    collections::entries(ctx, *dict)?.len()
                }
                Value::Class { .. } => {
                    let values = ctx.enum_values(args[0]).ok_or_else(|| no_length(ctx))?;
                    collections::entries(ctx, values)?.len()
                }
                Value::Deque(ring) => ring.items.len(),
                Value::Str(v) => v.chars().count(),
                Value::Bytes(v) | Value::ByteArray(v) => v.len(),
                Value::Range(range) => {
                    return range.len.to_i64().map(|n| ctx.int(n)).ok_or_else(|| {
                        Error::new(
                            "OverflowError",
                            "Python int too large to convert to C ssize_t",
                        )
                    });
                }
                Value::Instance { class, .. } if ctx.namedtuple_fields(*class).is_some() => {
                    ctx.items(args[0])?.len()
                }
                Value::Instance { .. } | Value::Raised(_) => {
                    if let Some(size) = ctx.instance_len(args[0])? {
                        size
                    } else {
                        let scalar = ctx.scalar_value(args[0]);
                        if matches!(ctx.heap.get(scalar), Value::Str(_) | Value::Bytes(_)) {
                            return call(ctx, "len", &[scalar], &[]);
                        }
                        return Err(no_length(ctx));
                    }
                }
                _ => return Err(no_length(ctx)),
            };
            let size = i64::try_from(size).map_err(|_| {
                Error::new("OverflowError", "length does not fit in a Python index")
            })?;
            Ok(ctx.int(size))
        }
        "map" | "zip" => ctx.zip_or_map(name, args, kwargs),
        "filter" => {
            if !kwargs.is_empty() {
                return Err(type_error("filter() takes no keyword arguments"));
            }
            let [predicate, source] = args else {
                return Err(type_error(format!(
                    "filter expected 2 arguments, got {}",
                    args.len()
                )));
            };
            let input = ctx.iterator(*source)?;
            Ok(ctx.alloc(Value::Filter {
                predicate: *predicate,
                input,
            }))
        }
        "slice" => super::slice::construct(ctx, args, kwargs),
        "range" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 3)?;
            let first = ctx.index_integer(args[0])?;
            let (start, stop) = if let Some(&stop) = args.get(1) {
                (first, ctx.index_integer(stop)?)
            } else {
                (crate::Int::from(0), first)
            };
            let step = args
                .get(2)
                .map(|&x| ctx.index_integer(x))
                .transpose()?
                .unwrap_or_else(|| crate::Int::from(1));
            Ok(ctx.alloc(Value::Range(super::range::Range::new(start, stop, step)?)))
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
        "property" => super::property::construct(ctx, args, kwargs),
        "open" => {
            arity(name, args, 0, 2)?;
            let [path, mode, encoding] = bind(name, args, kwargs, ["file", "mode", "encoding"])?;
            let filename = path.ok_or_else(|| type_error("open requires a file"))?;
            let path = ctx.str_value(filename)?;
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
            let writing = mode.contains(['w', 'a', 'x']);
            let file = match text_mode {
                Some(mode) => crate::objects::TextFile::open(path, mode)
                    .map(Value::TextFile)
                    .map_err(Error::from),
                None => super::stdlib::BinaryFile::open(path, mode)
                    .and_then(super::stdlib::BinaryFile::into_value),
            };
            match file {
                Ok(file) if writing => ctx.heap.alloc_file_writer(file),
                Ok(file) => Ok(ctx.alloc(file)),
                Err(error) => Err(ctx.with_open_filename(error, filename)?),
            }
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
            let iterable = one_positional_arg(name, args, kwargs)?;
            let iter = ctx.iterator(iterable)?;
            let stop_value = name == "any";
            while let Some(item) = ctx.next_item(iter)? {
                if ctx.truth(item)? == stop_value {
                    return Ok(boolean(stop_value));
                }
            }
            Ok(boolean(!stop_value))
        }
        "input" => {
            positional(name, kwargs)?;
            arity(name, args, 0, 0)?;
            Ok(ctx.text(crate::sys::read_input_line()?))
        }
        "print" => print::call(ctx, args, kwargs),
        "NoneType" | "NotImplementedType" | "ellipsis" => {
            if !args.is_empty() || !kwargs.is_empty() {
                let ty = if name == "ellipsis" {
                    "EllipsisType"
                } else {
                    name
                };
                return Err(type_error(format!("{ty} takes no arguments")));
            }
            Ok(match name {
                "NoneType" => NONE,
                "NotImplementedType" => NOT_IMPLEMENTED,
                _ => ELLIPSIS,
            })
        }
        "function" => ctx.construct_function(args, kwargs),
        "ImportError" | "ModuleNotFoundError" => {
            let mut error = ctx.exception(name, args);
            exceptions::initialize_import(&mut error, args, kwargs)?;
            Ok(ctx.alloc(Value::Raised(error)))
        }
        name @ ("SyntaxError" | "IndentationError") => {
            positional(name, kwargs)?;
            let mut fields = ctx.syntax_fields(args);
            if let [_, info] = args {
                let members = ctx.items(*info)?;
                match members.len() {
                    n @ 0..=3 => {
                        return Err(type_error(format!(
                            "function takes at least 4 arguments ({n} given)"
                        )));
                    }
                    5 => {
                        return Err(type_error(
                            "end_offset must be provided when end_lineno is provided",
                        ));
                    }
                    n if n > 7 => {
                        return Err(type_error(format!(
                            "function takes at most 7 arguments ({n} given)"
                        )));
                    }
                    _ => (),
                }
                if let Some(metadata) = members.get(6) {
                    fields[8] = *metadata;
                }
                for (field, member) in fields[1..7].iter_mut().zip(members) {
                    *field = member;
                }
            }
            let mut error = ctx.exception(name, args);
            ctx.retain_syntax_fields(&mut error, fields)?;
            Ok(ctx.alloc(Value::Raised(error)))
        }
        name if is_exception(name) => {
            positional(name, kwargs)?;
            let error = if exception_subclass(name, "OSError") == Some(true) {
                ctx.os_exception(name, None, args, None)?
            } else {
                ctx.exception(name, args)
            };
            Ok(ctx.alloc(Value::Raised(error)))
        }
        name => Err(type_error(format!("unsupported builtin {name}"))),
    }
}
