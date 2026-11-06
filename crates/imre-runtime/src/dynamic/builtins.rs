//! Built-ins used by native Python code whose values are known at runtime.

use imre_ast::providers::BUILTIN_FUNCTIONS;

use super::Context;
use super::value::{
    self, DictIterator, ELLIPSIS, Error, FALSE, Id, NONE, NOT_IMPLEMENTED, Result, TRUE, Value,
};

mod collections;
pub(super) mod dir;
mod exceptions;
mod float;
mod round;
pub(super) use float::{from_numeric, from_numeric_slots, round_float_to_integer};
mod integer;
mod power;
mod print;
mod text;
mod translation;
pub(super) use collections::{
    clear_container, copy_dict, hashable, list_extend, remove_dict_item, remove_dict_item_hashed,
    remove_table_entry, set_add, set_member, sort_values, update as update_dict, update_set,
};
pub(super) use text::{
    bytes_from_iterable, bytes_percent, format_conversion, format_value, percent, zeroed_bytes,
};

/// Drop the temporary bound method before a caller uses its result.
fn call_stream_method(ctx: &mut Context, stream: Id, name: &str) -> Result<Id> {
    let mark = ctx.heap.temporary_mark();
    let result = ctx
        .lookup_attr(stream, name)
        .and_then(|method| ctx.call(method, Vec::new(), Vec::new()));
    ctx.release_with_result(mark, result)
}

/// The contiguous stream roots are ordered so stdin is released first.
struct BoxedInput<'a>(&'a mut Context, std::ops::Range<usize>);

impl BoxedInput<'_> {
    /// input ignores a failing fileno call, but propagates errors converting its result.
    fn descriptor(&mut self, stream: Id) -> Result<Option<i64>> {
        let mark = self.0.heap.temporary_mark();
        let Ok(fd) = call_stream_method(self.0, stream, "fileno") else {
            self.0.discard_temporaries(mark, None);
            return Ok(None);
        };
        let converted = self.0.index_c_long(fd);
        let number = converted.as_ref().ok().copied();
        // Keep a conversion exception alive while releasing the Python descriptor.
        self.0.release_with_result(mark, converted.map(|_| NONE))?;
        Ok(number)
    }
}

impl crate::input::Backend for BoxedInput<'_> {
    type Stream = Id;
    type Prompt = Id;
    type Line = Id;
    type Error = Error;

    fn stream(&mut self, name: crate::input::StreamName) -> Result<Id> {
        let Self(ctx, stream_roots) = self;
        let name = name.as_str();
        let stream = ctx
            .sys_binding(name)?
            .filter(|&stream| stream != NONE)
            .ok_or_else(|| Error::new("RuntimeError", format!("lost sys.{name}")))?;
        ctx.heap.keep_at(stream_roots.start, stream);
        stream_roots.end += 1;
        Ok(stream)
    }

    fn audit(&mut self, prompt: Option<&Id>) -> Result<()> {
        super::stdlib::audit::event(self.0, "builtins.input", &[prompt.copied().unwrap_or(NONE)])
    }

    fn check_descriptors(&mut self, stdin: &Id, stdout: &Id) -> Result<()> {
        // CPython inspects stdout.fileno only after stdin identifies the process terminal.
        if self.descriptor(*stdin)? == Some(0) && crate::objects::os::isatty(0) {
            self.descriptor(*stdout)?;
        }
        Ok(())
    }

    fn flush_ignoring_errors(&mut self, stream: &Id) {
        let mark = self.0.heap.temporary_mark();
        let _ = call_stream_method(self.0, *stream, "flush");
        self.0.discard_temporaries(mark, None);
    }

    fn write_prompt(&mut self, stream: &Id, prompt: Id) -> Result<()> {
        print::write(self.0, *stream, prompt)
    }

    fn read_line(&mut self, stream: &Id) -> Result<Id> {
        call_stream_method(self.0, *stream, "readline")
    }

    fn last_unit(&self, line: &Id) -> Result<Option<u32>> {
        match self.0.heap.get(self.0.scalar_value(*line)) {
            Value::Bytes(bytes) => Ok(bytes.last().copied().map(u32::from)),
            Value::Str(text) => Ok(text.chars().next_back().map(u32::from)),
            Value::SurrogateStr(units) => Ok(units.last().copied()),
            _ => Err(type_error("object.readline() returned non-string")),
        }
    }

    fn trim_linefeed(&mut self, line: Id) -> Result<Id> {
        let stripped = match self.0.heap.get(self.0.scalar_value(line)) {
            Value::Bytes(bytes) => Value::Bytes(bytes[..bytes.len() - 1].to_vec()),
            Value::Str(text) => Value::Str((&text[..text.len() - 1]).into()),
            Value::SurrogateStr(units) => Value::SurrogateStr(units[..units.len() - 1].to_vec()),
            _ => unreachable!("last_unit checked the line type"),
        };
        Ok(self.0.alloc(stripped))
    }

    fn eof(&self) -> Error {
        Error::new("EOFError", "EOF when reading a line")
    }
}

pub(super) fn exception_kind(name: &str) -> Option<crate::objects::exceptions::Kind> {
    use crate::objects::exceptions::Kind;
    match name {
        "UnsupportedOperation" => Some(Kind::UnsupportedOperation),
        "socket.gaierror" => Some(Kind::GaiError),
        "socket.herror" => Some(Kind::HError),
        "_lzma.LZMAError" => Some(Kind::LzmaError),
        "zlib.error" => Some(Kind::ZlibError),
        "gzip.BadGzipFile" => Some(Kind::BadGzipFile),
        "binascii.Error" => Some(Kind::BinasciiError),
        "binascii.Incomplete" => Some(Kind::BinasciiIncomplete),
        _ => imre_ast::exceptions::boxed_builtin_kind(name),
    }
}

/// Methods declared on BaseException and inherited by every native exception type.
pub(super) fn base_exception_method(name: &str) -> Option<&'static str> {
    match name {
        "add_note" => Some("BaseException.add_note"),
        "with_traceback" => Some("BaseException.with_traceback"),
        _ => None,
    }
}

/// Native exception ancestors in MRO order, excluding the class itself and object.
pub(super) fn exception_ancestors(
    kind: imre_ast::exceptions::Kind,
) -> impl Iterator<Item = &'static str> {
    use imre_ast::exceptions::Kind::*;
    // These are the exception types with native boxed children. UnsupportedOperation
    // inherits from both OSError and ValueError, in that order.
    [
        ConnectionError,
        OSError,
        UnicodeError,
        ValueError,
        ArithmeticError,
        LookupError,
        ImportError,
        NameError,
        RuntimeError,
        IndentationError,
        SyntaxError,
        Warning,
        BaseExceptionGroup,
        Exception,
        BaseException,
    ]
    .into_iter()
    .filter(move |&base| base != kind && kind.matches(base))
    .map(|base| base.name())
}

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
    BUILTIN_FUNCTIONS
        .iter()
        .chain(TYPES)
        .chain(imre_ast::exceptions::BOXED_BUILTINS)
        .copied()
}

pub(super) fn is_exception(name: &str) -> bool {
    exception_kind(name).is_some()
}

pub(super) const IMPORT_KEYWORDS: [&str; 3] = ["name", "path", "name_from"];

/// Test the declaring class, independently of the Rust function implementing the slot.
pub(super) fn declares_exception_constructor(kind: imre_ast::exceptions::Kind, slot: &str) -> bool {
    use imre_ast::exceptions::Kind::*;
    match slot {
        "__init__" => matches!(
            kind,
            BaseException
                | BaseExceptionGroup
                | AttributeError
                | ImportError
                | NameError
                | OSError
                | StopIteration
                | SyntaxError
                | SystemExit
                | UnicodeDecodeError
                | UnicodeEncodeError
        ),
        "__new__" => match kind {
            KeyError | ExceptionGroup | SystemExit | NameError | UnboundLocalError
            | AttributeError | ImportError | ModuleNotFoundError | StopIteration | SyntaxError
            | IndentationError | TabError | BinasciiIncomplete | BinasciiError | LzmaError
            | ZlibError => false,
            OSError => true,
            _ => !kind.matches(OSError),
        },
        _ => false,
    }
}

pub(super) fn exception_init_descriptor(symbol: &str) -> Option<&'static str> {
    let kind = exception_kind(symbol.strip_suffix(".__init__")?)?;
    declares_exception_constructor(kind, "__init__").then(|| kind.name())
}

/// Consult the native hierarchy when both names denote known exception types.
pub(super) fn exception_subclass(child: &str, parent: &str) -> Option<bool> {
    Some(crate::objects::exceptions::imre_builtin_exception_subclass(
        exception_kind(child)? as u32,
        exception_kind(parent)? as u32,
    ))
}

pub(super) fn is_builtin(name: &str) -> bool {
    (name != "UnsupportedOperation" && is_builtin_type(name)) || is_builtin_function(name)
}

pub(super) fn is_builtin_function(name: &str) -> bool {
    BUILTIN_FUNCTIONS.contains(&name)
}

/// Return the CPython docstring shared by a built-in type and its instances.
pub(super) fn type_doc(name: &str) -> Option<&'static str> {
    Some(match name {
        "str" => {
            r#"str(object='') -> str
str(bytes_or_buffer[, encoding[, errors]]) -> str

Create a new string object from the given object. If encoding or
errors is specified, then the object must expose a data buffer
that will be decoded using the given encoding and error handler.
Otherwise, returns the result of object.__str__() (if defined)
or repr(object).
encoding defaults to 'utf-8'.
errors defaults to 'strict'."#
        }
        "int" => {
            r#"int([x]) -> integer
int(x, base=10) -> integer

Convert a number or string to an integer, or return 0 if no arguments
are given.  If x is a number, return x.__int__().  For floating-point
numbers, this truncates towards zero.

If x is not a number or if base is given, then x must be a string,
bytes, or bytearray instance representing an integer literal in the
given base.  The literal can be preceded by '+' or '-' and be surrounded
by whitespace.  The base defaults to 10.  Valid bases are 0 and 2-36.
Base 0 means to interpret the base from the string as an integer literal.
>>> int('0b100', base=0)
4"#
        }
        "bool" => {
            r#"Returns True when the argument is true, False otherwise.
The builtins True and False are the only two instances of the class bool.
The class bool is a subclass of the class int, and cannot be subclassed."#
        }
        "tuple" => {
            r#"Built-in immutable sequence.

If no argument is given, the constructor returns an empty tuple.
If iterable is specified the tuple is initialized from iterable's items.

If the argument is a tuple, the return value is the same object."#
        }
        "frozenset" => r#"Build an immutable unordered collection of unique elements."#,
        "_abc_data" => r#"Internal state held by ABC machinery."#,
        _ => return None,
    })
}

/// Documentation for bare native functions, from CPython's Python/bltinmodule.c.
pub(super) fn function_doc(name: &str) -> Option<&'static str> {
    match name {
        "min" => Some(
            "min(iterable, *[, default=obj, key=func]) -> value\n\
             min(arg1, arg2, *args, *[, key=func]) -> value\n\
             \n\
             With a single iterable argument, return its smallest item. The\n\
             default keyword-only argument specifies an object to return if\n\
             the provided iterable is empty.\n\
             With two or more positional arguments, return the smallest argument.",
        ),
        "max" => Some(
            "max(iterable, *[, default=obj, key=func]) -> value\n\
             max(arg1, arg2, *args, *[, key=func]) -> value\n\
             \n\
             With a single iterable argument, return its biggest item. The\n\
             default keyword-only argument specifies an object to return if\n\
             the provided iterable is empty.\n\
             With two or more positional arguments, return the largest argument.",
        ),
        _ => None,
    }
}

/// CPython compares built-in functions by their implementation and bound module.
pub(super) fn native_implementation(symbol: &str) -> &str {
    match symbol {
        "_thread.allocate" => "_thread.allocate_lock",
        _ => symbol,
    }
}

pub(super) const MAPPING_PROXY_METHODS: &[&str] = &[
    "keys",
    "values",
    "items",
    "get",
    "copy",
    "__reversed__",
    "__class_getitem__",
];

pub(super) const MAPPING_PROXY_SLOTS: &[&str] = &[
    "__lt__",
    "__le__",
    "__eq__",
    "__ne__",
    "__gt__",
    "__ge__",
    "__contains__",
    "__getitem__",
    "__hash__",
    "__iter__",
    "__len__",
    "__or__",
    "__ior__",
    "__repr__",
    "__ror__",
    "__str__",
];

/// Static constructors retain their declaring type as `__self__` even on instances.
pub(super) fn constructor_owner(name: &str) -> Option<&str> {
    let owner = name.strip_suffix(".__new__")?;
    (matches!(
        owner,
        "object"
            | "dict"
            | "list"
            | "mappingproxy"
            | "float"
            | "function"
            | "cell"
            | "int"
            | "str"
            | "bytes"
            | "super"
            | "type"
            | super::thread_local::CLASS
            | super::namespace::CLASS
    ) || super::stdlib::itertools::cursor_type(owner).is_some()
        || super::deque::iterator_class(owner).is_some()
        || is_exception(owner)
        || owner == super::stdlib::fixed_structseq::SCHED_PARAM
        || crate::objects::HashState::is_provider_class(owner))
    .then_some(owner)
}

/// The declaring type stored on standalone builtin method handles.
pub(super) fn native_method_owner(symbol: &str) -> Option<&str> {
    match symbol {
        "str.maketrans" => Some("str"),
        "bytes.maketrans" => Some("bytes"),
        "bytearray.maketrans" => Some("bytearray"),
        _ => constructor_owner(symbol),
    }
}

pub(super) fn is_builtin_type(name: &str) -> bool {
    TYPES.contains(&name)
        || super::stdlib::fixed_structseq::fixed_type(name).is_some()
        || name == super::stdlib::flags::TYPE
        || is_exception(name)
        || is_enum_base(name)
}

/// The native classes exported directly by the builtins module.
pub(super) fn is_exported_type(name: &str) -> bool {
    TYPES.contains(&name) || imre_ast::exceptions::boxed_builtin_kind(name).is_some()
}

/// Static builtin types have a GC-capable metaclass, but are never themselves tracked.
pub(super) fn is_static_gc_type(name: &str) -> bool {
    is_exported_type(name)
        || name == super::stdlib::flags::TYPE
        || (is_iterator_type(name) && !name.contains('.'))
        || matches!(
            name,
            "NoneType"
                | "NotImplementedType"
                | "ellipsis"
                | "code"
                | "coroutine"
                | "async_generator"
                | "cell"
                | "function"
                | "module"
                | "method"
                | "builtin_function_or_method"
                | "method-wrapper"
                | "wrapper_descriptor"
                | "method_descriptor"
                | "getset_descriptor"
                | "member_descriptor"
                | "mappingproxy"
                | "traceback"
                | "frame"
                | "types.GenericAlias"
                | "weakref.ReferenceType"
                | "weakref.ProxyType"
                | "weakref.CallableProxyType"
        )
}

pub(super) const STRUCT_METHODS: &[&str] = &[
    "__init__",
    "__repr__",
    "pack",
    "pack_into",
    "unpack",
    "unpack_from",
    "iter_unpack",
];

pub(super) const UNPACK_ITERATOR_METHODS: &[&str] = &["__iter__", "__next__", "__length_hint__"];

/// Native iterator classes implement both iterator slots in the runtime.
pub(super) fn is_iterator_type(name: &str) -> bool {
    is_final_iterator_type(name)
        || matches!(
            name,
            "reversed"
                | "enumerate"
                | "map"
                | "filter"
                | "zip"
                | "itertools.count"
                | "itertools.repeat"
                | "_io.BufferedRandom"
                | "_io.BufferedReader"
                | "_io.BufferedWriter"
        )
}

/// Iterator implementations that CPython does not expose as subclassable types.
pub(super) fn is_final_iterator_type(name: &str) -> bool {
    if DictIterator::class(name).is_some() {
        return true;
    }
    matches!(
        name,
        "iterator"
            | "generator"
            | "coroutine_wrapper"
            | "anext_awaitable"
            | "async_generator_asend"
            | "async_generator_athrow"
            | "list_iterator"
            | "list_reverseiterator"
            | "str_ascii_iterator"
            | "str_iterator"
            | "tuple_iterator"
            | "generic_alias_iterator"
            | "memory_iterator"
            | "bytes_iterator"
            | "bytearray_iterator"
            | "range_iterator"
            | "longrange_iterator"
            | "callable_iterator"
            | "fieldnameiterator"
            | "formatteriterator"
            | "keys"
            | "values"
            | "items"
            | super::deque::ITERATOR
            | super::deque::REVERSE_ITERATOR
            | value::STRUCT_UNPACK_ITERATOR
    )
}

/// Recognize the native classes that provide Enum's member methods.
pub(super) fn is_enum_base(name: &str) -> bool {
    matches!(
        name,
        "enum.Enum" | "enum.Flag" | "enum.IntEnum" | "enum.IntFlag" | "enum.StrEnum"
    )
}

pub(super) const DICT_METHODS: &[&str] = &[
    "__reversed__",
    "__or__",
    "__ror__",
    "__ior__",
    "__eq__",
    "__new__",
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
    "popitem",
    "setdefault",
    "update",
    "values",
];

pub(super) const LIST_METHODS: &[&str] = &[
    "__new__",
    "__add__",
    "__getitem__",
    "__iadd__",
    "__init__",
    "__iter__",
    "__len__",
    "__repr__",
    "append",
    "clear",
    "copy",
    "count",
    "extend",
    "index",
    "insert",
    "pop",
    "remove",
    "reverse",
    "sort",
];

const FROZENSET_OPERATORS: &[&str] = &[
    "__and__", "__rand__", "__or__", "__ror__", "__sub__", "__rsub__", "__xor__", "__rxor__",
];
const SET_OPERATORS: &[&str] = &[
    "__and__", "__rand__", "__or__", "__ror__", "__sub__", "__rsub__", "__xor__", "__rxor__",
    "__iand__", "__ior__", "__isub__", "__ixor__",
];

/// Methods for the two native real number classes.
pub(super) fn real_methods(owner: &str) -> &'static [&'static str] {
    match owner {
        "int" => &[
            "__abs__",
            "__bool__",
            "__eq__",
            "__float__",
            "__ge__",
            "__getnewargs__",
            "__gt__",
            "__hash__",
            "__int__",
            "__le__",
            "__lt__",
            "__ne__",
            "__pow__",
            "__round__",
            "__trunc__",
        ],
        "float" => &[
            "__abs__",
            "__ceil__",
            "__eq__",
            "__floor__",
            "__float__",
            "__ge__",
            "__gt__",
            "__int__",
            "__le__",
            "__lt__",
            "__ne__",
            "__round__",
            "__trunc__",
            "hex",
            "is_integer",
        ],
        _ => &[],
    }
}

/// Set methods shared by registration, attribute lookup, and directory enumeration.
pub(super) fn set_methods(owner: &str) -> &'static [&'static str] {
    match owner {
        "set" => &[
            "__class_getitem__",
            "__contains__",
            "__iter__",
            "__len__",
            "__reduce__",
            "__repr__",
            "add",
            "clear",
            "copy",
            "difference",
            "difference_update",
            "discard",
            "intersection",
            "intersection_update",
            "isdisjoint",
            "issubset",
            "issuperset",
            "pop",
            "remove",
            "symmetric_difference",
            "symmetric_difference_update",
            "union",
            "update",
        ],
        "frozenset" => &[
            "__class_getitem__",
            "__contains__",
            "__iter__",
            "__len__",
            "__reduce__",
            "__repr__",
            "copy",
            "difference",
            "intersection",
            "isdisjoint",
            "issubset",
            "issuperset",
            "symmetric_difference",
            "union",
        ],
        _ => &[],
    }
}

pub(super) fn set_operators(owner: &str) -> &'static [&'static str] {
    match owner {
        "set" => SET_OPERATORS,
        "frozenset" => FROZENSET_OPERATORS,
        _ => &[],
    }
}

pub(super) fn set_operator(method: &str) -> Option<&'static str> {
    Some(match method {
        "__and__" | "__rand__" | "__iand__" => "&",
        "__or__" | "__ror__" | "__ior__" => "|",
        "__sub__" | "__rsub__" | "__isub__" => "-",
        "__xor__" | "__rxor__" | "__ixor__" => "^",
        _ => return None,
    })
}

pub(super) const MEMORYVIEW_METHODS: &[&str] = &[
    "cast",
    "__iter__",
    "__enter__",
    "__exit__",
    "release",
    "tobytes",
    "toreadonly",
    "__getitem__",
    "__setitem__",
    "__delitem__",
];

pub(super) const BYTEARRAY_METHODS: &[&str] = &[
    "__repr__",
    "append",
    "clear",
    "copy",
    "endswith",
    "extend",
    "hex",
    "ljust",
    "lstrip",
    "replace",
    "resize",
    "reverse",
    "rstrip",
    "split",
    "splitlines",
    "startswith",
    "strip",
    "upper",
];

pub(super) const BYTES_METHODS: &[&str] = &[
    "__bytes__",
    "__mod__",
    "__repr__",
    "count",
    "decode",
    "endswith",
    "find",
    "hex",
    "index",
    "join",
    "ljust",
    "lower",
    "lstrip",
    "partition",
    "removeprefix",
    "replace",
    "rfind",
    "rjust",
    "rsplit",
    "rstrip",
    "split",
    "splitlines",
    "startswith",
    "strip",
    "translate",
    "upper",
];

pub(super) use imre_ast::strings::STR_METHODS;

/// Select the builtin formatting implementation inherited by this native value or type.
pub(super) fn format_owner(class: &str) -> &'static str {
    match class {
        "int" | "bool" | "enum.IntEnum" | "enum.IntFlag" => "int",
        "str" | "enum.StrEnum" => "str",
        "float" => "float",
        _ => "object",
    }
}

pub(super) fn method_name(value: &Value, attr: &str) -> Option<String> {
    let owner = match value {
        Value::Scandir(_) if super::stdlib::os::scandir::ITER_METHODS.contains(&attr) => {
            super::stdlib::os::scandir::ITER
        }
        Value::DirEntry(_) if super::stdlib::os::scandir::ENTRY_METHODS.contains(&attr) => {
            super::stdlib::os::scandir::ENTRY
        }
        Value::ThreadLocal(_) if super::thread_local::is_method(attr) => super::thread_local::CLASS,
        Value::HandlerLock(_) if super::thread_rlock::is_method(attr) => super::thread_rlock::CLASS,
        Value::PrimitiveLock(_) if super::thread_lock::is_method(attr) => super::thread_lock::CLASS,
        Value::Bool(_) if attr == "__repr__" => "bool",
        Value::Int(_) if attr == "__repr__" => "int",
        Value::Int(_) | Value::Bool(_)
            if real_methods("int").contains(&attr)
                || matches!(attr, "__add__" | "__index__" | "bit_length" | "to_bytes") =>
        {
            "int"
        }
        Value::Float(_)
            if real_methods("float").contains(&attr)
                || matches!(attr, "__repr__" | "as_integer_ratio") =>
        {
            "float"
        }
        Value::Str(_) | Value::SurrogateStr(_) if STR_METHODS.contains(&attr) => "str",
        Value::Range(_) if super::range::METHODS.contains(&attr) => "range",
        Value::DictIterator(iter) if DictIterator::methods(iter.class_name()).contains(&attr) => {
            iter.class_name()
        }
        Value::Iterator {
            kind:
                kind @ ("list_iterator" | "tuple_iterator" | "str_ascii_iterator" | "str_iterator"),
            ..
        } if matches!(attr, "__iter__" | "__next__") => kind,
        Value::RangeIterator(iter) if super::range::ITER_METHODS.contains(&attr) => iter.kind,
        Value::Slice(_) if super::slice::METHODS.contains(&attr) => "slice",
        Value::TupleGetterDoc | Value::ReadOnlyMember(_, _) | Value::SlotMember { .. }
            if super::namedtuple::MEMBER_SLOTS.contains(&attr) || attr == "__reduce__" =>
        {
            "member_descriptor"
        }
        Value::TupleGetter { .. } if super::namedtuple::GETTER_METHODS.contains(&attr) => {
            super::namedtuple::GETTER
        }
        Value::ItemGetter(_) if attr == "__reduce__" => "operator.itemgetter",
        Value::NamedTuple {
            class: Some(owner), ..
        } if let Some(layout) = super::stdlib::os::stat::layout(owner)
            && layout.methods().contains(&attr) =>
        {
            owner
        }
        Value::Property(_) if super::property::METHODS.contains(&attr) => "property",
        Value::ClassMethod { .. } | Value::StaticMethod { .. }
            if matches!(attr, "__reduce__" | "__reduce_ex__") =>
        {
            "object"
        }
        Value::ClassMethod { .. } if attr == "__get__" => "classmethod",
        Value::StaticMethod { .. } if attr == "__get__" => "staticmethod",
        Value::AsyncGenerator { .. } if super::async_generator::method("async_generator", attr) => {
            "async_generator"
        }
        Value::AsyncGeneratorOperation { kind, .. }
            if super::async_generator::method(kind.type_name(), attr) =>
        {
            kind.type_name()
        }
        Value::MemoryIterator { .. } if matches!(attr, "__iter__" | "__next__") => {
            "memory_iterator"
        }
        Value::Generator { .. } if super::generator_protocol::generator_method(attr) => "generator",
        Value::Coroutine { .. }
            if super::generator_protocol::coroutine_method("coroutine", attr) =>
        {
            "coroutine"
        }
        Value::CoroutineWrapper(_)
            if super::generator_protocol::coroutine_method("coroutine_wrapper", attr) =>
        {
            "coroutine_wrapper"
        }
        Value::ANextAwaitable { .. }
            if super::generator_protocol::coroutine_method("anext_awaitable", attr) =>
        {
            "anext_awaitable"
        }
        Value::ZlibCompressor(_) if super::value::ZLIB_COMPRESS_METHODS.contains(&attr) => {
            super::value::ZLIB_COMPRESS_TYPE
        }
        Value::MemoryView { .. } if MEMORYVIEW_METHODS.contains(&attr) => "memoryview",
        Value::Bytes(_) if BYTES_METHODS.contains(&attr) => "bytes",
        Value::Complex(_) if super::complex::METHODS.contains(&attr) => "complex",
        Value::ByteArray(_)
            if BYTEARRAY_METHODS.contains(&attr) || matches!(attr, "decode" | "translate") =>
        {
            "bytearray"
        }
        Value::StructUnpackIterator { .. } if UNPACK_ITERATOR_METHODS.contains(&attr) => {
            value::STRUCT_UNPACK_ITERATOR
        }
        Value::Struct { .. } if STRUCT_METHODS.contains(&attr) => "struct.Struct",
        Value::Struct { .. } if attr == "__str__" => "object",
        Value::ZlibDecompressor { .. }
            if matches!(attr, "decompress" | "flush")
                || value::ZLIB_DECOMPRESS_COPY_METHODS.contains(&attr) =>
        {
            super::value::ZLIB_DECOMPRESS_TYPE
        }
        Value::Hash(state)
            if crate::objects::HashState::is_provider_class(state.lock().class_name())
                && matches!(attr, "update" | "copy" | "digest" | "hexdigest") =>
        {
            state.lock().class_name()
        }
        Value::Hash(_) if matches!(attr, "update" | "copy" | "digest" | "hexdigest") => "hash",
        Value::Hmac(_) if matches!(attr, "update" | "copy" | "digest" | "hexdigest") => "hmac.HMAC",
        Value::Socket(_)
            if matches!(
                attr,
                "bind"
                    | "listen"
                    | "accept"
                    | "connect"
                    | "connect_ex"
                    | "getsockname"
                    | "getpeername"
                    | "detach"
                    | "fileno"
                    | "close"
                    | "recv"
                    | "recvfrom"
                    | "recvfrom_into"
                    | "recv_into"
                    | "makefile"
                    | "_decref_socketios"
                    | "dup"
                    | "send"
                    | "sendall"
                    | "sendto"
                    | "shutdown"
                    | "getsockopt"
                    | "setsockopt"
                    | "setblocking"
                    | "settimeout"
                    | "gettimeout"
                    | "get_inheritable"
                    | "set_inheritable"
                    | "getblocking"
                    | "__enter__"
                    | "__exit__"
            ) =>
        {
            "socket.socket"
        }
        // Context-manager lookup bypasses the normal _io descriptors for native files.
        Value::BufferedRandom(..) if matches!(attr, "__enter__" | "__exit__") => "BufferedRandom",
        Value::File(_) if matches!(attr, "__enter__" | "__exit__") => "file",
        Value::TextFile(_)
            if matches!(
                attr,
                "fileno"
                    | "read"
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
        Value::Array(_) if super::array::has_method(attr) => "array.array",
        Value::List(_) if LIST_METHODS.contains(&attr) => "list",
        Value::Tuple(_) | Value::NamedTuple { .. }
            if matches!(
                attr,
                "__getitem__" | "__getnewargs__" | "__iter__" | "__len__" | "count" | "index"
            ) =>
        {
            "tuple"
        }
        Value::Tuple(_) if attr == "__repr__" => "tuple",
        Value::Set { frozen, .. } => {
            let owner = if *frozen { "frozenset" } else { "set" };
            if !(set_methods(owner).contains(&attr) || set_operators(owner).contains(&attr)) {
                return None;
            }
            owner
        }
        Value::DefaultDict { .. } if super::defaultdict::METHODS.contains(&attr) => {
            super::defaultdict::CLASS
        }
        Value::Dict(_) | Value::DefaultDict { .. } if DICT_METHODS.contains(&attr) => "dict",
        #[cfg(target_os = "linux")]
        Value::Epoll(_) if super::stdlib::epoll::METHODS.contains(&attr) => "select.epoll",
        Value::Poll(_) if matches!(attr, "register" | "modify" | "unregister" | "poll") => {
            "select.poll"
        }
        Value::Selector(_)
            if matches!(
                attr,
                "register"
                    | "modify"
                    | "unregister"
                    | "select"
                    | "get_key"
                    | "get_map"
                    | "close"
                    | "__enter__"
                    | "__exit__"
            ) =>
        {
            "selectors"
        }
        Value::RegexPattern(_) | Value::RegexMatch(_)
            if value::REGEX_COPY_METHODS.contains(&attr) =>
        {
            value.intrinsic_type_name()?
        }
        Value::RegexPattern(_)
            if matches!(
                attr,
                "findall"
                    | "finditer"
                    | "match"
                    | "fullmatch"
                    | "search"
                    | "split"
                    | "sub"
                    | "subn"
                    | "scanner"
            ) =>
        {
            "pattern"
        }
        Value::RegexScanner { .. } if matches!(attr, "match" | "search") => "scanner",
        Value::RegexMatch(_)
            if matches!(
                attr,
                "group" | "groups" | "groupdict" | "expand" | "start" | "end" | "span"
            ) =>
        {
            "match"
        }
        Value::Map { .. } if matches!(attr, "__iter__" | "__next__") => "map",
        Value::Filter { .. } if matches!(attr, "__iter__" | "__next__" | "__reduce__") => "filter",
        Value::Filter { .. } | Value::Range(_) | Value::RangeIterator(_)
            if attr == "__reduce_ex__" =>
        {
            "object"
        }
        Value::Enumerate { .. } if matches!(attr, "__iter__" | "__next__") => "enumerate",
        Value::Count { .. } if matches!(attr, "__iter__" | "__next__") => "itertools.count",
        Value::Deque(_) if attr == "__reduce_ex__" => "object",
        Value::Deque(_) if super::deque::member(attr).is_some() => super::deque::CLASS,
        Value::Repeat(_) if matches!(attr, "__iter__" | "__next__" | "__length_hint__") => {
            "itertools.repeat"
        }
        Value::Function { .. } | Value::Cached { .. } if attr == "__get__" => "function",
        Value::Partial { .. } if attr == "__get__" => "functools.partial",
        Value::ContextVar { .. } if matches!(attr, "get" | "set" | "reset") => {
            "_contextvars.ContextVar"
        }
        Value::Context(_)
            if matches!(attr, "copy" | "run" | "get" | "keys" | "values" | "items") =>
        {
            "_contextvars.Context"
        }
        Value::ContextToken { .. } if matches!(attr, "__enter__" | "__exit__") => {
            "_contextvars.Token"
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
pub(super) fn one_positional_arg(name: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
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
/// An empty name marks a positional-only parameter.
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
        if !key.is_empty() && kwargs.iter().any(|(provided, _)| provided == key) {
            return Err(type_error(format!(
                "argument for {name}() given by name ('{key}') and position ({})",
                i + 1
            )));
        }
    }
    let mut values = std::array::from_fn(|i| args.get(i).copied());
    for (key, value) in kwargs {
        let index = names
            .iter()
            .position(|&arg| !arg.is_empty() && arg == key)
            .ok_or_else(|| {
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
            && !names[..args.len()]
                .iter()
                .any(|name| kwargs.iter().any(|(key, _)| *name == key))
        {
            return Err(type_error(format!(
                "{builtin}() got an unexpected keyword argument '{key}'. Did you mean '{hint}'?"
            )));
        }
    }
    bind_method_arguments(builtin, args, kwargs, names, N)
}

/// Bind two required positional-or-keyword operands followed by keyword-only options.
pub(super) fn bind_two_positional<const N: usize>(
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    names: [&str; N],
) -> Result<[Option<Id>; N]> {
    if args.len() > 2 && args.len() + kwargs.len() <= N {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes exactly 2 positional arguments ({} given)",
                args.len()
            ),
        ));
    }
    bind_builtin_arguments(name, args, kwargs, names, 2)
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
    power::ternary(ctx, base, exp, modulus)
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

impl Context {
    /// Direct Python calls carry their lexical scope, including inlined comprehensions.
    pub(super) fn call_in_scope(
        &mut self,
        scope: Id,
        callable: Id,
        args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        if let Value::Native(name) = self.heap.get(callable) {
            match name.as_str() {
                "eval" => return self.call_eval_exec(scope, "eval", &args, &kwargs),
                "exec" => return self.call_eval_exec(scope, "exec", &args, &kwargs),
                "locals" | "vars" if args.is_empty() && kwargs.is_empty() => {
                    return self.frame_locals(scope);
                }
                "dir"
                    if args.is_empty()
                        && kwargs.is_empty()
                        && self.scope_data(scope).parent.is_none() =>
                {
                    let namespace = self.module_globals(scope)?;
                    return collections::sorted(self, &[namespace], &[]);
                }
                _ => {}
            }
        }
        self.call(callable, args, kwargs)
    }
}

/// Validate the receiver and keywords of a positional-only native method.
pub(super) fn method_receiver_args<'a>(
    ctx: &Context,
    owner: &str,
    method: &str,
    args: &'a [Id],
    kwargs: &[(String, Id)],
) -> Result<(Id, &'a [Id])> {
    let short = owner.rsplit('.').next().unwrap_or(owner);
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(type_error(format!(
            "unbound method {short}.{method}() needs an argument"
        )));
    };
    if !ctx.builtin_slot_accepts(receiver, owner) {
        let actual = ctx
            .builtin_type_name(receiver)
            .unwrap_or_else(|| ctx.type_name(receiver));
        return Err(type_error(format!(
            "descriptor '{method}' for '{owner}' objects doesn't apply to a '{actual}' object"
        )));
    }
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "{short}.{method}() takes no keyword arguments"
        )));
    }
    Ok((receiver, rest))
}

pub(super) fn noargs_method_receiver(
    ctx: &Context,
    owner: &str,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (receiver, rest) = method_receiver_args(ctx, owner, method, args, kwargs)?;
    if !rest.is_empty() {
        let short = owner.rsplit('.').next().unwrap_or(owner);
        return Err(type_error(format!(
            "{short}.{method}() takes no arguments ({} given)",
            rest.len()
        )));
    }
    Ok(receiver)
}

/// Look up a reconstruction callable in the current frame's builtins, as PyEval_GetBuiltin does.
pub(super) fn frame_builtin(ctx: &mut Context, name: &str) -> Result<Id> {
    let scope = ctx.heap.current_scope();
    let builtins = if scope != NONE
        && let Some(globals) = ctx.scope_data(ctx.root_scope(scope)).exec_globals
    {
        globals.builtins
    } else {
        ctx.default_source_builtins()?
    };
    ctx.load_exec_name(builtins, name).map_err(|error| {
        if ctx.is_builtin_error(&error, "KeyError") {
            Error::new("AttributeError", name)
        } else {
            error
        }
    })
}

/// Reduce a builtin using its stored receiver and name, without consulting receiver attributes.
fn reduce_builtin_method(ctx: &mut Context, method: Id) -> Result<Id> {
    let (callable, receiver) = match *ctx.heap.get(method) {
        Value::Method {
            callable, receiver, ..
        } => (callable, Some(receiver)),
        Value::Native(ref symbol) => {
            let owner = native_method_owner(symbol).map(str::to_owned);
            (method, owner.map(|owner| ctx.builtin(&owner)))
        }
        _ => unreachable!("validated builtin function or method"),
    };
    let name = match ctx.heap.get(callable) {
        Value::Native(symbol) => symbol.rsplit('.').next().unwrap(),
        Value::HeapTypeMethod { name, .. } | Value::IoMethod { name, .. } => name,
        Value::IoOpen { .. } => "open",
        _ => unreachable!("validated native callable"),
    }
    .to_owned();
    let Some(receiver) =
        receiver.filter(|&value| !matches!(ctx.heap.get(value), Value::Module { .. }))
    else {
        return Ok(ctx.text(name));
    };
    let name = ctx.text(name);
    bound_method_reduction(ctx, receiver, name)
}

/// Rebuild a bound method by looking up its name on the restored receiver.
fn bound_method_reduction(ctx: &mut Context, receiver: Id, name: Id) -> Result<Id> {
    let getattr = frame_builtin(ctx, "getattr")?;
    let args = ctx.alloc(Value::Tuple(vec![receiver, name]));
    Ok(ctx.alloc(Value::Tuple(vec![getattr, args])))
}

/// Apply the native absolute-value implementation without invoking an override.
fn absolute_value(ctx: &mut Context, value: Id) -> Result<Id> {
    let scalar = ctx.scalar_value(value);
    match ctx.heap.get(scalar) {
        Value::Calendar {
            value: duration, ..
        } if duration.class() == "timedelta" => {
            super::stdlib::call(ctx, "datetime.timedelta.__abs__", &[value], &[])
        }
        Value::Float(number) => Ok(ctx.alloc(Value::Float(number.abs()))),
        Value::Complex(number) => {
            let magnitude = number.real.hypot(number.imag);
            if magnitude.is_infinite() && number.real.is_finite() && number.imag.is_finite() {
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
                Ok(integer::exact_value(ctx, value))
            }
        }
        _ => Err(type_error(format!(
            "bad operand type for abs(): '{}'",
            ctx.type_name(value)
        ))),
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "filter.__reduce__" {
        let receiver = noargs_method_receiver(ctx, "filter", "__reduce__", args, kwargs)?;
        let Value::Filter { predicate, input } = *ctx.heap.get(ctx.scalar_value(receiver)) else {
            unreachable!("validated filter")
        };
        let args = ctx.alloc(Value::Tuple(vec![predicate, input]));
        let class = ctx.exact_type(receiver)?;
        return Ok(ctx.alloc(Value::Tuple(vec![class, args])));
    }
    if name == "method.__reduce__" {
        let method = noargs_method_receiver(ctx, "method", "__reduce__", args, kwargs)?;
        let Value::Method {
            callable, receiver, ..
        } = *ctx.heap.get(method)
        else {
            unreachable!("validated Python method")
        };
        let name = ctx.lookup_attr(callable, "__name__")?;
        return bound_method_reduction(ctx, receiver, name);
    }
    if name == "builtin_function_or_method.__reduce__" {
        let method = noargs_method_receiver(
            ctx,
            "builtin_function_or_method",
            "__reduce__",
            args,
            kwargs,
        )?;
        return reduce_builtin_method(ctx, method);
    }
    if name == "BaseException.add_note" {
        let Some((&exception, args)) = args.split_first() else {
            return Err(type_error(
                "unbound method BaseException.add_note() needs an argument",
            ));
        };
        if !matches!(ctx.heap.get(exception), Value::Raised(_)) {
            let kind = ctx
                .builtin_type_name(exception)
                .unwrap_or_else(|| ctx.type_name(exception));
            return Err(type_error(format!(
                "descriptor 'add_note' for 'BaseException' objects doesn't apply to a '{kind}' object"
            )));
        }
        let note = one_positional_arg(name, args, kwargs)?;
        ctx.add_exception_note(exception, note)?;
        return Ok(NONE);
    }
    if name == "mappingproxy.__class_getitem__" {
        return collections::method(ctx, name, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("mappingproxy.")
        && MAPPING_PROXY_METHODS.contains(&method)
    {
        let Some((&proxy, rest)) = args.split_first() else {
            return Err(type_error(format!(
                "unbound method {name}() needs an argument"
            )));
        };
        let Value::MappingProxy(mapping) = *ctx.heap.get(proxy) else {
            return Err(type_error(format!(
                "descriptor '{method}' for 'mappingproxy' objects doesn't apply to a '{}' object",
                ctx.type_name(proxy)
            )));
        };
        if !kwargs.is_empty() {
            return Err(type_error(format!("{name}() takes no keyword arguments")));
        }
        let forwarded = match (method, rest) {
            ("get", [key]) => vec![*key, NONE],
            ("get", [key, default]) => vec![*key, *default],
            ("get", []) => return Err(type_error("get expected at least 1 argument, got 0")),
            ("get", _) => {
                return Err(type_error(format!(
                    "get expected at most 2 arguments, got {}",
                    rest.len()
                )));
            }
            (_, []) => Vec::new(),
            _ => {
                return Err(type_error(format!(
                    "{name}() takes no arguments ({} given)",
                    rest.len()
                )));
            }
        };
        let mapping = if method == "__reversed__" {
            ctx.mapping_proxy_target(mapping)
        } else {
            mapping
        };
        let bound = ctx.lookup_attr(mapping, method)?;
        return ctx.call(bound, forwarded, Vec::new());
    }
    if let Some(owner @ ("object" | "super")) = name.strip_suffix(".__getattribute__") {
        let Some((&object, values)) = args.split_first() else {
            return Err(type_error(format!(
                "descriptor '__getattribute__' of '{owner}' object needs an argument"
            )));
        };
        if owner == "super" && !ctx.builtin_slot_accepts(object, owner) {
            return Err(type_error(format!(
                "descriptor '__getattribute__' requires a 'super' object but received a '{}'",
                ctx.type_name(object)
            )));
        }
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
        return if owner == "super" {
            ctx.super_getattribute(object, &attr)
        } else {
            ctx.default_get_attr(object, &attr)
        };
    }
    if matches!(name, "setattr" | "object.__setattr__") {
        return set_attribute(ctx, name, args, kwargs);
    }
    if matches!(name, "delattr" | "object.__delattr__") {
        return delete_attribute(ctx, name, args, kwargs);
    }
    if name == "types.SimpleNamespace.__new__" {
        return super::namespace_init::allocate(ctx, args);
    }
    if let Some(owner) = name.strip_suffix(".__new__")
        && is_exception(owner)
    {
        return exceptions::allocate(ctx, owner, args);
    }
    if let Some(owner) = exception_init_descriptor(name) {
        return exceptions::initialize(ctx, owner, args, kwargs);
    }
    let complex_op = match name {
        "complex.__mul__" => Some(("*", false)),
        "complex.__rmul__" => Some(("*", true)),
        "complex.__truediv__" => Some(("/", false)),
        "complex.__rtruediv__" => Some(("/", true)),
        "complex.__pow__" => Some(("**", false)),
        "complex.__rpow__" => Some(("**", true)),
        _ => None,
    };
    if matches!(
        name,
        "object.__hash__"
            | "object.__repr__"
            | "object.__str__"
            | "object.__ne__"
            | "object.__eq__"
            | "object.__lt__"
            | "object.__le__"
            | "object.__gt__"
            | "object.__ge__"
            | "mappingproxy.__lt__"
            | "mappingproxy.__le__"
            | "mappingproxy.__eq__"
            | "mappingproxy.__ne__"
            | "mappingproxy.__gt__"
            | "mappingproxy.__ge__"
            | "mappingproxy.__hash__"
            | "mappingproxy.__repr__"
            | "mappingproxy.__str__"
            | "mappingproxy.__iter__"
            | "mappingproxy.__len__"
            | "mappingproxy.__or__"
            | "mappingproxy.__ror__"
            | "mappingproxy.__ior__"
            | "super.__repr__"
            | "struct.Struct.__repr__"
    ) || complex_op.is_some()
    {
        let (owner, method) = name.rsplit_once('.').unwrap();
        let owner = if owner == "struct.Struct" {
            "_struct.Struct"
        } else {
            owner
        };
        let Some((&receiver, rest)) = args.split_first() else {
            return Err(type_error(format!(
                "descriptor '{method}' of '{owner}' object needs an argument"
            )));
        };
        if (owner == "mappingproxy" && !matches!(ctx.heap.get(receiver), Value::MappingProxy(_)))
            || (owner == "_struct.Struct"
                && !matches!(
                    ctx.heap.get(ctx.scalar_value(receiver)),
                    Value::Struct { .. }
                ))
            || (matches!(owner, "super" | "complex") && !ctx.builtin_slot_accepts(receiver, owner))
        {
            return Err(type_error(format!(
                "descriptor '{method}' requires a '{owner}' object but received a '{}'",
                ctx.type_name(receiver)
            )));
        }
        if !kwargs.is_empty() {
            return Err(type_error(format!(
                "wrapper {method}() takes no keyword arguments"
            )));
        }
        if let Some(("**", reflected)) = complex_op {
            let (other, modulus) = match *rest {
                [other] => (other, None),
                [other, third] => (other, Some(third)),
                _ => {
                    return Err(type_error(format!(
                        "expected 1 or 2 arguments, got {}",
                        rest.len()
                    )));
                }
            };
            let (base, exponent) = if reflected {
                (other, receiver)
            } else {
                (receiver, other)
            };
            return Ok(ctx
                .complex_power(base, exponent, modulus)?
                .unwrap_or(NOT_IMPLEMENTED));
        }
        let comparison = super::operators::comparison_op(method);
        if complex_op.is_some()
            || comparison.is_some()
            || matches!(method, "__or__" | "__ror__" | "__ior__")
        {
            let [other] = rest else {
                return Err(type_error(format!(
                    "expected 1 argument, got {}",
                    rest.len()
                )));
            };
            if let Some((op, reflected)) = complex_op {
                let (a, b) = if reflected {
                    (*other, receiver)
                } else {
                    (receiver, *other)
                };
                return Ok(ctx
                    .complex_product_or_quotient(op, a, b)?
                    .unwrap_or(NOT_IMPLEMENTED));
            }
            if owner == "object" {
                if method != "__ne__" {
                    return Ok(if method == "__eq__" && receiver == *other {
                        TRUE
                    } else {
                        NOT_IMPLEMENTED
                    });
                }
                return ctx.with_slot_depth(
                    "maximum recursion depth exceeded while calling a Python object",
                    |ctx| ctx.object_ne(receiver, *other),
                );
            }
            if method == "__ior__" {
                return Err(type_error(
                    "'|=' is not supported by mappingproxy; use '|' instead",
                ));
            }
            let Value::MappingProxy(mapping) = *ctx.heap.get(receiver) else {
                unreachable!("validated mapping proxy slot")
            };
            if let Some(op) = comparison {
                return ctx.binary(op, mapping, *other);
            }
            let other = match *ctx.heap.get(*other) {
                Value::MappingProxy(backing) => backing,
                _ => *other,
            };
            return if method == "__ror__" {
                ctx.binary("|", other, mapping)
            } else {
                ctx.binary("|", mapping, other)
            };
        }
        if !rest.is_empty() {
            return Err(type_error(format!(
                "expected 0 arguments, got {}",
                rest.len()
            )));
        }
        if owner == "mappingproxy" {
            if method == "__iter__" {
                return ctx.iterator(receiver);
            }
            if method == "__len__" {
                return call(ctx, "len", &[receiver], &[]);
            }
            if method == "__hash__" {
                let hash = ctx.dictionary_hash(receiver)?;
                return Ok(ctx.int(hash));
            }
            if method == "__str__" {
                return call(ctx, "str", &[receiver], &[]);
            }
            let repr = ctx.checked_format(receiver, true)?;
            return Ok(ctx.text(repr));
        }
        if owner == "super" {
            return Ok(ctx.text(ctx.super_repr(receiver)));
        }
        if owner == "_struct.Struct" {
            return Ok(ctx.text(super::stdlib::structure::repr(ctx, receiver)?));
        }
        if method == "__repr__" {
            return ctx.object_repr(receiver);
        }
        if method == "__str__" {
            if let Some(value) = ctx.repr_override(receiver)? {
                return Ok(value);
            }
            let repr = ctx.checked_format(receiver, true)?;
            return Ok(ctx.text(repr));
        }
        return Ok(ctx.int(Context::identity_hash(receiver)));
    }
    if name == "object.__dir__" {
        let [receiver, rest @ ..] = args else {
            return Err(type_error(
                "unbound method object.__dir__() needs an argument",
            ));
        };
        if !kwargs.is_empty() {
            return Err(type_error("object.__dir__() takes no keyword arguments"));
        }
        if !rest.is_empty() {
            return Err(type_error(format!(
                "object.__dir__() takes no arguments ({} given)",
                rest.len()
            )));
        }
        let directory = dir::namespace_directory(ctx, *receiver, true)?;
        let keys = ctx
            .dict_entries(directory)
            .unwrap()
            .iter()
            .map(|&(key, _)| key)
            .collect();
        return Ok(ctx.alloc(Value::List(keys)));
    }
    if matches!(name, "object.__init_subclass__" | "object.__subclasshook__") {
        let method = name.strip_prefix("object.").unwrap();
        let [class, rest @ ..] = args else {
            return Err(type_error(format!(
                "descriptor '{method}' of 'object' object needs an argument"
            )));
        };
        super::stdlib::datetime::validate_classmethod_receiver(ctx, "object", method, *class)?;
        if !kwargs.is_empty() {
            let name = ctx
                .own_class_attr(*class, "__qualname__")
                .and_then(|id| ctx.str_value(id).ok())
                .unwrap_or_else(|| ctx.class_name(*class));
            return Err(type_error(format!(
                "{name}.{method}() takes no keyword arguments"
            )));
        }
        if method == "__subclasshook__" {
            if rest.len() != 1 {
                return Err(type_error(format!(
                    "{}.{method}() takes exactly one argument ({} given)",
                    ctx.class_name(*class),
                    rest.len()
                )));
            }
            return Ok(NOT_IMPLEMENTED);
        }
        if !rest.is_empty() {
            return Err(type_error(format!(
                "{}.__init_subclass__() takes no arguments ({} given)",
                ctx.class_name(*class),
                rest.len()
            )));
        }
        return Ok(NONE);
    }
    if name == "object.__init__" {
        if let Some(receiver) = args.first()
            && crate::objects::HashState::is_provider_class(ctx.type_name(*receiver))
        {
            return Ok(NONE);
        }
        if args.len() != 1 || !kwargs.is_empty() {
            if let Some(&receiver) = args.first() {
                let class = ctx.exact_type(receiver)?;
                let init = ctx.class_attr(class, "__init__");
                if init.is_none_or(|init| Some(&init) == ctx.builtins.get("object.__init__")) {
                    let new =
                        ctx.class_attr(class, "__new__")
                            .map(|slot| match *ctx.heap.get(slot) {
                                Value::StaticMethod { callable, .. } => callable,
                                _ => slot,
                            });
                    let tuple = ctx.builtin("tuple");
                    if ctx.instance_of(receiver, tuple)
                        || new.is_some_and(|new| Some(&new) != ctx.builtins.get("object.__new__"))
                    {
                        // The allocator consumed the arguments; the inherited initializer does nothing.
                        return Ok(NONE);
                    }
                }
            }
            return Err(type_error(
                "object.__init__() takes exactly one argument (the instance to initialize)",
            ));
        }
        return Ok(NONE);
    }
    // Module-qualified exceptions use the exception constructor below.
    if let Some((owner, method)) = name.split_once('.')
        && !is_exception(name)
    {
        return match owner {
            "super" if method == "__new__" => ctx.super_new(args, kwargs),
            "super" if method == "__init__" => ctx.super_init(args, kwargs),
            "int" | "float" | "str" | "bytes" if method == "__new__" => {
                let Some((&class, values)) = args.split_first() else {
                    return Err(type_error(format!("{name}(): not enough arguments")));
                };
                ctx.new_immutable_scalar(owner, class, values, kwargs)
            }
            "type" if method == "__init__" => {
                let Some((&class, args)) = args.split_first() else {
                    return Err(type_error(
                        "descriptor '__init__' of 'type' object needs an argument",
                    ));
                };
                if !ctx.is_type_object(class) {
                    return Err(type_error(format!(
                        "descriptor '__init__' requires a 'type' object but received a '{}'",
                        ctx.type_name(class)
                    )));
                }
                match args.len() {
                    1 if !kwargs.is_empty() => {
                        Err(type_error("type.__init__() takes no keyword arguments"))
                    }
                    1 | 3 => Ok(NONE),
                    _ => Err(type_error("type.__init__() takes 1 or 3 arguments")),
                }
            }
            "async_generator" | "async_generator_asend" | "async_generator_athrow"
                if super::async_generator::method(owner, method) =>
            {
                positional(name, kwargs)?;
                let (min, max) = match method {
                    "send" | "asend" => (2, 2),
                    "throw" => (2, 4),
                    "athrow" => (1, usize::MAX),
                    _ => (1, 1),
                };
                arity(name, args, min, max)?;
                if ctx.heap.get(args[0]).intrinsic_type_name() != Some(owner) {
                    return Err(type_error(format!(
                        "descriptor '{method}' requires a '{owner}' object but received a '{}'",
                        ctx.type_name(args[0])
                    )));
                }
                if owner == "async_generator" {
                    ctx.call_async_generator(args[0], method, &args[1..])
                } else {
                    ctx.call_async_generator_op(args[0], method, &args[1..])
                }
            }
            "anext_awaitable" => {
                positional(name, kwargs)?;
                let (min, max) = match method {
                    "send" => (2, 2),
                    "throw" => (1, usize::MAX),
                    _ => (1, 1),
                };
                arity(name, args, min, max)?;
                ctx.call_anext_method(method, args)
            }
            "coroutine" | "coroutine_wrapper" => {
                positional(name, kwargs)?;
                let (min, max) = match method {
                    "send" => (2, 2),
                    "throw" => (2, 4),
                    _ => (1, 1),
                };
                arity(name, args, min, max)?;
                ctx.call_coroutine_method(owner, method, args)
            }
            "generator" | "memory_iterator" if matches!(method, "__iter__" | "__next__") => {
                ctx.iterator_method(owner, method, args, kwargs)
            }
            "generator" if method == "send" => {
                let [generator, value] = args else {
                    return Err(type_error("generator.send() takes exactly one argument"));
                };
                if !kwargs.is_empty() {
                    return Err(type_error("generator.send() takes no keyword arguments"));
                }
                ctx.generator_send(*generator, *value)
            }
            "generator" if method == "throw" => {
                positional(name, kwargs)?;
                arity(name, args, 2, 4)?;
                ctx.generator_throw(args[0], &args[1..])
            }
            "generator" if method == "close" => {
                positional(name, kwargs)?;
                arity(name, args, 1, 1)?;
                ctx.generator_close(args[0])
            }
            "object" | "int" | "float" | "str" if method == "__format__" => {
                positional(name, kwargs)?;
                arity(name, args, 2, 2)?;
                if owner != "object" && !ctx.builtin_slot_accepts(args[0], owner) {
                    return Err(type_error(format!(
                        "descriptor '__format__' requires a '{owner}' object"
                    )));
                }
                text::builtin_format(ctx, owner, args[0], Some(args[1]))
            }
            "module" => ctx.module_method(method, args, kwargs),
            "float" if method == "fromhex" => float::fromhex(ctx, args, kwargs),
            "float" if method == "__getformat__" => float::getformat(ctx, args, kwargs),
            "int" | "float"
                if matches!(method, "__abs__" | "__int__" | "__float__")
                    || super::operators::comparison_op(method).is_some() =>
            {
                collections::builtin_slot(ctx, name, args, kwargs)
            }
            "int" | "float" if method == "__round__" => {
                let (receiver, rest) = method_receiver_args(ctx, owner, method, args, kwargs)?;
                if rest.len() > 1 {
                    return Err(type_error(format!(
                        "__round__ expected at most 1 argument, got {}",
                        rest.len()
                    )));
                }
                round::native(
                    ctx,
                    receiver,
                    rest.first().copied().filter(|&id| id != NONE),
                )
            }
            "int" | "float" if method == "__trunc__" => {
                let receiver = noargs_method_receiver(ctx, owner, method, args, kwargs)?;
                integer::truncate_real(ctx, receiver)
            }
            "float" if matches!(method, "__ceil__" | "__floor__") => {
                let receiver = noargs_method_receiver(ctx, owner, method, args, kwargs)?;
                let Value::Float(value) = *ctx.heap.get(ctx.scalar_value(receiver)) else {
                    unreachable!("validated float receiver")
                };
                round_float_to_integer(ctx, value, method)
            }
            "float" if method == "hex" => float::hex(ctx, args, kwargs),
            "float" if method == "is_integer" => {
                let receiver = noargs_method_receiver(ctx, owner, method, args, kwargs)?;
                let Value::Float(value) = ctx.heap.get(ctx.scalar_value(receiver)) else {
                    unreachable!("validated float receiver")
                };
                Ok(boolean(value.is_finite() && value.trunc() == *value))
            }
            "float" if method == "as_integer_ratio" => float::as_integer_ratio(ctx, args, kwargs),
            "getset_descriptor" if matches!(method, "__set__" | "__delete__") => {
                if !kwargs.is_empty() {
                    return Err(type_error(format!(
                        "wrapper {method}() takes no keyword arguments"
                    )));
                }
                let (required, message) = if method == "__set__" {
                    (2, "__set__ expected 2 arguments")
                } else {
                    (1, "expected 1 argument")
                };
                let Some((&descriptor, values)) = args.split_first() else {
                    return Err(type_error("expected an integer getset descriptor"));
                };
                if values.len() != required {
                    return Err(type_error(format!("{message}, got {}", values.len())));
                }
                let Value::GetSetDescriptor {
                    get,
                    owner,
                    name: field,
                    set,
                } = *ctx.heap.get(descriptor)
                else {
                    return Err(type_error("expected a getset descriptor"));
                };
                let accepts = if let Value::HeapTypeMethod { owner: class, .. } = *ctx.heap.get(get)
                {
                    ctx.check_heap_method_receiver(class, field, values[0])?;
                    true
                } else if let Value::IoMethod { owner: class, .. } = *ctx.heap.get(get) {
                    super::stdlib::io::receiver_class(ctx, values[0])
                        .is_some_and(|receiver| ctx.direct_subclass(receiver, class))
                } else if matches!(owner, "int" | "float")
                    || Context::dict_view_owner(owner).is_some()
                {
                    ctx.builtin_slot_accepts(values[0], owner)
                } else if owner == "typing.Union" {
                    matches!(ctx.heap.get(values[0]), Value::Union(_))
                } else if owner == "typing.TypeAliasType" {
                    matches!(ctx.heap.get(values[0]), Value::TypeAlias { .. })
                } else if crate::objects::HashState::is_provider_class(owner) {
                    ctx.type_name(values[0]) == owner
                } else {
                    return Err(type_error("expected a read-only getset descriptor"));
                };
                if !accepts {
                    return Err(type_error(format!(
                        "descriptor '{field}' for '{owner}' objects doesn't apply to a '{}' object",
                        ctx.type_name(values[0])
                    )));
                }
                if let Some(set) = set {
                    return ctx.call(set, values.to_vec(), Vec::new());
                }
                Err(Error::new(
                    "AttributeError",
                    format!("attribute '{field}' of '{owner}' objects is not writable"),
                ))
            }
            "_dictview_mapping" => {
                let receiver = noargs_method_receiver(ctx, method, "mapping", args, kwargs)?;
                let Value::DictView { dict, .. } = *ctx.heap.get(receiver) else {
                    unreachable!("dictionary view receiver checked");
                };
                Ok(ctx.alloc(Value::MappingProxy(dict)))
            }
            "_int_getset" => integer::rational_field(ctx, method, args),
            "_float_getset" => float::component(ctx, method, args, kwargs),
            "int" if method == "__getnewargs__" => {
                let receiver = noargs_method_receiver(ctx, owner, method, args, kwargs)?;
                let value = ctx.as_int(receiver)?;
                let value = ctx.alloc(Value::Int(value));
                Ok(ctx.alloc(Value::Tuple(vec![value])))
            }
            "int" if method == "bit_length" => integer::bit_length(ctx, args, kwargs),
            "int" if method == "from_bytes" => integer::from_bytes(ctx, args, kwargs),
            "int" if method == "to_bytes" => integer::to_bytes(ctx, args, kwargs),
            _ if matches!(
                (owner, method),
                ("bytes", "__bytes__") | ("complex", "__complex__")
            ) =>
            {
                let receiver = noargs_method_receiver(ctx, owner, method, args, kwargs)?;
                let storage = ctx.scalar_value(receiver);
                if storage == receiver {
                    return Ok(receiver);
                }
                let value = match ctx.heap.get(storage) {
                    Value::Bytes(bytes) => {
                        let bytes = super::copy_ids(bytes)?;
                        return Ok(ctx.heap.bytes_from_data(bytes));
                    }
                    Value::Complex(number) => Value::Complex(*number),
                    _ => unreachable!("descriptor receiver has checked native layout"),
                };
                Ok(ctx.alloc(value))
            }
            "bytes" | "bytearray" if method == "maketrans" => {
                translation::maketrans(ctx, name, args, kwargs)
            }
            "bytes" | "bytearray" if method == "translate" => {
                translation::translate(ctx, owner, args, kwargs)
            }
            "int"
                if matches!(
                    method,
                    "__add__" | "__bool__" | "__hash__" | "__index__" | "__pow__"
                ) =>
            {
                collections::builtin_slot(ctx, name, args, kwargs)
            }
            "str" if method == "__str__" => collections::builtin_slot(ctx, name, args, kwargs),
            "mappingproxy" if method == "__contains__" => {
                collections::builtin_slot(ctx, name, args, kwargs)
            }
            "bool" | "int" | "float" | "str" | "bytes" | "bytearray" if method == "__repr__" => {
                collections::builtin_slot(ctx, name, args, kwargs)
            }
            _ if matches!(name, "bytes.lower" | "bytes.upper" | "bytearray.upper") => {
                let receiver = noargs_method_receiver(ctx, owner, method, args, kwargs)?;
                let storage = ctx.scalar_value(receiver);
                let bytes = ctx.bytes_like(storage)?;
                if owner == "bytes" && bytes.is_empty() {
                    return Ok(storage);
                }
                let mut result = super::copy_ids(bytes)?;
                if method == "lower" {
                    result.make_ascii_lowercase();
                } else {
                    result.make_ascii_uppercase();
                }
                Ok(ctx.alloc(if owner == "bytes" {
                    Value::Bytes(result)
                } else {
                    Value::ByteArray(result)
                }))
            }
            "bytes" | "bytearray" if method == "hex" => text::bytes_hex(ctx, owner, args, kwargs),
            "bytes" | "bytearray" if method == "fromhex" => text::fromhex(ctx, name, args, kwargs),
            "bytearray"
                if matches!(
                    method,
                    "decode"
                        | "endswith"
                        | "ljust"
                        | "lstrip"
                        | "replace"
                        | "rstrip"
                        | "split"
                        | "splitlines"
                        | "startswith"
                        | "strip"
                ) =>
            {
                text::method(ctx, owner, method, args, kwargs)
            }
            "str" | "bytes" => text::method(ctx, owner, method, args, kwargs),
            "bytearray" if BYTEARRAY_METHODS.contains(&method) => {
                text::bytearray_method(ctx, name, method, args, kwargs)
            }
            "tuple" if method == "__getnewargs__" => {
                let receiver = noargs_method_receiver(ctx, owner, method, args, kwargs)?;
                let payload = if matches!(ctx.heap.get(receiver), Value::Tuple(_)) {
                    receiver
                } else {
                    let items = ctx
                        .tuple_items(receiver)?
                        .expect("validated tuple receiver");
                    ctx.alloc(Value::Tuple(items))
                };
                Ok(ctx.alloc(Value::Tuple(vec![payload])))
            }
            "tuple" | "mappingproxy" if method == "__getitem__" => {
                let (&receiver, args) = args.split_first().ok_or_else(|| {
                    type_error(format!(
                        "descriptor '__getitem__' of '{owner}' object needs an argument"
                    ))
                })?;
                if !ctx.builtin_slot_accepts(receiver, owner) {
                    return Err(type_error(format!(
                        "descriptor '__getitem__' requires a '{owner}' object but received a '{}'",
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
                if owner == "mappingproxy" {
                    return ctx.get_item(receiver, *key);
                }
                ctx.sequence_subscript(receiver, *key)
            }
            "set" | "frozenset" if set_operator(method).is_some() => {
                collections::builtin_slot(ctx, name, args, kwargs)
            }
            "list" | "tuple" | "dict" | "set" | "frozenset" => {
                collections::method(ctx, name, args, kwargs)
            }
            "memoryview" if matches!(method, "__getitem__" | "__setitem__" | "__delitem__") => {
                ctx.buffer_item_method("memoryview", method, args, kwargs)
            }
            "memoryview" if method == "cast" => ctx.cast_memoryview(args, kwargs),
            "memoryview" if MEMORYVIEW_METHODS.contains(&method) => {
                if method == "tobytes" {
                    if args.is_empty() {
                        return Err(type_error(
                            "unbound method memoryview.tobytes() needs an argument",
                        ));
                    }
                    let [order] =
                        bind_method_arguments("tobytes", &args[1..], kwargs, ["order"], 1)?;
                    let fortran = if let Some(order) = order.filter(|&id| id != NONE) {
                        let value = ctx.str_value(order).map_err(|_| {
                            type_error(format!(
                                "tobytes() argument 'order' must be str or None, not {}",
                                ctx.type_name(order)
                            ))
                        })?;
                        if !matches!(value, "C" | "F" | "A") {
                            return Err(Error::new("ValueError", "order must be 'C', 'F' or 'A'"));
                        }
                        value == "F"
                    } else {
                        false
                    };
                    if !matches!(ctx.heap.get(args[0]), Value::MemoryView { .. }) {
                        return Err(type_error("expected memoryview"));
                    }
                    let bytes = ctx.memoryview_bytes_order(args[0], fortran)?;
                    return Ok(ctx.alloc(Value::Bytes(bytes)));
                }
                positional(name, kwargs)?;
                // CPython's __exit__ ignores any positional exception information.
                let max_args = if method == "__exit__" { usize::MAX } else { 1 };
                arity(name, args, 1, max_args)?;
                if method == "toreadonly" {
                    if !matches!(ctx.heap.get(args[0]), Value::MemoryView { .. }) {
                        return Err(type_error("expected memoryview"));
                    }
                    let child = ctx.new_memoryview(args[0])?;
                    if let Value::MemoryView { readonly, .. } = ctx.heap.get_mut(child) {
                        *readonly = true;
                    }
                    return Ok(child);
                }
                let Value::MemoryView { owner, .. } = *ctx.heap.get(args[0]) else {
                    return Err(type_error("expected memoryview"));
                };
                if method == "__iter__" {
                    return ctx.memoryview_iterator(args[0]);
                }
                if method == "__enter__" {
                    Context::memoryview_owner(owner)?;
                    return Ok(args[0]);
                }
                let mut exports = ctx
                    .buffer_exports
                    .iter()
                    .filter(|&&view| view == args[0])
                    .count();
                if ctx.heap.may_have_buffer_views(args[0]) {
                    let (heap, roots) = ctx.heap_and_roots();
                    exports += heap.live_buffer_export_count(args[0], roots)?;
                }
                if exports != 0 {
                    let unit = if exports == 1 { "buffer" } else { "buffers" };
                    return Err(Error::new(
                        "BufferError",
                        format!("memoryview has {exports} exported {unit}"),
                    ));
                }
                let Value::MemoryView { owner, lease, .. } = ctx.heap.get_mut(args[0]) else {
                    unreachable!()
                };
                let owner = owner.take();
                let lease = lease.take();
                if let Some(owner) = owner {
                    ctx.heap.weak_edge_removed(owner);
                }
                if let Some(lease) = lease
                    && let Value::PythonBufferLease { exporter, view, .. } = *ctx.heap.get(lease)
                {
                    ctx.heap.weak_edge_removed(exporter);
                    ctx.heap.weak_edge_removed(view);
                }
                Ok(NONE)
            }
            "slice" => super::slice::method(ctx, method, args, kwargs),
            "range" | "range_iterator" | "longrange_iterator" => {
                super::range::method(ctx, owner, method, args, kwargs)
            }
            "list_iterator" | "tuple_iterator" | "str_ascii_iterator" | "str_iterator" | "map"
            | "filter" | "enumerate" => ctx.iterator_method(owner, method, args, kwargs),
            "property" => super::property::method(ctx, method, args, kwargs),
            "function" | "cell" | "mappingproxy" if method == "__new__" => {
                let Some((&class, args)) = args.split_first() else {
                    return Err(type_error(format!(
                        "{owner}.__new__(): not enough arguments"
                    )));
                };
                if class != ctx.builtin(owner) {
                    return Err(ctx.invalid_new_subtype(owner, class));
                }
                call(ctx, owner, args, kwargs)
            }
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
        "mappingproxy" => {
            let [mapping] = bind_builtin_arguments(name, args, kwargs, ["mapping"], 1)?;
            ctx.mapping_proxy(mapping.expect("required mapping"))
        }
        "type" => match args {
            [object] if kwargs.is_empty() => ctx.exact_type(*object),
            [_] => Err(type_error("type() takes no keyword arguments")),
            [name, bases, namespace] => {
                let class =
                    ctx.type_from_namespace(None, *name, *bases, *namespace, kwargs, false)?;
                let caller = ctx.builtin("type");
                ctx.finish_type_call(caller, class, args.to_vec(), kwargs.to_vec())
            }
            _ => Err(type_error("type() takes 1 or 3 arguments")),
        },
        "super" => {
            if !kwargs.is_empty() {
                return Err(type_error("super() takes no keyword arguments"));
            }
            ctx.make_super(args)
        }
        "compile" => {
            positional(name, kwargs)?;
            arity(name, args, 3, 3)?;
            ctx.compile_source(args[0], args[1], args[2], false)
        }
        "eval" | "exec" => ctx.call_eval_exec(ctx.heap.current_scope(), name, args, kwargs),
        "bin" | "hex" | "oct" => {
            let value = one_positional_arg(name, args, kwargs)?;
            let number = ctx.index_integer(value)?;
            let number = number.big();
            let text = match name {
                "bin" => format!("{:#b}", number.as_ref()),
                "oct" => format!("{:#o}", number.as_ref()),
                _ => format!("{:#x}", number.as_ref()),
            };
            Ok(ctx.text(text))
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
            absolute_value(ctx, value)
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
        "vars" => {
            if !kwargs.is_empty() {
                return Err(type_error("vars() takes no keyword arguments"));
            }
            match args {
                [object] => match ctx.lookup_attr(*object, "__dict__") {
                    Err(error) if ctx.is_attribute_error(&error) => {
                        Err(type_error("vars() argument must have __dict__ attribute"))
                    }
                    result => result,
                },
                [] => ctx.frame_locals(ctx.heap.current_scope()),
                _ => Err(type_error(format!(
                    "vars expected at most 1 argument, got {}",
                    args.len()
                ))),
            }
        }
        "globals" | "locals" => {
            if !kwargs.is_empty() {
                return Err(type_error(format!("{name}() takes no keyword arguments")));
            }
            if !args.is_empty() {
                return Err(type_error(format!(
                    "{name}() takes no arguments ({} given)",
                    args.len()
                )));
            }
            if name == "globals" {
                ctx.module_globals(ctx.heap.current_scope())
            } else {
                ctx.frame_locals(ctx.heap.current_scope())
            }
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
            ctx.divmod(*a, *b)
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
                if let Some(point @ 0xd800..=0xdfff) = code.as_small() {
                    return Ok(ctx.text_codepoints(vec![point as u32]));
                }
                return Ok(ctx.text(crate::objects::chr_scalar(&code)?.to_string()));
            }
            let code = match ctx.heap.get(ctx.scalar_value(*value)) {
                Value::SurrogateStr(points) => match points.as_slice() {
                    [point] => *point,
                    _ => {
                        return Err(type_error(format!(
                            "ord() expected a character, but string of length {} found",
                            points.len()
                        )));
                    }
                },
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
        "iter" | "next" | "anext" => {
            if !kwargs.is_empty() {
                return Err(type_error(format!("{name}() takes no keyword arguments")));
            }
            match args {
                [] => Err(type_error(format!(
                    "{name} expected at least 1 argument, got 0"
                ))),
                [iterator, ..] if name == "anext" && args.len() <= 2 => {
                    let Some(method) = ctx.bound_slot(*iterator, "__anext__")? else {
                        return Err(type_error(format!(
                            "'{:.200}' object is not an async iterator",
                            ctx.type_name(*iterator)
                        )));
                    };
                    let wrapped = ctx.call(method, Vec::new(), Vec::new())?;
                    match args.get(1) {
                        Some(&default) => Ok(ctx.alloc(Value::ANextAwaitable { wrapped, default })),
                        None => Ok(wrapped),
                    }
                }
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
            if matches!(ctx.heap.get(object), Value::MappingProxy(_)) {
                return call(ctx, "mappingproxy.__reversed__", args, kwargs);
            }
            if let Value::Range(range) = ctx.heap.get(object) {
                return Ok(ctx.alloc(Value::RangeIterator(range.iter(true))));
            }
            if let Value::Deque(ring) = ctx.heap.get(object) {
                let iter = super::deque::iterator(object, ring, true, 0);
                return Ok(ctx.alloc(iter));
            }
            let start = match ctx.heap.get(object) {
                Value::List(items) => Some((items.len(), "list_reverseiterator")),
                Value::Tuple(items) => Some((items.len(), "reversed")),
                Value::Str(text) => Some((text.len(), "reversed")),
                Value::SurrogateStr(points) => Some((points.len(), "reversed")),
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
                        Value::Str(_)
                            | Value::SurrogateStr(_)
                            | Value::Bytes(_)
                            | Value::List(_)
                            | Value::Tuple(_)
                    ))
            {
                let remaining = ctx.sequence_len(object)?;
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
                Value::DictView { dict, .. } => collections::entries(ctx, *dict)?.len(),
                value if value.is_weak_proxy() => {
                    let target = ctx.weak_proxy_target(args[0])?;
                    return ctx.with_weak_target(target, |ctx| call(ctx, "len", &[target], &[]));
                }
                Value::MappingProxy(_) => {
                    let mapping = ctx.mapping_proxy_target(args[0]);
                    return call(ctx, "len", &[mapping], &[]);
                }
                Value::Class { .. } if ctx.enum_values(args[0]).is_none() => {
                    ctx.instance_len(args[0])?.ok_or_else(|| no_length(ctx))?
                }
                Value::Class { .. } => {
                    let values = ctx.enum_values(args[0]).ok_or_else(|| no_length(ctx))?;
                    if let Some(names) = ctx.float_enum_names(args[0]) {
                        return call(ctx, "len", &[names], &[]);
                    }
                    collections::entries(ctx, values)?.len()
                }
                Value::Deque(ring) => ring.items.len(),
                Value::Array(array) => array.len(),
                Value::Str(v) if ctx.heap.is_ascii_str(args[0]) => v.len(),
                Value::Str(v) => v.chars().count(),
                Value::SurrogateStr(points) => points.len(),
                Value::Bytes(v) | Value::ByteArray(v) => v.len(),
                Value::MemoryView { .. } => ctx.view_layout(args[0])?.first_len()?,
                Value::Range(range) => {
                    return range.len.to_i64().map(|n| ctx.int(n)).ok_or_else(|| {
                        Error::new(
                            "OverflowError",
                            "Python int too large to convert to C ssize_t",
                        )
                    });
                }
                Value::Instance { .. } | Value::Raised(_) | Value::FrameLocalsProxy { .. } => {
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
        "memoryview" => {
            let [object] = bind(name, args, kwargs, ["object"])?;
            let object = object.ok_or_else(|| {
                type_error("memoryview() missing required argument 'object' (pos 1)")
            })?;
            ctx.new_memoryview(object)
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
        "int" | "bool" | "str" | "bytes" | "bytearray" | "repr" | "ascii" => {
            text::construct(ctx, name, args, kwargs)
        }
        "format" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 2)?;
            format_value(ctx, args[0], args.get(1).copied())
        }
        "classmethod" => {
            positional(name, kwargs)?;
            arity(name, args, 1, 1)?;
            super::method_decorator::class_method(ctx, args[0])
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
            super::method_decorator::static_method(ctx, *callable)
        }
        "property" => super::property::construct(ctx, args, kwargs),
        "open" => super::stdlib::io::open(ctx, args, kwargs, None),
        "__import__" => {
            let [module, globals, _, fromlist, level] = bind(
                name,
                args,
                kwargs,
                ["name", "globals", "locals", "fromlist", "level"],
            )?;
            let name = ctx
                .str_value(module.ok_or_else(|| type_error("__import__ requires a name"))?)?
                .to_owned();
            let level = match level {
                Some(level) => ctx
                    .index_integer(level)?
                    .to_i64()
                    .and_then(|level| i32::try_from(level).ok())
                    .ok_or_else(|| {
                        Error::new("OverflowError", "Python int too large to convert to C int")
                    })?,
                None => 0,
            };
            if level < 0 {
                return Err(Error::new("ValueError", "level must be >= 0"));
            }
            if level != 0 {
                return ctx.relative_import(&name, globals, fromlist, level);
            }
            if name.is_empty() {
                return Err(Error::new("ValueError", "Empty module name"));
            }
            let result = ctx.module(&name)?;
            if let Some(list) = fromlist
                && ctx.truth(list)?
            {
                return ctx.prepare_fromlist(result, list);
            }
            match name.split_once('.') {
                Some((top, _)) => ctx.module(top),
                None => Ok(result),
            }
        }
        "complex" => ctx.new_complex(args, kwargs),
        "float" => float::construct(ctx, args, kwargs),
        "all" | "any" => {
            let iterable = one_positional_arg(name, args, kwargs)?;
            let iter = ctx.iterator(iterable)?;
            ctx.heap.keep(iter);
            let mark = ctx.heap.temporary_mark();
            let stop_value = name == "any";
            while let Some(item) = ctx.next_item(iter)? {
                if ctx.truth(item)? == stop_value {
                    return Ok(boolean(stop_value));
                }
                ctx.heap.release_temporaries_since(mark);
                ctx.heap.reserve_slot_or_request_collection();
                ctx.collect_garbage();
            }
            Ok(boolean(!stop_value))
        }
        "input" => {
            if !kwargs.is_empty() {
                return Err(type_error("input() takes no keyword arguments"));
            }
            if args.len() > 1 {
                return Err(type_error(format!(
                    "input expected at most 1 argument, got {}",
                    args.len()
                )));
            }
            let mark = ctx.heap.temporary_mark();
            let prompt = args.first().copied();
            ctx.heap.reserve_temporaries(3 + args.len())?;
            if let Some(prompt) = prompt {
                ctx.heap.keep(prompt);
            }
            let start = ctx.heap.temporary_mark();
            let mut backend = BoxedInput(ctx, start..start);
            let mut result = crate::input::read(&mut backend, prompt);
            let BoxedInput(ctx, stream_roots) = backend;
            // Drop the consumed line and other work before the captured streams.
            // Each stream's finalizer may observe the others, even on an error.
            for root in (mark..=stream_roots.end).rev() {
                result = ctx.release_with_result(root, result);
            }
            result
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
        "cell" => {
            if !kwargs.is_empty() {
                return Err(type_error("cell() takes no keyword arguments"));
            }
            if args.len() > 1 {
                return Err(type_error(format!(
                    "cell expected at most 1 argument, got {}",
                    args.len()
                )));
            }
            Ok(ctx.alloc(Value::Cell(args.first().copied())))
        }
        "function" => ctx.construct_function(args, kwargs),
        "ImportError" | "ModuleNotFoundError" => {
            let mut error = ctx.exception(name, args);
            exceptions::initialize_import(&mut error, args, kwargs)?;
            Ok(ctx.alloc(Value::Raised(error)))
        }
        name @ ("SyntaxError" | "IndentationError" | "TabError") => {
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
        "BaseExceptionGroup" | "ExceptionGroup" => {
            let class = ctx.builtin(name);
            let group = exceptions::allocate_group(ctx, class, args)?;
            if !kwargs.is_empty() {
                return Err(type_error(format!(
                    "{}() takes no keyword arguments",
                    ctx.type_name(group)
                )));
            }
            Ok(group)
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
        "NoDefaultType" if args.is_empty() && kwargs.is_empty() => {
            Ok(super::typing::no_default(ctx))
        }
        "NoDefaultType" => Err(type_error("NoDefaultType takes no arguments")),
        name => Err(type_error(format!("unsupported builtin {name}"))),
    }
}
