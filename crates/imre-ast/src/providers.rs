//! Names supplied by the boxed runtime without a filesystem module.
pub enum Provider {
    Builtin,
    #[cfg(unix)]
    Ssl,
    Hashlib,
}

/// Optional native libraries are resolved by the runtime on the execution host.
pub fn provider(name: &str) -> Option<Provider> {
    #[cfg(unix)]
    if matches!(name, "_ssl" | "_imre_ssl") {
        return Some(Provider::Ssl);
    }
    if name == "_hashlib" {
        return Some(Provider::Hashlib);
    }
    is_builtin(name).then_some(Provider::Builtin)
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "_blake2"
            | "_frozen_importlib"
            | "_frozen_importlib_external"
            | "_json"
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
            | "itertools"
            | "json"
            | "subprocess"
            | "warnings"
            | "_tracemalloc"
            | "_warnings"
    ) || (cfg!(unix) && matches!(name, "_signal" | "_imre_signal" | "zipimport"))
        || (cfg!(any(target_os = "linux", target_os = "macos")) && matches!(name, "grp" | "pwd"))
        || has_lazy_attributes(name)
}

/// These providers synthesize attributes on lookup. Their module dictionaries
/// alone do not enumerate their exports.
pub fn has_lazy_attributes(name: &str) -> bool {
    if cfg!(unix) && name == "_imre_zipimport"
        || cfg!(all(unix, not(target_os = "linux"))) && name == "_signal"
        || cfg!(target_os = "linux") && matches!(name, "resource" | "fcntl")
    {
        return true;
    }
    matches!(
        name,
        "sys"
            | "_imp"
            | "_imre_thread"
            | "threading"
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
            | "_imre_json"
            | "_imre_linecache"
            | "_imre_unittest"
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
            | "marshal"
            | "pickle"
            | "lzma"
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
    )
}
