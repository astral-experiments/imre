//! Names supplied by the boxed runtime without a filesystem module.

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod termios;

/// Loopback addresses defined by the public socket module.
pub const SOCKET_LOOPBACK_HOSTS: &[(&str, &str)] =
    &[("_LOCALHOST", "127.0.0.1"), ("_LOCALHOST_V6", "::1")];

/// Non-data methods installed on the exact module type. A module field shadows them.
pub const MODULE_METHODS: &[&str] = &[
    "__new__",
    "__repr__",
    "__getattribute__",
    "__init__",
    "__dir__",
];

/// Functions installed in the boxed builtins namespace.
pub const BUILTIN_FUNCTIONS: &[&str] = &[
    "len",
    "hash",
    "reversed",
    "dir",
    "sorted",
    "min",
    "max",
    "ord",
    "chr",
    "bin",
    "hex",
    "oct",
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
    "locals",
    "vars",
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
    "ascii",
    "format",
    "compile",
    "exec",
    "eval",
    "iter",
    "next",
    "anext",
];

/// Class objects exported directly by the boxed builtins namespace.
pub const BUILTIN_TYPES: &[&str] = &[
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

pub enum Provider {
    Builtin,
    #[cfg(unix)]
    Ssl,
    Hashlib,
}

/// These frozen registrations can yield to sys.path when frozen modules are disabled.
pub fn is_optional_frozen(name: &str) -> bool {
    matches!(name, "__hello__" | "runpy")
        || (name == "stat" && cfg!(any(target_os = "linux", target_os = "macos")))
}

/// Bootstrap modules remain frozen irrespective of the optional frozen-module setting.
pub fn is_frozen(name: &str, optional: impl FnOnce() -> bool) -> bool {
    matches!(name, "_frozen_importlib" | "_frozen_importlib_external")
        || (cfg!(unix) && name == "zipimport")
        || (is_optional_frozen(name) && optional())
}

// Generate the hot path string match and its enumerable names from one declaration.
macro_rules! provider_groups {
    ($(#[$doc:meta])* $vis:vis fn $matches:ident, $groups:ident; $( $(#[$condition:meta])* ($($name:literal)|+) ),+ $(,)?) => {
        const $groups: &[&[&str]] = &[$( $(#[$condition])* &[$($name),+] ),+];

        $(#[$doc])*
        $vis fn $matches(name: &str) -> bool {
            match name {
                $( $(#[$condition])* $($name)|+ => true, )+
                _ => false,
            }
        }
    };
}

provider_groups! {
    fn is_eager, EAGER;
    (
        "_blake2"
            | "_frozen_importlib"
            | "_frozen_importlib_external"
            | "_functools"
            | "_json"
            | "_lzma"
            | "_socket"
            | "abc"
            | "array"
            | "ast"
            | "contextlib"
            | "email"
            | "importlib.machinery"
            | "importlib.util"
            | "locale"
            | "urllib"
            | "_md5"
            | "_sha3"
            | "_datetime"
            | "_imre_fractions"
            | "_imre_itertools"
            | "_imre_ordered_dict"
            | "operator"
            | "_operator"
            | "_opcode"
            | "_pickle"
            | "atexit"
            | "builtins"
            | "errno"
            | "faulthandler"
            | "genericpath"
            | "hmac"
            | "urllib.parse"
            | "re._constants"
            | "io"
            | "marshal"
            | "itertools"
            | "json"
            | "subprocess"
            | "threading"
            | "warnings"
            | "_tracemalloc"
            | "_warnings"
    ),
    #[cfg(unix)] ("_signal" | "_imre_signal" | "zipimport"),
    #[cfg(any(target_os = "linux", target_os = "macos"))] ("grp" | "pwd"),
}

provider_groups! {
    /// These providers synthesize attributes on lookup. Their dictionaries alone
    /// do not enumerate their exports.
    pub fn has_lazy_attributes, LAZY;
    (
        "sys"
            | "_imp"
            | "_imre_thread"
            | "_thread"
            | "_typing"
            | "_collections"
            | "codecs"
            | "_codecs"
            | "types"
            | "_io"
            | "_weakref"
            | "_locale"
            | "traceback"
            | "sysconfig"
            | "os"
            | "posix"
            | "posixpath"
            | "_imre_posixpath"
            | "_imre_metadata"
            | "_imre_pyexpat"
            | "_imre_json"
            | "_imre_gzip"
            | "_imre_linecache"
            | "_imre_unittest"
            | "_imre_base32"
            | "_imre_base85"
            | "_imre_csv"
            | "_imre_tokenize"
            | "_imre_pkgutil"
            | "fnmatch"
            | "platform"
            | "site"
            | "select"
            | "selectors"
            | "struct"
            | "base64"
            | "binascii"
            | "math"
            | "unicodedata"
            | "_imre_subprocess"
            | "_imre_dataclasses"
            | "gc"
            | "pickle"
            | "zlib"
            | "email.utils"
            | "tempfile"
            | "_imre_tempfile"
            | "time"
            | "random"
            | "secrets"
            | "hashlib"
            | "re"
            | "importlib"
            | "_imre_importlib"
            | "functools"
            | "_string"
            | "inspect"
            | "collections"
            | "_collections_abc"
            | "enum"
            | "typing"
            | "contextvars"
            | "_contextvars"
            | "socket"
    ),
    #[cfg(unix)] ("_imre_zipimport"),
    #[cfg(all(unix, not(target_os = "linux")))] ("_signal"),
    #[cfg(target_os = "linux")] ("resource" | "fcntl"),
}

#[cfg(unix)]
const SSL: &[&str] = &["_ssl", "_imre_ssl"];
const HASHLIB: &str = "_hashlib";

/// Candidates for the built-in finder, before applying the executable's source
/// registrations and checking optional native libraries on this host.
pub fn builtin_module_candidates() -> impl Iterator<Item = &'static str> {
    let groups = EAGER.iter().chain(LAZY).copied();
    #[cfg(unix)]
    let groups = groups.chain([SSL]);
    groups.flatten().copied().chain([HASHLIB])
}

/// Optional native libraries are resolved by the runtime on the execution host.
pub fn provider(name: &str) -> Option<Provider> {
    #[cfg(unix)]
    if SSL.contains(&name) {
        return Some(Provider::Ssl);
    }
    if name == HASHLIB {
        return Some(Provider::Hashlib);
    }
    (is_eager(name) || is_native_fallback(name) || has_lazy_attributes(name))
        .then_some(Provider::Builtin)
}

/// Native packages have a package path and are resolved outside the built-in finder.
pub fn is_native_package(name: &str) -> bool {
    matches!(
        name,
        "collections" | "email" | "importlib" | "json" | "re" | "urllib"
    )
}

/// Identify the built-in provider before probing optional libraries on the execution host.
pub fn builtin_finder_provider(
    name: &str,
    is_frozen: impl FnOnce() -> bool,
    has_source: impl FnOnce() -> bool,
) -> Option<Provider> {
    let provider = provider(name)?;
    (!is_native_fallback(name) && !is_frozen() && !is_native_package(name) && !has_source())
        .then_some(provider)
}

/// Native providers that are searched after the application path.
pub fn is_native_fallback(name: &str) -> bool {
    matches!(name, "_bz2" | "_zstd" | "inspect" | "typing")
        || cfg!(target_os = "linux") && name == "mmap"
        || cfg!(any(target_os = "linux", target_os = "macos")) && name == "termios"
}
