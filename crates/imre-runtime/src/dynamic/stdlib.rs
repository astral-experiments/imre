//! Python system modules available to native functions with runtime-selected imports.

use std::path::{Path, PathBuf};

use super::{
    Context, builtins,
    value::{Error, FALSE, Id, NONE, Result, TRUE, Value},
};
use crate::objects::interpreter_stdlib as system;

pub(super) mod ast;
mod binascii;
pub(super) mod codecs;
mod contextvars;
pub(super) mod datetime;
mod email;
mod files;
mod fnmatch;
mod formatters;
mod functools;
mod hashlib;
mod hmac;
mod inspect;
pub(super) mod itertools;
pub(super) mod tempfile;
mod time;
pub(super) use files::{BinaryFile, ReadSource};
pub(super) mod io;
mod json;
mod logging;
mod marshal;
mod math;
mod metadata;
mod operator;
mod ordered_dict;
mod os;
mod pickle;
mod pkgutil;
mod posixpath;
pub(super) mod readiness;
pub(super) mod regex;
mod secrets;
mod socket;
mod structure;
mod traceback;
pub(super) mod urllib;
mod zipimport;

pub(super) fn is_module(name: &str) -> bool {
    if name == "stat" {
        return cfg!(any(target_os = "linux", target_os = "macos"));
    }
    if cfg!(unix) && matches!(name, "zipimport" | "_imre_zipimport") {
        return true;
    }
    matches!(
        name,
        "sys"
            | "threading"
            | "_thread"
            | "abc"
            | "array"
            | "_collections"
            | "codecs"
            | "types"
            | "_io"
            | "_weakref"
            | "io"
            | "traceback"
            | "sysconfig"
            | "ast"
            | "os"
            | "posix"
            | "posixpath"
            | "_imre_posixpath"
            | "_imre_metadata"
            | "_imre_pkgutil"
            | "fnmatch"
            | "itertools"
            | "operator"
            | "platform"
            | "site"
            | "selectors"
            | "struct"
            | "base64"
            | "binascii"
            | "math"
            | "unicodedata"
            | "json"
            | "_imre_logging"
            | "_imre_dataclasses"
            | "marshal"
            | "pickle"
            | "zlib"
            | "datetime"
            | "email"
            | "email.utils"
            | "tempfile"
            | "time"
            | "random"
            | "secrets"
            | "hashlib"
            | "hmac"
            | "re"
            | "importlib"
            | "importlib.machinery"
            | "importlib.util"
            | "_frozen_importlib"
            | "_frozen_importlib_external"
            | "_imre_importlib"
            | "functools"
            | "_string"
            | "inspect"
            | "collections"
            | "_collections_abc"
            | "enum"
            | "errno"
            | "typing"
            | "contextlib"
            | "contextvars"
            | "warnings"
            | "subprocess"
            | "socket"
            | "urllib"
            | "urllib.parse"
    )
}

fn os_error(err: std::io::Error) -> Error {
    crate::objects::errno_error(err).into()
}

fn argument(args: &[Id], index: usize) -> Result<Id> {
    args.get(index)
        .copied()
        .ok_or_else(|| Error::new("TypeError", "missing required argument"))
}

/// Bind the one positional-or-keyword parameter of a Python function.
fn single_argument(name: &str, param: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let mut value = args.first().copied();
    for (key, arg) in kwargs {
        if key != param {
            let mut message = format!("{name}() got an unexpected keyword argument '{key}'");
            if let Some(hint) = crate::objects::exceptions::keyword_suggestion(key, &[param]) {
                message.push_str(&format!(". Did you mean '{hint}'?"));
            }
            return Err(Error::new("TypeError", message));
        }
        if value.replace(*arg).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() got multiple values for argument '{param}'"),
            ));
        }
    }
    if args.len() > 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes 1 positional argument but {} were given",
                args.len()
            ),
        ));
    }
    value.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!("{name}() missing 1 required positional argument: '{param}'"),
        )
    })
}

fn option(args: &[Id], index: usize, kwargs: &[(String, Id)], name: &str) -> Result<Option<Id>> {
    let by_name = kwargs
        .iter()
        .find(|(key, _)| key == name)
        .map(|&(_, id)| id);
    match (args.get(index).copied(), by_name) {
        (Some(_), Some(_)) => Err(Error::new(
            "TypeError",
            format!("multiple values for argument '{name}'"),
        )),
        (left, right) => Ok(left.or(right)),
    }
}

fn record(ctx: &mut Context, fields: &[(&str, Id)]) -> Id {
    ctx.alloc(Value::NamedTuple {
        class: None,
        extras: Vec::new(),
        names: fields.iter().map(|(name, _)| (*name).to_owned()).collect(),
        values: fields.iter().map(|&(_, value)| value).collect(),
    })
}

fn instance(ctx: &mut Context, type_name: &str, fields: &[(&str, Id)]) -> Id {
    let class = ctx.alloc(Value::Native(type_name.to_owned()));
    ctx.alloc(Value::Instance {
        namespace: None,
        class,
        dictionary: None,
        scalar: None,
        property: None,
        weak_reference: None,
        memory_stream: None,
        fields: fields
            .iter()
            .map(|&(key, value)| (key.to_owned(), value))
            .collect(),
    })
}

fn tuple_text(ctx: &mut Context, values: &[&str]) -> Id {
    let values = values.iter().map(|&text| ctx.text(text)).collect();
    ctx.alloc(Value::Tuple(values))
}

fn language_version(ctx: &Context) -> Result<(u8, u8)> {
    let (major, minor) = ctx.config["python_version"]
        .as_str()
        .and_then(|v| v.split_once('.'))
        .ok_or_else(|| Error::new("RuntimeError", "missing Imre language version"))?;
    let parse = |v: &str| {
        v.parse()
            .map_err(|_| Error::new("RuntimeError", "invalid Imre language version"))
    };
    Ok((parse(major)?, parse(minor)?))
}

fn layout(ctx: &Context) -> Result<system::InterpreterLayout> {
    system::InterpreterLayout::new(&ctx.executable, language_version(ctx)?).map_err(os_error)
}

/// Resolve a native package's search directory relative to this executable's installation.
pub(super) fn package_path(ctx: &Context, name: &str) -> Result<PathBuf> {
    let stdlib = layout(ctx)?
        .path("posix_prefix", "stdlib", true)
        .expect("the default scheme defines the standard library path");
    Ok(Path::new(&stdlib).join(name))
}

fn full_version(ctx: &Context) -> Result<String> {
    let (major, minor) = language_version(ctx)?;
    Ok(format!("{major}.{minor}.0"))
}

fn version_info(ctx: &mut Context, version: &str) -> Result<Id> {
    let components: Vec<i64> = version
        .split('.')
        .map(|item| {
            item.parse()
                .map_err(|_| Error::new("RuntimeError", format!("invalid Imre version {version}")))
        })
        .collect::<Result<_>>()?;
    let [major, minor, micro] = components.as_slice() else {
        return Err(Error::new(
            "RuntimeError",
            format!("invalid Imre version {version}"),
        ));
    };
    let major = ctx.int(*major);
    let minor = ctx.int(*minor);
    let micro = ctx.int(*micro);
    let level = ctx.text("final");
    let serial = ctx.int(0);
    Ok(record(
        ctx,
        &[
            ("major", major),
            ("minor", minor),
            ("micro", micro),
            ("releaselevel", level),
            ("serial", serial),
        ],
    ))
}

fn compiler_version(ctx: &Context) -> String {
    ctx.config["implementation_version"]
        .as_str()
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_owned()
}

fn path_text(ctx: &mut Context, path: &Path) -> Result<Id> {
    path.to_str()
        .map(|value| ctx.text(value))
        .ok_or_else(|| Error::new("UnicodeError", "filesystem path contains surrogate escapes"))
}

pub(super) fn attribute(ctx: &mut Context, module: &str, name: &str) -> Result<Id> {
    if module == "_imre_logging" && logging::is_function(name) {
        return Ok(ctx.builtin(&format!("{module}.{name}")));
    }
    if matches!((module, name), ("importlib", "machinery") | ("os", "path")) {
        return ctx.module(&format!("{module}.{name}"));
    }
    let id = match (module, name) {
        ("math", "trunc" | "floor" | "ceil") => ctx.builtin(&format!("math.{name}")),
        ("enum", "auto") => ctx.enum_auto_class()?,
        ("enum", "Enum" | "IntEnum" | "IntFlag" | "StrEnum") => {
            ctx.builtin("enum.Enum.__copy__");
            ctx.builtin("enum.Enum.__deepcopy__");
            ctx.builtin(&format!("enum.{name}"))
        }
        ("tempfile", "TemporaryFile") => ctx.builtin("tempfile.TemporaryFile"),
        ("unicodedata", "normalize") => ctx.builtin("unicodedata.normalize"),
        ("_imre_importlib", "find_spec" | "get_code" | "loader_fields")
        | ("_imre_dataclasses", "loaded_module" | "members" | "metadata") => {
            ctx.builtin(&format!("{module}.{name}"))
        }
        ("threading", "Lock") | ("_thread", "LockType") => ctx.builtin(super::thread_lock::CLASS),
        ("_weakref", name) if let Some(value) = super::weakref::export(ctx, name) => value,
        ("threading", "local") | ("_thread", "_local") => ctx.builtin(super::thread_local::CLASS),
        ("re", name) if regex::is_flag_attribute(name) => regex::flag_attribute(ctx, name)?,
        ("traceback", "format_exc" | "format_exception" | "print_exc" | "print_exception") => {
            ctx.builtin(&format!("traceback.{name}"))
        }
        ("types", "ModuleType") => ctx.builtin("module"),
        ("types", "TracebackType") => ctx.builtin("traceback"),
        ("types", "MethodType") => ctx.builtin("method"),
        ("types", "FrameType") => ctx.builtin("frame"),
        ("types", "FunctionType") => ctx.builtin("function"),
        ("types", "BuiltinFunctionType") => ctx.builtin("builtin_function_or_method"),
        ("types", "EllipsisType") => ctx.builtin("ellipsis"),
        ("types", "NoneType" | "NotImplementedType") => ctx.builtin(name),
        ("types", "CodeType") => ctx.builtin("code"),
        ("itertools", "count") => ctx.builtin("itertools.count"),
        ("itertools", "repeat") => ctx.builtin("itertools.repeat"),
        ("itertools", "_chain_allocate") => ctx.builtin("itertools._chain_allocate"),
        ("itertools", "_chain_get" | "_chain_set") => ctx.builtin(&format!("itertools.{name}")),
        (
            "_collections",
            "_count_elements"
            | "_ordered_dict_equal"
            | "_ordered_dict_move_to_end"
            | "_ordered_dict_popitem"
            | "_ordered_dict_repr"
            | "_ordered_dict_view",
        ) => ctx.builtin(&format!("_collections.{name}")),
        ("collections" | "_collections", "_tuplegetter") => ctx.builtin(super::namedtuple::GETTER),
        ("collections", "defaultdict") => ctx.builtin("collections.defaultdict"),
        ("types", "GenericAlias") => ctx.builtin("types.GenericAlias"),
        ("typing", "TYPE_CHECKING") => FALSE,
        ("typing", "Union") | ("types", "UnionType") => super::union::class(ctx),
        ("typing", "Generic" | "Protocol") => super::typing::named_class(ctx, "typing", name),
        (
            "_collections_abc",
            abc
            @ ("Sized" | "Iterable" | "Container" | "Collection" | "Mapping" | "MutableMapping"),
        ) => super::mapping_abc::class(ctx, abc)?,
        ("_collections_abc", "_check_methods") => ctx.builtin("_collections_abc._check_methods"),
        ("_collections_abc", "Callable") => {
            super::typing::named_class(ctx, "collections.abc", "Callable")
        }
        (
            "typing",
            "TypeVar"
            | "Any"
            | "Callable"
            | "cast"
            | "overload"
            | "get_overloads"
            | "clear_overloads"
            | "_overload_dummy"
            | "get_args"
            | "get_origin"
            | "_GenericAlias"
            | "_LiteralGenericAlias",
        ) => ctx.builtin(&format!("typing.{name}")),
        ("inspect", "_is_coroutine_mark") => inspect::marker(ctx),
        ("datetime", "UTC") => datetime::utc(ctx),
        ("time", "time" | "gmtime" | "localtime" | "mktime" | "struct_time") => {
            ctx.builtin(&format!("time.{name}"))
        }
        ("datetime", "datetime" | "date" | "time" | "timedelta" | "timezone") => {
            ctx.builtin(&format!("datetime.{name}"))
        }
        ("socket", name) => return socket::attribute(ctx, name),
        ("errno", "__doc__") => ctx.text(crate::objects::ERRNO_DOC),
        ("errno", "errorcode") => {
            let dict = ctx.dict(Vec::new());
            for (symbol, value) in crate::objects::errno_entries() {
                let key = ctx.int(i64::from(value));
                let symbol = ctx.text(symbol);
                ctx.put_item(dict, key, symbol)?;
            }
            dict
        }
        ("errno", name) => {
            let value = crate::objects::errno_entries()
                .find_map(|(symbol, value)| (symbol == name).then_some(value))
                .ok_or_else(|| {
                    Error::new(
                        "AttributeError",
                        format!("module 'errno' has no attribute '{name}'"),
                    )
                })?;
            ctx.int(i64::from(value))
        }
        ("secrets", "choice") => ctx.builtin("secrets.choice"),
        ("selectors", "EVENT_READ") => ctx.int(1),
        ("selectors", "EVENT_WRITE") => ctx.int(2),
        ("sys", "argv") => ctx.argv,
        ("zlib", "adler32") => ctx.builtin("zlib.adler32"),
        ("binascii", "Error" | "Incomplete" | "a2b_base64" | "b2a_base64") => {
            ctx.builtin(&format!("binascii.{name}"))
        }
        ("json", "JSONDecodeError") => ctx.alloc(Value::Native("JSONDecodeError".to_owned())),
        ("sys", "path") => ctx.sys_path,
        ("sys", "prefix" | "base_prefix" | "exec_prefix" | "base_exec_prefix") => {
            path_text(ctx, &layout(ctx)?.prefix)?
        }
        ("sys", "executable" | "_base_executable") => path_text(ctx, &ctx.executable.clone())?,
        ("sys", "maxsize") => ctx.int(isize::MAX as i64),
        ("sys", "platform") => ctx.text(if cfg!(target_os = "macos") {
            "darwin"
        } else {
            std::env::consts::OS
        }),
        ("sys", "version") => ctx.text(format!(
            "{} (Imre {})",
            full_version(ctx)?,
            compiler_version(ctx)
        )),
        ("sys", "version_info") => version_info(ctx, &full_version(ctx)?)?,
        ("sys", "implementation") => {
            let name = ctx.text("imre");
            let version = version_info(ctx, &compiler_version(ctx))?;
            instance(
                ctx,
                "types.SimpleNamespace",
                &[("name", name), ("version", version), ("cache_tag", NONE)],
            )
        }
        ("sys", "dont_write_bytecode") => {
            if ctx.config["dont_write_bytecode"].as_bool().unwrap_or(false) {
                TRUE
            } else {
                FALSE
            }
        }
        ("sys", "flags") => {
            let isolated_flag = ctx.config["isolated"].as_bool().unwrap_or(false);
            let isolated = ctx.int(i64::from(isolated_flag));
            let no_user_site = ctx.int(i64::from(
                ctx.config["no_user_site"].as_bool().unwrap_or(false),
            ));
            let safe_path = if ctx.config["safe_path"].as_bool().unwrap_or(false) {
                TRUE
            } else {
                FALSE
            };
            let bytecode = ctx.int(i64::from(
                ctx.config["dont_write_bytecode"].as_bool().unwrap_or(false),
            ));
            record(
                ctx,
                &[
                    ("isolated", isolated),
                    ("ignore_environment", isolated),
                    ("no_user_site", no_user_site),
                    ("safe_path", safe_path),
                    ("dont_write_bytecode", bytecode),
                ],
            )
        }
        ("functools", "WRAPPER_ASSIGNMENTS") => tuple_text(ctx, functools::ASSIGNED),
        ("functools", "WRAPPER_UPDATES") => tuple_text(ctx, functools::UPDATED),
        ("posixpath", "join" | "abspath" | "dirname") => ctx.builtin(&format!("os.path.{name}")),
        ("os" | "posix", "fspath") => ctx.builtin("posix.fspath"),
        ("posixpath", "normpath") | ("posix", "_path_normpath") => {
            ctx.builtin("posix._path_normpath")
        }
        ("posix", "environ") => os::environment(ctx),
        ("posix", "putenv" | "unsetenv") => ctx.builtin(&format!("posix.{name}")),
        ("os", "name") => ctx.text("posix"),
        ("posixpath", "sep") => ctx.text("/"),
        ("posixpath", "altsep") => NONE,
        ("importlib.machinery", "EXTENSION_SUFFIXES") => {
            let suffix = ctx.text(
                ctx.config["archive_suffix"]
                    .as_str()
                    .unwrap_or(".imre.a")
                    .to_owned(),
            );
            ctx.alloc(Value::List(vec![suffix]))
        }
        ("operator", name)
            if matches!(name, "attrgetter" | "itemgetter")
                || operator::Operation::named(name).is_some() =>
        {
            ctx.builtin(&format!("operator.{name}"))
        }
        ("subprocess", "PIPE") => ctx.int(-1),
        ("functools", "Placeholder") => ctx.alloc(Value::PartialPlaceholder),
        (
            "sys",
            "exit" | "exc_info" | "exception" | "intern" | "_is_interned" | "getdefaultencoding",
        )
        | (
            "sysconfig",
            "get_path"
            | "get_paths"
            | "get_path_names"
            | "get_scheme_names"
            | "get_platform"
            | "get_config_var"
            | "get_default_scheme"
            | "get_preferred_scheme",
        )
        | ("site", "getsitepackages")
        | ("os", "confstr" | "uname")
        | ("_imre_pkgutil", "filenames" | "isdir" | "find_module" | "find_spec")
        | ("_imre_metadata", "filenames" | "dedent" | "exists" | "read_text")
        | ("_imre_posixpath", "exists" | "extension_start" | "isfile")
        | ("_imre_zipimport", "exists" | "index" | "read" | "contains")
        | ("platform", "system" | "node" | "release" | "version" | "machine" | "mac_ver")
        | ("struct", "Struct" | "calcsize" | "unpack" | "error")
        | ("base64", "urlsafe_b64encode" | "urlsafe_b64decode")
        | ("json", "dumps" | "loads")
        | ("random", "random")
        | ("codecs", "lookup" | "register_error" | "lookup_error")
        | ("marshal", "loads")
        | ("pickle", "dumps")
        | (
            "urllib.parse",
            "unquote" | "urlsplit" | "urlunsplit" | "SplitResult" | "SplitResultBytes" | "quote"
            | "quote_plus" | "quote_from_bytes" | "urlencode",
        )
        | ("hashlib", "sha1" | "sha256")
        | ("hmac", "new" | "HMAC" | "compare_digest")
        | ("re", "match" | "fullmatch" | "search" | "compile" | "split")
        | ("typing", "IO" | "NamedTuple" | "Generator" | "Iterator" | "Sequence")
        | ("enum", "_simple_enum")
        | ("functools", "lru_cache" | "partial" | "update_wrapper")
        | ("_string", "formatter_parser" | "formatter_field_name_split")
        | ("inspect", "iscoroutinefunction" | "markcoroutinefunction")
        | ("contextlib", "contextmanager")
        | ("email.utils", "parsedate_to_datetime" | "format_datetime" | "formatdate")
        | ("contextvars", "ContextVar" | "Context" | "Token" | "copy_context")
        | ("warnings", "warn")
        | ("fnmatch", "filter")
        | ("subprocess", "run") => ctx.alloc(Value::Native(format!("{module}.{name}"))),
        (
            "codecs",
            "strict_errors"
            | "ignore_errors"
            | "replace_errors"
            | "xmlcharrefreplace_errors"
            | "backslashreplace_errors"
            | "namereplace_errors",
        ) => ctx.builtin(&format!("codecs.{name}")),
        _ => {
            return Err(Error::new(
                "AttributeError",
                format!("module '{module}' has no attribute '{name}'"),
            ));
        }
    };
    Ok(id)
}

/// Count each key before pulling the next and preserve updates if a callback raises.
fn count_elements(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "_collections._count_elements() takes no keyword arguments",
        ));
    }
    let &[mapping, iterable] = args else {
        return Err(Error::new(
            "TypeError",
            format!("_count_elements expected 2 arguments, got {}", args.len()),
        ));
    };
    let iterator = ctx.iterator(iterable)?;
    let dict_get = ctx.builtin("dict.get");
    let dict_set = ctx.builtin("dict.__setitem__");
    // The builtin dict operations bypass instance attributes when neither type method is overridden.
    let receiver = ctx
        .dict_storage(mapping)
        .filter(|_| {
            ctx.slot(mapping, "get").is_none_or(|get| get == dict_get)
                && ctx
                    .slot(mapping, "__setitem__")
                    .is_none_or(|set| set == dict_set)
        })
        .unwrap_or(mapping);
    let get = ctx.lookup_attr(receiver, "get")?;
    let zero = ctx.int(0);
    let one = ctx.int(1);
    let roots = ctx.heap.temporary_mark();
    while let Some(key) = ctx.next_item(iterator)? {
        ctx.heap.keep(key);
        let previous = ctx.call(get, vec![key, zero], vec![])?;
        let count = ctx.binary("+", previous, one)?;
        ctx.put_item(receiver, key, count)?;
        ctx.heap.release_temporaries_since(roots);
    }
    Ok(NONE)
}

/// Invoke the text object's ASCII encoder, preserving user hooks and their exceptions.
fn base64_ascii(ctx: &mut Context, text: Id) -> Result<Id> {
    let result = ctx.lookup_attr(text, "encode").and_then(|encoder| {
        let ascii = ctx.text("ascii");
        ctx.call(encoder, vec![ascii], Vec::new())
    });
    result.map_err(|error| {
        let class = ctx.builtin("UnicodeEncodeError");
        if ctx.raised_error_matches(&error, class) {
            Error::new(
                "ValueError",
                "string argument should contain only ASCII characters",
            )
        } else {
            error
        }
    })
}

/// A nonstandard encoder may supply its own translation object before standard decoding.
fn urlsafe_input(ctx: &mut Context, text: Id) -> Result<(Id, bool)> {
    let encoded = base64_ascii(ctx, text)?;
    if matches!(
        ctx.heap.get(encoded),
        Value::Bytes(_) | Value::ByteArray(_) | Value::Str(_)
    ) {
        return Ok((encoded, true));
    }
    // The inherited str.translate with this table only remaps the URL alphabet.
    if let Some(text) = ctx.string_storage(encoded)
        && let Value::Instance { class, fields, .. } = ctx.heap.get(encoded)
        && !fields.contains_key("translate")
        && ctx.class_attr(*class, "translate") == ctx.builtins.get("str.translate").copied()
        && ctx.class_attr(*class, "__getattribute__").is_none()
    {
        return Ok((text, true));
    }
    let translate = ctx.lookup_attr(encoded, "translate")?;
    let mut table = Vec::new();
    table
        .try_reserve_exact(256)
        .map_err(|_| Error::new("MemoryError", ""))?;
    table.extend(0..=u8::MAX);
    table[b'-' as usize] = b'+';
    table[b'_' as usize] = b'/';
    let table = ctx.alloc(Value::Bytes(table));
    let translated = ctx.call(translate, vec![table], Vec::new())?;
    let data = if ctx.string_storage(translated).is_some() {
        base64_ascii(ctx, translated)?
    } else {
        translated
    };
    Ok((data, false))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "collections.defaultdict.__repr__" {
        let Some((&object, args)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "descriptor '__repr__' of 'collections.defaultdict' object needs an argument",
            ));
        };
        let Some(storage) = ctx
            .dict_storage(object)
            .filter(|&storage| matches!(ctx.heap.get(storage), Value::DefaultDict { .. }))
        else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '__repr__' requires a 'collections.defaultdict' object but received a '{}'",
                    ctx.type_name(object)
                ),
            ));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "wrapper __repr__() takes no keyword arguments",
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("expected 0 arguments, got {}", args.len()),
            ));
        }
        let text = ctx.checked_defaultdict_slot_repr(object, storage)?;
        return Ok(ctx.text(text));
    }
    if matches!(
        name,
        "collections.defaultdict" | "collections.defaultdict.__init__"
    ) {
        let (receiver, args) = if name == "collections.defaultdict.__init__" {
            let Some((&object, args)) = args.split_first() else {
                return Err(Error::new(
                    "TypeError",
                    "descriptor '__init__' of 'collections.defaultdict' object needs an argument",
                ));
            };
            if ctx.defaultdict_factory(object).is_none() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "descriptor '__init__' requires a 'collections.defaultdict' object but received a '{}'",
                        ctx.type_name(object)
                    ),
                ));
            }
            (Some(object), args)
        } else {
            (None, args)
        };
        let factory = args.first().copied().unwrap_or(NONE);
        if factory != NONE && !ctx.callable(factory) {
            return Err(Error::new(
                "TypeError",
                "first argument must be callable or None",
            ));
        }
        let dict = if let Some(object) = receiver {
            ctx.replace_defaultdict_factory(object, factory);
            ctx.dict_storage(object)
                .expect("validated defaultdict storage")
        } else {
            ctx.alloc(Value::DefaultDict {
                factory,
                entries: Default::default(),
            })
        };
        if args.len() > 2 {
            return Err(Error::new(
                "TypeError",
                format!("dict expected at most 1 argument, got {}", args.len() - 1),
            ));
        }
        builtins::update_dict(ctx, dict, args.get(1).copied(), kwargs)?;
        return Ok(if receiver.is_some() { NONE } else { dict });
    }
    if matches!(name, "sys.intern" | "sys._is_interned") {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no keyword arguments"),
            ));
        }
        let [value] = args else {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes exactly one argument ({} given)", args.len()),
            ));
        };
        let operation = name.trim_start_matches("sys.");
        let query = operation == "_is_interned";
        let Some(storage) = ctx.string_storage(*value) else {
            let class = if *value == NONE {
                "None"
            } else {
                ctx.type_name(*value)
            };
            return Err(Error::new(
                "TypeError",
                format!("{operation}() argument must be str, not {class}"),
            ));
        };
        if query {
            return Ok(super::builtins::boolean(
                storage == *value && ctx.heap.is_interned(*value),
            ));
        }
        if storage != *value {
            return Err(Error::new(
                "TypeError",
                format!("can't intern {}", ctx.type_name(*value)),
            ));
        }
        let (heap, roots) = ctx.heap_and_roots();
        return Ok(heap.intern(*value, roots));
    }
    if name == "sys.getdefaultencoding" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "sys.getdefaultencoding() takes no keyword arguments",
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "sys.getdefaultencoding() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        return Ok(ctx.default_encoding);
    }
    if name == "random.random" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "Random.random() takes no keyword arguments",
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("Random.random() takes no arguments ({} given)", args.len()),
            ));
        }
        return Ok(ctx.alloc(Value::Float(crate::objects::imre_random_float())));
    }
    if name == "secrets.choice" {
        return secrets::choice(ctx, args, kwargs);
    }
    if name == "_collections_abc._check_methods" {
        if let Some((key, _)) = kwargs.iter().find(|(key, _)| key != "C") {
            return Err(Error::new(
                "TypeError",
                format!("_check_methods() got an unexpected keyword argument '{key}'"),
            ));
        }
        let class = option(args, 0, kwargs, "C")?.ok_or_else(|| {
            Error::new(
                "TypeError",
                "_check_methods() missing required argument: 'C'",
            )
        })?;
        return ctx.abc_check_methods(class, args.get(1..).unwrap_or_default());
    }
    if name == super::thread_lock::CLASS {
        return super::thread_lock::construct(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_thread._local.") {
        return super::thread_local::method(ctx, method, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_thread.lock.") {
        return super::thread_lock::method(ctx, method, args, kwargs);
    }
    if name.starts_with("_weakref.") || name.starts_with("weakref.") {
        return super::weakref::call(ctx, name, args, kwargs);
    }
    if name == super::thread_local::CLASS {
        if !args.is_empty() || !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "Initialization arguments are not supported",
            ));
        }
        return Ok(ctx.alloc(Value::ThreadLocal(Default::default())));
    }
    if name == "struct.Struct" {
        return structure::new(ctx, args, kwargs);
    }
    if name == "unicodedata.normalize" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "unicodedata.normalize() takes no keyword arguments",
            ));
        }
        let [form, text] = args else {
            return Err(Error::new(
                "TypeError",
                format!("normalize expected 2 arguments, got {}", args.len()),
            ));
        };
        let string = |value, position| {
            ctx.str_value(value).map_err(|_| {
                let kind = if value == NONE {
                    "None"
                } else {
                    ctx.type_name(value)
                };
                Error::new(
                    "TypeError",
                    format!("normalize() argument {position} must be str, not {kind}"),
                )
            })
        };
        return Ok(
            match crate::objects::normalize_unicode(string(*form, 1)?, string(*text, 2)?)? {
                Some(normalized) => ctx.text(normalized.into_string()),
                None => ctx.scalar_value(*text),
            },
        );
    }
    if let Some(operation) = name.strip_prefix("math.") {
        return math::round(ctx, operation, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("binascii.") {
        return binascii::call(ctx, method, args, kwargs);
    }
    if name == "zlib.adler32" {
        return binascii::adler32(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("struct.Struct.") {
        return structure::method(ctx, method, args, kwargs);
    }
    if let Some(op) = name.strip_prefix("_imre_zipimport.") {
        return zipimport::call(ctx, op, args, kwargs);
    }
    if matches!(name, "posix.fspath" | "posix._path_normpath")
        || name.starts_with("_imre_posixpath.")
    {
        return posixpath::call(ctx, name, args, kwargs);
    }
    if let Some(operation) = name.strip_prefix("_imre_metadata.") {
        return metadata::call(ctx, operation, args, kwargs);
    }
    if matches!(name, "posix.putenv" | "posix.unsetenv") {
        return os::change_environment(ctx, name, args, kwargs);
    }
    if let Some(operation) = name.strip_prefix("_imre_pkgutil.") {
        return pkgutil::call(ctx, operation, args, kwargs);
    }
    if name == "fnmatch.filter" {
        return fnmatch::filter(ctx, args, kwargs);
    }
    if name == "_collections._count_elements" {
        return count_elements(ctx, args, kwargs);
    }
    if let Some(operation) = name.strip_prefix("_collections._ordered_dict_") {
        return ordered_dict::call(ctx, operation, args, kwargs);
    }
    if name == "collections.namedtuple" {
        return super::namedtuple::factory(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("member_descriptor.") {
        return super::namedtuple::doc_method(ctx, method, args, kwargs);
    }
    if name == super::namedtuple::GETTER {
        return super::namedtuple::new_getter(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("collections._tuplegetter.") {
        return super::namedtuple::getter_method(ctx, method, args, kwargs);
    }
    if name == "_collections._make" {
        return super::namedtuple::make(ctx, args, kwargs);
    }
    if name == "itertools.count" {
        return itertools::count(ctx, args, kwargs);
    }
    if name == "itertools._chain_allocate" {
        let [base, class, source] = args else {
            return Err(Error::new(
                "TypeError",
                "invalid chain allocation arguments",
            ));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "chain allocation takes no keyword arguments",
            ));
        }
        let valid = matches!(ctx.heap.get(*class), Value::Class { ancestors, .. }
            if *class == *base || ancestors.contains(base));
        if !valid {
            return Err(Error::new("TypeError", "expected a chain subclass"));
        }
        let result = ctx.instance(*class, None);
        if let Value::Instance { fields, .. } = ctx.heap.get_mut(result) {
            fields.insert("_sources".to_owned(), *source);
            fields.insert("_active".to_owned(), NONE);
        }
        return Ok(result);
    }
    if name == "itertools._chain_get" || name == "itertools._chain_set" {
        if !kwargs.is_empty()
            || (name.ends_with("_get") && args.len() != 2)
            || (name.ends_with("_set") && args.len() != 3)
        {
            return Err(Error::new("TypeError", "invalid chain state arguments"));
        }
        let key = if args[1] == TRUE {
            "_active"
        } else {
            "_sources"
        };
        if let Value::Instance { fields, .. } = ctx.heap.get_mut(args[0]) {
            if name.ends_with("_set") {
                fields.insert(key.to_owned(), args[2]);
                return Ok(NONE);
            }
            if let Some(&value) = fields.get(key) {
                return Ok(value);
            }
        }
        return Err(Error::new("TypeError", "invalid chain state receiver"));
    }
    if name == super::deque::CLASS {
        return super::deque::construct(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("collections.deque.") {
        return super::deque::method(ctx, method, args, kwargs);
    }
    if name == "itertools.repeat" {
        return itertools::repeat(ctx, args, kwargs);
    }
    if let Some(method) = name
        .strip_prefix("itertools.count.")
        .or_else(|| name.strip_prefix("itertools.repeat."))
    {
        return itertools::method(ctx, method, args, kwargs);
    }
    if let Some(function) = name.strip_prefix("inspect.") {
        return inspect::call(ctx, function, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_imre_importlib.") {
        match (method, args, kwargs) {
            ("find_spec", [fullname, original], []) => {
                let fullname = ctx.str_value(*fullname)?.to_owned();
                let original = ctx.str_value(*original)?.to_owned();
                return ctx.find_spec(&fullname, &original, false);
            }
            ("get_code", [name, path], []) => return ctx.registered_code(*name, *path),
            ("loader_fields", [object], []) => {
                if let Value::Instance { fields, .. } = ctx.heap.get(*object) {
                    let fields = fields
                        .clone()
                        .into_iter()
                        .map(|(key, value)| (ctx.text(key), value))
                        .collect();
                    return Ok(ctx.dict(fields));
                }
                return Err(Error::new("TypeError", "expected a source loader"));
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("invalid arguments to {name}"),
                ));
            }
        }
    }
    if let Some(method) = name.strip_prefix("_imre_dataclasses.") {
        match (method, args, kwargs) {
            ("loaded_module", [name], []) => {
                return Ok(ctx
                    .modules
                    .get(ctx.str_value(*name)?)
                    .copied()
                    .unwrap_or(NONE));
            }
            ("members", [class], []) => {
                if let Value::Class { scope, .. } = *ctx.heap.get(*class) {
                    let names = ctx.scope_data(scope).names.clone();
                    let entries = names
                        .into_iter()
                        .map(|(name, value)| (ctx.text(name), value))
                        .collect();
                    return Ok(ctx.dict(entries));
                }
            }
            ("metadata", [dict], []) if ctx.dict_storage(*dict).is_some() => {
                return Ok(ctx.alloc(Value::MappingProxy(*dict)));
            }
            _ => {}
        }
        return Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        ));
    }
    if let Some(function) = name.strip_prefix("_imre_logging.") {
        return logging::call(ctx, function, args, kwargs);
    }
    if let Some(function) = name.strip_prefix("urllib.parse.") {
        return urllib::call(ctx, function, args, kwargs);
    }
    if let Some(name) = name.strip_prefix("ast.") {
        return ast::call(ctx, name, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("abc.") {
        return super::mapping_abc::call(ctx, method, args, kwargs);
    }
    if let Some(operation) = name.strip_prefix("typing.") {
        return super::typing::call(ctx, operation, args, kwargs);
    }
    if name == "types.GenericAlias" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "GenericAlias() takes no keyword arguments",
            ));
        }
        let [origin, parameters] = args else {
            return Err(Error::new(
                "TypeError",
                format!("GenericAlias expected 2 arguments, got {}", args.len()),
            ));
        };
        return Ok(super::typing::builtin_alias(ctx, *origin, *parameters));
    }
    if let Some(name) = name.strip_prefix("operator.") {
        if name == "attrgetter" {
            return operator::attrgetter(ctx, args, kwargs);
        }
        if name == "itemgetter" {
            return operator::itemgetter(ctx, args, kwargs);
        }
        if name == "itemgetter.__reduce__" {
            return operator::reduce_itemgetter(ctx, args, kwargs);
        }
        if let Some(op) = operator::Operation::named(name) {
            return op.call(ctx, name, args, kwargs);
        }
    }
    if let Some(method) = name.strip_prefix("selectors.") {
        return readiness::call(ctx, method, args, kwargs);
    }
    if matches!(name, "enum.auto.__init__" | "enum.auto.__repr__") {
        return ctx.auto_method(name, args, kwargs);
    }
    if name == "enum.Enum.__copy__" {
        let [member] = builtins::bind_builtin_arguments("__copy__", args, kwargs, ["self"], 1)?;
        return Ok(member.unwrap());
    }
    if name == "enum.Enum.__deepcopy__" {
        let [member, _] =
            builtins::bind_builtin_arguments("__deepcopy__", args, kwargs, ["self", "memo"], 2)?;
        return Ok(member.unwrap());
    }
    if name == "enum._simple_enum" {
        return ctx.enum_decorator(args, kwargs);
    }
    if name == "enum._convert" {
        return ctx.decorate_enum(args, kwargs);
    }
    // Reject unsupported options before a host primitive can silently use its default.
    if let Some(function) = name.strip_prefix("email.utils.") {
        return email::call(ctx, function, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("datetime.") {
        return datetime::call(ctx, method, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("time.") {
        return time::call(ctx, method, args, kwargs);
    }
    if let Some(function) = name.strip_prefix("codecs.") {
        return match function {
            "lookup" => codecs::lookup(ctx, args, kwargs),
            "register_error" | "lookup_error" => {
                codecs::error_registry(ctx, function, args, kwargs)
            }
            _ => codecs::builtin_handler(ctx, function, args, kwargs),
        };
    }
    if let Some(name) = name.strip_prefix("socket.") {
        return socket::call(ctx, name, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("contextvars.") {
        return contextvars::call(ctx, method, args, kwargs);
    }
    if name == "tempfile.TemporaryFile" {
        return tempfile::open(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("BufferedRandom.") {
        return tempfile::method(ctx, method, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("TextIOWrapper.") {
        return files::text_method(ctx, method, args, kwargs);
    }
    if name == "marshal.loads" {
        return marshal::loads(ctx, args, kwargs);
    }
    if name == "pickle.dumps" {
        return pickle::dumps(ctx, args, kwargs);
    }
    if name == "functools.partial" {
        let Some((&function, bound)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "type 'partial' takes at least one argument",
            ));
        };
        if !ctx.callable(function) {
            return Err(Error::new(
                "TypeError",
                "the first argument must be callable",
            ));
        }
        if bound
            .last()
            .is_some_and(|&id| matches!(ctx.heap.get(id), Value::PartialPlaceholder))
        {
            return Err(Error::new(
                "TypeError",
                "trailing Placeholders are not allowed",
            ));
        }
        let (function, mut args, keywords) = match ctx.heap.get(function).clone() {
            Value::Partial {
                function,
                args,
                keywords,
                ..
            } => {
                let Value::Dict(entries) = ctx.heap.get(keywords) else {
                    unreachable!()
                };
                let keywords = ctx.dict(entries.to_vec());
                (function, ctx.items(args)?, keywords)
            }
            _ => (function, Vec::new(), ctx.dict(Vec::new())),
        };
        let mut supplied = bound.iter().copied();
        for argument in &mut args {
            if matches!(ctx.heap.get(*argument), Value::PartialPlaceholder)
                && let Some(value) = supplied.next()
            {
                *argument = value;
            }
        }
        args.extend(supplied);
        let args = ctx.alloc(Value::Tuple(args));
        for (name, value) in kwargs {
            if matches!(ctx.heap.get(*value), Value::PartialPlaceholder) {
                return Err(Error::new(
                    "TypeError",
                    "Placeholder cannot be passed as a keyword argument",
                ));
            }
            let name = ctx.text(name);
            ctx.put_item(keywords, name, *value)?;
        }
        return Ok(ctx.alloc(Value::Partial {
            function,
            args,
            keywords,
            coroutine_marker: None,
        }));
    }
    if name == "pattern.search" {
        return regex::search(ctx, args, kwargs);
    }
    if name == "pattern.split" {
        return regex::pattern_split(ctx, args, kwargs);
    }
    if name.starts_with("re.") {
        return regex::module_call(ctx, name, args, kwargs);
    }
    let urlsafe = matches!(
        name,
        "base64.urlsafe_b64encode" | "base64.urlsafe_b64decode"
    );
    let (max_args, keywords): (usize, &[&str]) = match name {
        "functools.update_wrapper" => (4, &["wrapper", "wrapped", "assigned", "updated"]),
        "sysconfig.get_path" => (4, &["name", "scheme", "vars", "expand"]),
        "sysconfig.get_paths" => (3, &["scheme", "vars", "expand"]),
        "json.loads" => (1, &["s"]),
        "json.dumps" => (
            1,
            &["obj", "default", "ensure_ascii", "sort_keys", "separators"],
        ),
        "hashlib.sha1" | "hashlib.sha256" => (1, &["data", "usedforsecurity"]),
        "hmac.new" | "hmac.HMAC" => (3, &["key", "msg", "digestmod"]),
        "hmac.compare_digest" | "hmac.HMAC.update" => (2, &[]),
        "hmac.HMAC.digest" | "hmac.HMAC.hexdigest" | "hmac.HMAC.copy" => (1, &[]),
        "hash.update" => (2, &[]),
        "hash.copy" | "hash.digest" | "hash.hexdigest" => (1, &[]),
        "subprocess.run" => (1, &["text", "capture_output", "stdout", "stderr", "check"]),
        "file.seek" => (3, &["whence"]),
        "file.__exit__" => (4, &[]),
        "struct.unpack" | "pattern.match" | "pattern.fullmatch" | "pattern.findall"
        | "file.read" | "file.readline" | "file.readinto" | "file.write" | "warnings.warn" => {
            (2, &[])
        }
        "match.group" | "os.path.join" => (usize::MAX, &[]),
        "collections.defaultdict.__missing__" => (2, &[]),
        "match.groups" => (2, &["default"]),
        "sys.exc_info" | "sys.exception" => (0, &[]),
        "traceback.print_exc" => return traceback::print_active(ctx, args, kwargs),
        "traceback.format_exc" => return traceback::format_active(ctx, args, kwargs),
        "traceback.format_exception" => return traceback::format(ctx, args, kwargs),
        "traceback.print_exception" => (6, &["value", "tb", "limit", "file", "chain", "colorize"]),
        "sys.exit"
        | "sysconfig.get_config_var"
        | "sysconfig.get_preferred_scheme"
        | "os.confstr"
        | "os.path.abspath"
        | "os.path.dirname"
        | "struct.calcsize"
        | "_string.formatter_parser"
        | "_string.formatter_field_name_split"
        | "collections.defaultdict.copy"
        | "collections.defaultdict.__copy__"
        | "collections.defaultdict.__reduce__"
        | "functools.lru_cache"
        | "functools._cached"
        | "contextlib.contextmanager"
        | "file.close"
        | "file.readable"
        | "file.writable"
        | "file.seekable"
        | "file.isatty"
        | "file.flush"
        | "file.fileno"
        | "file.tell"
        | "file.__enter__" => (1, &[]),
        _ => (0, &[]),
    };
    if !urlsafe
        && (args.len() > max_args
            || kwargs
                .iter()
                .any(|(key, _)| !keywords.contains(&key.as_str())))
    {
        if name == "file.flush"
            && let Some((&receiver, positional)) = args.split_first()
        {
            let class = ctx.type_name(receiver).trim_start_matches("_io.");
            let message = if kwargs.is_empty() {
                format!(
                    "{class}.flush() takes no arguments ({} given)",
                    positional.len()
                )
            } else {
                format!("{class}.flush() takes no keyword arguments")
            };
            return Err(Error::new("TypeError", message));
        }
        return Err(Error::new(
            "TypeError",
            format!("unsupported arguments to {name}"),
        ));
    }
    let value = match name {
        "_string.formatter_parser" | "_string.formatter_field_name_split" => {
            return ctx.format_cursor(
                argument(args, 0)?,
                name == "_string.formatter_field_name_split",
            );
        }
        "traceback.print_exception" => return traceback::print(ctx, args, kwargs),
        _ if urlsafe => {
            let id = single_argument(name.trim_start_matches("base64."), "s", args, kwargs)?;
            let decode = name == "base64.urlsafe_b64decode";
            let (id, urlsafe) = if decode
                && matches!(ctx.heap.get(id), Value::Instance { .. })
                && ctx.string_storage(id).is_some()
            {
                urlsafe_input(ctx, id)?
            } else {
                (id, true)
            };
            let bytes = match ctx.heap.get(id) {
                Value::Bytes(bytes) | Value::ByteArray(bytes) => bytes.as_slice(),
                Value::Str(text) if decode && text.is_ascii() => text.as_bytes(),
                Value::Str(_) if decode => {
                    return Err(Error::new(
                        "ValueError",
                        "string argument should contain only ASCII characters",
                    ));
                }
                _ => {
                    let ty = ctx.type_name(id);
                    let message = if decode {
                        format!(
                            "argument should be a bytes-like object or ASCII string, not '{ty}'"
                        )
                    } else {
                        format!("a bytes-like object is required, not '{ty}'")
                    };
                    return Err(Error::new("TypeError", message));
                }
            };
            let result = if decode {
                crate::objects::base64_decode(bytes, urlsafe)?
            } else {
                crate::objects::urlsafe_encode(bytes)?
            };
            ctx.alloc(Value::Bytes(result))
        }
        "hmac.new" | "hmac.HMAC" => return hmac::new(ctx, args, kwargs),
        "hmac.compare_digest" => return hmac::compare(ctx, args),
        method if method.starts_with("hmac.HMAC.") => return hmac::method(ctx, method, args),
        "hashlib.sha1" => return hashlib::sha1(ctx, args, kwargs),
        "hashlib.sha256" => return hashlib::sha256(ctx, args, kwargs),
        "hash.update" | "hash.copy" | "hash.digest" | "hash.hexdigest" => {
            return hashlib::method(ctx, name, args);
        }
        "platform.system" | "platform.node" | "platform.release" | "platform.version"
        | "platform.machine" => {
            let uname = system::uname().map_err(os_error)?;
            ctx.text(match name {
                "platform.system" => uname.system,
                "platform.node" => uname.node,
                "platform.release" => uname.release,
                "platform.version" => uname.version,
                _ => uname.machine,
            })
        }
        "platform.mac_ver" => {
            let (release, machine) = system::mac_ver().map_err(os_error)?;
            let release = ctx.text(release);
            let version_info = tuple_text(ctx, &["", "", ""]);
            let machine = ctx.text(machine);
            ctx.alloc(Value::Tuple(vec![release, version_info, machine]))
        }
        "os.uname" => {
            let uname = system::uname().map_err(os_error)?;
            let data = [
                uname.system,
                uname.node,
                uname.release,
                uname.version,
                uname.machine,
            ]
            .map(|v| ctx.text(v));
            record(
                ctx,
                &[
                    ("sysname", data[0]),
                    ("nodename", data[1]),
                    ("release", data[2]),
                    ("version", data[3]),
                    ("machine", data[4]),
                ],
            )
        }
        "sysconfig.get_config_var" => {
            let value = layout(ctx)?.config_var(ctx.str_value(argument(args, 0)?)?);
            match value {
                serde_json::Value::String(s) => ctx.text(s),
                serde_json::Value::Number(n) => ctx.int(n.as_i64().unwrap()),
                _ => NONE,
            }
        }
        "sysconfig.get_path_names" => tuple_text(ctx, system::PATH_NAMES),
        "sysconfig.get_scheme_names" => tuple_text(ctx, system::SCHEME_NAMES),
        "sysconfig.get_default_scheme" => ctx.text("posix_prefix"),
        "sysconfig.get_preferred_scheme" => {
            if ctx.str_value(argument(args, 0)?)? != "prefix" {
                return Err(Error::new("KeyError", "unsupported installation scheme"));
            }
            ctx.text("posix_prefix")
        }
        "sysconfig.get_path" | "sysconfig.get_paths" => {
            let all = name.ends_with("_paths");
            let index = usize::from(!all);
            let scheme = option(args, index, kwargs, "scheme")?
                .map(|id| ctx.str_value(id))
                .transpose()?
                .unwrap_or("posix_prefix")
                .to_owned();
            if option(args, index + 1, kwargs, "vars")?.is_some_and(|id| id != NONE) {
                return Err(Error::new(
                    "NotImplementedError",
                    "custom installation variables are unsupported",
                ));
            }
            let expand = ctx.optional_truth(option(args, index + 2, kwargs, "expand")?, true)?;
            let layout = layout(ctx)?;
            let get = |name| {
                layout.path(&scheme, name, expand).ok_or_else(|| {
                    Error::new(
                        "KeyError",
                        format!("unknown installation scheme '{scheme}' or path '{name}'"),
                    )
                })
            };
            if all {
                let mut paths = Vec::new();
                for &name in system::PATH_NAMES {
                    paths.push((ctx.text(name), ctx.text(get(name)?)));
                }
                ctx.dict(paths)
            } else {
                let name = option(args, 0, kwargs, "name")?
                    .ok_or_else(|| Error::new("TypeError", "missing path name"))?;
                ctx.text(get(ctx.str_value(name)?)?)
            }
        }
        "sysconfig.get_platform" => ctx.text(system::sysconfig_platform().map_err(os_error)?),
        "site.getsitepackages" => {
            let site = ctx.text(layout(ctx)?.path("posix_prefix", "purelib", true).unwrap());
            ctx.alloc(Value::List(vec![site]))
        }
        "os.confstr" => {
            let value = system::confstr(ctx.str_value(argument(args, 0)?)?).map_err(|err| {
                if err.kind() == std::io::ErrorKind::InvalidInput {
                    Error::new("ValueError", err.to_string())
                } else {
                    os_error(err)
                }
            })?;
            value.map_or(NONE, |value| ctx.text(value))
        }
        "struct.calcsize" => {
            let size = system::struct_calcsize(ctx.str_value(argument(args, 0)?)?)
                .map_err(|e| Error::new("struct.error", e))?;
            ctx.int(size as i64)
        }
        "struct.unpack" => {
            let format = ctx.str_value(argument(args, 0)?)?;
            let Value::Bytes(bytes) = ctx.heap.get(argument(args, 1)?) else {
                return Err(Error::new("TypeError", "expected bytes"));
            };
            let values =
                system::struct_unpack(format, bytes).map_err(|e| Error::new("struct.error", e))?;
            structure::unsigned_tuple(ctx, values)
        }
        "json.loads" => {
            let input = option(args, 0, kwargs, "s")?.ok_or_else(|| {
                Error::new(
                    "TypeError",
                    "loads() missing 1 required positional argument: 's'",
                )
            })?;
            json::loads(ctx, input)?
        }
        "json.dumps" => {
            let input = option(args, 0, kwargs, "obj")?.ok_or_else(|| {
                Error::new(
                    "TypeError",
                    "dumps() missing 1 required positional argument: 'obj'",
                )
            })?;
            let text = json::dumps(ctx, input, kwargs)?;
            ctx.text(text)
        }
        "functools.update_wrapper" => functools::update_wrapper(ctx, args, kwargs)?,
        "functools.lru_cache" => {
            if args.is_empty() && kwargs.is_empty() {
                ctx.alloc(Value::Native("functools._cached".into()))
            } else if args.len() == 1
                && matches!(ctx.heap.get(args[0]), Value::Function { .. })
                && kwargs.is_empty()
            {
                ctx.cached_function(args[0])
            } else {
                return Err(Error::new(
                    "NotImplementedError",
                    "custom lru_cache parameters are unsupported",
                ));
            }
        }
        "functools._cached" => ctx.cached_function(argument(args, 0)?),
        "collections.defaultdict.copy" | "collections.defaultdict.__copy__" => {
            let receiver = argument(args, 0)?;
            if let source @ Value::DefaultDict { .. } = ctx.heap.get(receiver) {
                ctx.alloc(source.clone())
            } else {
                let factory = ctx.defaultdict_factory(receiver).ok_or_else(|| {
                    Error::new("TypeError", "defaultdict.copy requires a defaultdict")
                })?;
                let class = ctx.exact_type(receiver)?;
                ctx.call(class, vec![factory, receiver], Vec::new())?
            }
        }
        "collections.defaultdict.__missing__" => {
            ctx.defaultdict_missing(argument(args, 0)?, argument(args, 1)?)?
        }
        "collections.defaultdict.__reduce__" => {
            let receiver = argument(args, 0)?;
            let Some(factory) = ctx.defaultdict_factory(receiver) else {
                return Err(Error::new(
                    "TypeError",
                    "defaultdict.__reduce__ requires a defaultdict",
                ));
            };
            let args = ctx.alloc(Value::Tuple(if factory == NONE {
                Vec::new()
            } else {
                vec![factory]
            }));
            let class = ctx.exact_type(receiver)?;
            let items =
                ctx.dict_iterator(receiver, super::value::DictViewKind::Items, false, false);
            ctx.alloc(Value::Tuple(vec![class, args, NONE, NONE, items]))
        }
        "contextlib.contextmanager" => {
            let callable = ctx.alloc(Value::Native("contextlib._manager".into()));
            ctx.alloc(Value::Method {
                callable,
                receiver: argument(args, 0)?,
                explicit: false,
            })
        }
        "warnings.warn" => {
            eprintln!("Warning: {}", ctx.str_value(argument(args, 0)?)?);
            NONE
        }
        "sys.exception" => ctx.heap.active_exception(),
        "sys.exc_info" => {
            let exc = ctx.heap.active_exception();
            let (class, traceback) = if exc == NONE {
                (NONE, NONE)
            } else {
                (ctx.exact_type(exc)?, ctx.exception_traceback(exc)?)
            };
            ctx.alloc(Value::Tuple(vec![class, exc, traceback]))
        }
        "sys.exit" => {
            // PyErr_SetObject normalizes a tuple as the exception's constructor
            // arguments and propagates a supplied SystemExit instance directly.
            if let [arg] = args {
                match ctx.heap.get(*arg) {
                    Value::None => return Err(ctx.exception("SystemExit", &[])),
                    Value::Tuple(items) | Value::NamedTuple { values: items, .. } => {
                        let items = items.clone();
                        return Err(ctx.exception("SystemExit", &items));
                    }
                    Value::Raised(error) if error.kind == "SystemExit" => return Err(error.clone()),
                    _ => {}
                }
            }
            return Err(ctx.exception("SystemExit", args));
        }
        name if name.starts_with("os.path.")
            || name.starts_with("file.")
            || name == "subprocess.run" =>
        {
            return files::call(ctx, name, args, kwargs);
        }
        "match.groups" => {
            return regex::match_groups(
                ctx,
                argument(args, 0)?,
                option(args, 1, kwargs, "default")?.unwrap_or(NONE),
            );
        }
        name if name.starts_with("pattern.") || name == "match.group" => {
            return regex::call(ctx, name, args);
        }
        _ => {
            return Err(Error::new(
                "NotImplementedError",
                format!("unsupported native call {name}"),
            ));
        }
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_elements_keeps_a_bounded_number_of_temporary_roots() {
        let roots = [1, 256].map(|length| {
            // SAFETY: the empty argv is valid for zero arguments, and the config is NUL-terminated.
            let mut ctx = unsafe {
                Box::from_raw(super::super::ffi::imre_dyn_new(
                    0,
                    [].as_ptr(),
                    c"{\"paths\":[]}".as_ptr(),
                ))
            };
            let mapping = ctx.alloc(Value::Dict(Default::default()));
            let key = ctx.text("key");
            let values = ctx.alloc(Value::List(vec![key; length]));
            let before = ctx.heap.temporary_mark();
            count_elements(&mut ctx, &[mapping, values], &[]).unwrap();
            ctx.heap.temporary_mark() - before
        });
        assert_eq!(roots[0], roots[1]);
    }
}
