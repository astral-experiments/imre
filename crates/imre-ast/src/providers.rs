//! Names supplied by the boxed runtime without a filesystem module.

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod termios;

/// Loopback addresses defined by the public socket module.
pub const SOCKET_LOOPBACK_HOSTS: &[(&str, &str)] =
    &[("_LOCALHOST", "127.0.0.1"), ("_LOCALHOST_V6", "::1")];

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

pub enum Provider {
    Builtin,
    #[cfg(unix)]
    Ssl,
    Hashlib,
}

// Keep lookup and enumeration on the same declarations.
macro_rules! module_names {
    ($names:ident, $contains:ident, [$($(#[$cfg:meta])* $name:literal),* $(,)?]) => {
        const $names: &[&str] = &[$($(#[$cfg])* $name),*];

        fn $contains(name: &str) -> bool {
            match name {
                $($(#[$cfg])* $name => true,)*
                _ => false,
            }
        }
    };
}

module_names!(
    EAGER,
    is_eager,
    [
        "_blake2",
        "_frozen_importlib",
        "_frozen_importlib_external",
        "_json",
        "_lzma",
        "_socket",
        "abc",
        "array",
        "ast",
        "contextlib",
        "email",
        "importlib.machinery",
        "importlib.util",
        "locale",
        "urllib",
        "_md5",
        "_sha3",
        "_datetime",
        "_imre_fractions",
        "_imre_itertools",
        "_imre_ordered_dict",
        "operator",
        "_operator",
        "_opcode",
        "_pickle",
        "atexit",
        "builtins",
        "errno",
        "faulthandler",
        "genericpath",
        "hmac",
        "urllib.parse",
        "re._constants",
        "io",
        "itertools",
        "json",
        "subprocess",
        "threading",
        "warnings",
        "_tracemalloc",
        "_warnings",
        #[cfg(unix)]
        "_signal",
        #[cfg(unix)]
        "_imre_signal",
        #[cfg(unix)]
        "zipimport",
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        "grp",
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        "pwd",
    ]
);

module_names!(
    LAZY,
    is_lazy,
    [
        "sys",
        "_imp",
        "_imre_thread",
        "_thread",
        "_typing",
        "_collections",
        "codecs",
        "_codecs",
        "types",
        "_io",
        "_weakref",
        "_locale",
        "traceback",
        "sysconfig",
        "os",
        "posix",
        "posixpath",
        "_imre_posixpath",
        "_imre_metadata",
        "_imre_pyexpat",
        "_imre_json",
        "_imre_gzip",
        "_imre_linecache",
        "_imre_unittest",
        "_imre_base32",
        "_imre_base85",
        "_imre_csv",
        "_imre_tokenize",
        "_imre_pkgutil",
        "fnmatch",
        "platform",
        "site",
        "select",
        "selectors",
        "struct",
        "base64",
        "binascii",
        "math",
        "unicodedata",
        "_imre_subprocess",
        "_imre_dataclasses",
        "gc",
        "marshal",
        "pickle",
        "zlib",
        "email.utils",
        "tempfile",
        "_imre_tempfile",
        "time",
        "random",
        "secrets",
        "hashlib",
        "re",
        "importlib",
        "_imre_importlib",
        "functools",
        "_string",
        "inspect",
        "collections",
        "_collections_abc",
        "enum",
        "typing",
        "contextvars",
        "_contextvars",
        "socket",
        #[cfg(unix)]
        "_imre_zipimport",
        #[cfg(all(unix, not(target_os = "linux")))]
        "_signal",
        #[cfg(target_os = "linux")]
        "resource",
        #[cfg(target_os = "linux")]
        "fcntl",
    ]
);

module_names!(
    NATIVE_FALLBACK,
    is_fallback,
    [
        "_bz2",
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        "termios",
    ]
);

#[cfg(unix)]
module_names!(SSL, is_ssl, ["_ssl", "_imre_ssl"]);
module_names!(HASHLIB, is_hashlib, ["_hashlib"]);

/// Names registered as providers before checking optional host libraries or import origins.
pub fn module_provider_names() -> impl Iterator<Item = &'static str> {
    [
        EAGER,
        LAZY,
        NATIVE_FALLBACK,
        #[cfg(unix)]
        SSL,
        HASHLIB,
    ]
    .into_iter()
    .flatten()
    .copied()
}

/// Optional native libraries are resolved by the runtime on the execution host.
pub fn provider(name: &str) -> Option<Provider> {
    #[cfg(unix)]
    if is_ssl(name) {
        return Some(Provider::Ssl);
    }
    if is_hashlib(name) {
        return Some(Provider::Hashlib);
    }
    (is_eager(name) || is_native_fallback(name) || has_lazy_attributes(name))
        .then_some(Provider::Builtin)
}

/// Native extension modules that are searched after the application path.
pub fn is_native_fallback(name: &str) -> bool {
    is_fallback(name)
}

/// These providers synthesize attributes on lookup. Their module dictionaries
/// alone do not enumerate their exports.
pub fn has_lazy_attributes(name: &str) -> bool {
    is_lazy(name)
}
