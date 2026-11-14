//! Python system modules available to native functions with runtime-selected imports.

use std::path::{Path, PathBuf};

use super::{
    Context, builtins,
    value::{self, Error, FALSE, Id, NONE, Result, TRUE, Value},
};
use crate::objects::interpreter_stdlib as system;

pub(super) mod ast;
mod asyncgen_hooks;
pub(super) mod audit;
mod binascii;
pub(super) mod bisect;
pub(super) mod bz2;
pub(super) mod codecs;
mod contextvars;
mod csv;
pub(super) mod datetime;
mod email;
pub(super) mod faulthandler;
#[cfg(target_os = "linux")]
mod fcntl;
mod files;
pub(super) mod fixed_structseq;
pub(super) mod flags;
mod fnmatch;
mod formatters;
pub(super) mod functools;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) mod grp;
mod gzip;
pub(super) mod hashlib;
mod hmac;
mod idna;
mod inspect;
pub(super) mod itertools;
pub(super) mod locale;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) mod pwd;
pub(super) mod tempfile;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) mod termios;
pub(super) mod time;
mod tokenize;
pub(super) mod unicodedata;
pub(super) mod zstd;
pub(super) use files::BinaryFile;
pub(super) use socket::{
    IMPORTED_FUNCTIONS as SOCKET_FUNCTIONS, import_socket_constants, install_lowlevel_socket,
};
pub(super) mod io;
mod json;
pub(super) mod lzma;
mod marshal;
mod math;
pub(super) use math::fraction_hash;
#[cfg(target_os = "linux")]
pub(super) mod epoll;
mod metadata;
#[cfg(target_os = "linux")]
pub(super) mod mmap;
pub(super) mod monitoring;
pub(super) mod opcode;
pub(super) mod operator;
mod ordered_dict;
pub(super) mod os;
pub(super) mod pickle;
mod pkgutil;
#[cfg(target_os = "linux")]
pub(super) mod posix_spawn;
mod posixpath;
#[cfg(target_os = "linux")]
mod process_arguments;
pub(super) mod pyexpat;
pub(super) mod random;
pub(super) mod readiness;
pub(super) mod regex;
#[cfg(target_os = "linux")]
mod resource;
mod select;
#[cfg(unix)]
pub(super) mod signal;
pub(super) mod socket;
#[cfg(unix)]
pub(super) mod ssl;
pub(super) mod structseq;
pub(super) mod structure;
mod subprocess;
mod system_size;
pub(super) mod traceback;
pub(super) mod tracemalloc;
pub(super) mod urllib;
mod warnings;
pub(super) mod warnings_accel;
mod zipimport;
pub(super) mod zlib;

/// Format a type for CPython diagnostics, including a non-default module and its qualname.
pub(super) fn qualified_type(ctx: &mut Context, class: Id) -> Result<Id> {
    let name = match ctx.own_class_attr(class, "__qualname__") {
        Some(name) => name,
        None => ctx.text(ctx.class_name(class).to_owned()),
    };
    let Some(module) = ctx.own_class_attr(class, "__module__") else {
        return Ok(name);
    };
    if ctx.string_storage(module).is_none()
        || matches!(ctx.str_value(module), Ok("builtins" | "__main__"))
    {
        return Ok(name);
    }
    let dot = ctx.text(".");
    let prefix = ctx.concat_strings(module, dot)?;
    ctx.concat_strings(prefix, name)
}

/// The JIT observer is a sys attribute with its own namespace, outside the import cache.
pub(super) fn jit_module(ctx: &mut Context) -> Result<Id> {
    let class = ctx.builtin("module");
    let module = ctx.allocate_module(class);
    let name = ctx.text("sys._jit");
    let doc = ctx.text("Utilities for observing just-in-time compilation.");
    ctx.init_module(module, &[name, doc], &[])?;
    for leaf in ["is_available", "is_enabled", "is_active"] {
        let callable = ctx.builtin(&format!("sys._jit.{leaf}"));
        let function = ctx.alloc(Value::Method {
            callable,
            receiver: module,
            explicit: false,
        });
        ctx.set_attr(module, leaf, function)?;
    }
    Ok(module)
}

pub(super) const SYS_BOUND_FUNCTIONS: &[&str] = &[
    "audit",
    "addaudithook",
    "getsizeof",
    "getrecursionlimit",
    "get_asyncgen_hooks",
    "set_asyncgen_hooks",
    "is_finalizing",
    "setrecursionlimit",
    "_getframe",
    "_getframemodulename",
    "displayhook",
    "excepthook",
    "unraisablehook",
    "get_int_max_str_digits",
    "set_int_max_str_digits",
];

pub(super) const SYS_EAGER_FUNCTIONS: &[&str] = &[
    #[cfg(target_os = "linux")]
    "_get_cpu_count_config",
    "get_coroutine_origin_tracking_depth",
    "set_coroutine_origin_tracking_depth",
];

pub(super) const SYS_LAZY_FUNCTIONS: &[&str] = &[
    "exit",
    "exc_info",
    "exception",
    "intern",
    "_is_interned",
    "getdefaultencoding",
    "getfilesystemencoding",
    "getfilesystemencodeerrors",
];

pub(super) fn is_module(name: &str) -> bool {
    imre_ast::providers::provider(name).is_some_and(provider_available)
}

pub(crate) fn provider_available(provider: imre_ast::providers::Provider) -> bool {
    use imre_ast::providers::Provider;
    match provider {
        Provider::Builtin => true,
        #[cfg(unix)]
        Provider::Ssl => ssl::provider::available(),
        Provider::Hashlib => hashlib::openssl::provider().is_some(),
    }
}

pub(super) use imre_ast::providers::has_lazy_attributes;

/// Populate errno in platform registration order so aliases and the reverse
/// mapping share the same definitions as the ordinary compiler.
pub(super) fn install_errno(ctx: &mut Context, scope: Id) -> Result<()> {
    let doc = ctx.text(crate::objects::ERRNO_DOC);
    ctx.bind(scope, "__doc__", doc);
    let errorcode = ctx.dict(Vec::new());
    ctx.bind(scope, "errorcode", errorcode);
    for (symbol, value) in crate::objects::errno_entries() {
        let code = ctx.int(i64::from(value));
        ctx.bind(scope, symbol, code);
        let name = ctx.text(symbol);
        ctx.put_item(errorcode, code, name)?;
    }
    Ok(())
}

pub(super) fn os_error(err: std::io::Error) -> Error {
    crate::objects::errno_error(err).into()
}

fn argument(args: &[Id], index: usize) -> Result<Id> {
    args.get(index)
        .copied()
        .ok_or_else(|| Error::new("TypeError", "missing required argument"))
}

/// Name the actual type for C diagnostics without consulting Python attribute hooks.
pub(in crate::dynamic) fn diagnostic_type_name(
    ctx: &Context,
    value: Id,
) -> std::borrow::Cow<'_, str> {
    if matches!(ctx.heap.get(value), Value::Struct { .. }) {
        return "_struct.Struct".into();
    }
    if let Value::Calendar { value, .. } = ctx.heap.get(value) {
        return format!("datetime.{}", value.class()).into();
    }
    if let Some(class) = ctx.heap.get(value).instance_class() {
        let ty = ctx.heap.get(class);
        if let Some(name) = ty.io_kind() {
            return format!("_io.{name}").into();
        }
        // Only CPython extension types have these qualified tp_names. A private
        // implementation tag can also identify a Python class like random.Random.
        if let Some(
            name @ (hashlib::md5::CLASS
            | datetime::isocalendar::CLASS
            | ssl::provider::bio::CLASS
            | value::SSL_ERROR_TYPE),
        ) = ty.heap_type()
        {
            return name.into();
        }
        // The bundled itertools module freezes the types that stand in for C types.
        if let Value::Class { scope, .. } = ty
            && ctx.scope_data(*scope).module == "itertools"
            && ctx
                .scope_data(*scope)
                .class_namespace
                .as_ref()
                .is_some_and(|ns| ns.immutable_type)
        {
            return format!("itertools.{}", ctx.class_name(class)).into();
        }
        // Ordinary instances use the runtime's registered type name, without exposing private heap tags.
        if ty.heap_type().is_none() && matches!(ctx.heap.get(value), Value::Instance { .. }) {
            return ctx.type_name(value).into();
        }
        return ctx.class_name(class).into();
    }
    ctx.builtin_type_name(value)
        .unwrap_or_else(|| ctx.type_name(value))
        .into()
}

/// PyArg formats at most 50 bytes of tp_name before decoding the entire diagnostic as UTF-8.
fn string_argument_message(
    function: &str,
    position: usize,
    kind: &str,
) -> std::result::Result<String, crate::Error> {
    let mut message = format!("{function}() argument {position} must be str, not ").into_bytes();
    message.extend_from_slice(&kind.as_bytes()[..kind.len().min(50)]);
    crate::objects::Codec::Utf8 { signature: false }.decode(&message, None)
}

/// Bind a native implementation of a Python function. Keyword errors take
/// precedence over excess positional arguments, even when both apply.
fn python_arguments<'a, const N: usize>(
    name: &str,
    params: [&str; N],
    positional: usize,
    defaults: usize,
    args: impl IntoIterator<Item = &'a Id>,
    kwargs: &[(String, Id)],
) -> Result<[Option<Id>; N]> {
    let mut args = args.into_iter().copied();
    let mut values = [None; N];
    let mut supplied = 0;
    for (slot, value) in values[..positional].iter_mut().zip(&mut args) {
        *slot = Some(value);
        supplied += 1;
    }
    bind_python_keywords(
        name,
        params.split_at(positional),
        supplied + args.count(),
        defaults,
        kwargs,
        &mut values,
    )?;
    Ok(values)
}

/// These providers supply their keyword-only defaults when consuming the bound slots.
fn bind_python_keywords(
    name: &str,
    (positional, keyword_only): (&[&str], &[&str]),
    supplied: usize,
    defaults: usize,
    kwargs: &[(String, Id)],
    values: &mut [Option<Id>],
) -> Result<()> {
    let shape = crate::source_arguments::Shape {
        positional,
        positional_only: 0,
        keyword_only,
        varargs: false,
        kwargs: false,
    };
    let keys = kwargs.iter().map(|(key, _)| key.as_str());
    let binding = shape.bind(name, supplied, keys, defaults, |_| true)?;
    for ((_, value), slot) in kwargs.iter().zip(binding.keywords) {
        values[slot.expect("fixed keyword parameter")] = Some(*value);
    }
    Ok(())
}

/// Bind a Python function whose parameters are all required and positional-or-keyword.
fn required_arguments<'a, const N: usize>(
    name: &str,
    params: [&str; N],
    args: impl IntoIterator<Item = &'a Id>,
    kwargs: &[(String, Id)],
) -> Result<[Id; N]> {
    Ok(python_arguments(name, params, N, 0, args, kwargs)?.map(Option::unwrap))
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

fn tuple_text(ctx: &mut Context, values: &[&str]) -> Id {
    let values = values.iter().map(|&text| ctx.text(text)).collect();
    ctx.alloc(Value::Tuple(values))
}

fn language_version(config: &serde_json::Value) -> Result<(u8, u8)> {
    let (major, minor) = config["python_version"]
        .as_str()
        .and_then(|v| v.split_once('.'))
        .ok_or_else(|| Error::new("RuntimeError", "missing Imre language version"))?;
    let parse = |v: &str| {
        v.parse()
            .map_err(|_| Error::new("RuntimeError", "invalid Imre language version"))
    };
    Ok((parse(major)?, parse(minor)?))
}

pub(super) fn interpreter_layout(
    config: &serde_json::Value,
    executable: &Path,
) -> Result<system::InterpreterLayout> {
    system::InterpreterLayout::new(executable, language_version(config)?).map_err(os_error)
}

fn layout(ctx: &Context) -> Result<system::InterpreterLayout> {
    ctx.interpreter_layout.clone()
}

/// Initialize the cached site attributes before the Python-level directory helpers run.
pub(super) fn initialize_site(ctx: &mut Context, scope: Id) -> Result<()> {
    let get_path = ctx.builtin("site._get_path");
    ctx.bind(scope, "_get_path", get_path);
    let layout = layout(ctx)?;
    ctx.bind(
        scope,
        "_include_system_site",
        builtins::boolean(ctx.config["no_site"] != true && layout.venv_system_site == Some(true)),
    );
    let mut enabled = NONE;
    let mut base = NONE;
    let mut directory = NONE;
    if ctx.config["no_site"] != true {
        let allowed = system::InterpreterLayout::user_site_enabled(
            ctx.config["no_user_site"] == true,
            layout.venv_system_site,
        );
        enabled = match allowed {
            Some(true) => TRUE,
            Some(false) => FALSE,
            None => NONE,
        };
        if let Some(path) = ctx.config["initial_user_base"].as_str().map(PathBuf::from) {
            base = path_text(ctx, &path)?;
            directory = path_text(ctx, &layout.user_site(&path))?;
        } else {
            enabled = FALSE;
        }
    }
    ctx.bind(scope, "ENABLE_USER_SITE", enabled);
    ctx.bind(scope, "USER_BASE", base);
    ctx.bind(scope, "USER_SITE", directory);
    Ok(())
}

/// Resolve a native package's search directory relative to this executable's installation.
pub(super) fn package_path(ctx: &Context, name: &str) -> Result<PathBuf> {
    let stdlib = layout(ctx)?
        .path("posix_prefix", "stdlib", true)
        .expect("the default scheme defines the standard library path");
    Ok(Path::new(&stdlib).join(name))
}

fn full_version(ctx: &Context) -> Result<String> {
    let (major, minor) = language_version(&ctx.config)?;
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

pub(super) fn sys_version(ctx: &mut Context) -> Result<Id> {
    Ok(ctx.text(format!(
        "{} (Imre {})",
        full_version(ctx)?,
        compiler_version(ctx)
    )))
}

fn path_text(ctx: &mut Context, path: &Path) -> Result<Id> {
    path.to_str()
        .map(|value| ctx.text(value))
        .ok_or_else(|| Error::new("UnicodeError", "filesystem path contains surrogate escapes"))
}

/// Materialize the target layout's format strings for Python's sysconfig expander.
fn install_schemes(ctx: &mut Context) -> Result<Id> {
    let layout = layout(ctx)?;
    let mut schemes = Vec::new();
    for &scheme in system::SCHEME_NAMES {
        let mut paths = Vec::new();
        for &name in system::PATH_NAMES {
            if let Some(template) = layout.path(scheme, name, false) {
                paths.push((ctx.text(name), ctx.text(template)));
            }
        }
        schemes.push((ctx.text(scheme), ctx.dict(paths)));
    }
    Ok(ctx.dict(schemes))
}

pub(super) fn attribute(ctx: &mut Context, module: &str, name: &str) -> Result<Id> {
    #[cfg(target_os = "linux")]
    if module == "fcntl"
        && let Some(value) = fcntl::constant(name)
    {
        return Ok(ctx.int(i64::from(value)));
    }
    if module == "_imre_subprocess" && subprocess::function(name) {
        return Ok(ctx.builtin(&format!("{module}.{name}")));
    }
    if module == "zlib"
        && let Some(value) = imre_ast::zlib::integer_constant(name)
    {
        return Ok(ctx.int(value));
    }
    if (module, name) == ("importlib", "machinery") {
        return ctx.module("importlib.machinery");
    }
    let id = match (module, name) {
        ("sysconfig", "_INSTALL_SCHEMES") => return install_schemes(ctx),
        #[cfg(all(unix, not(target_os = "linux")))]
        ("_signal", "SIGINT") => ctx.int(i64::from(libc::SIGINT)),
        #[cfg(all(unix, not(target_os = "linux")))]
        ("_signal", "SIGALRM") => ctx.int(i64::from(libc::SIGALRM)),
        #[cfg(all(unix, not(target_os = "linux")))]
        ("_signal", "SIGTERM") => ctx.int(i64::from(libc::SIGTERM)),
        #[cfg(all(unix, not(target_os = "linux")))]
        ("_signal", "alarm") => ctx.builtin("_signal.alarm"),
        (
            "codecs" | "_codecs",
            "encode" | "decode" | "register" | "unregister" | "lookup" | "escape_decode"
            | "utf_8_decode",
        ) => ctx.builtin(&format!("codecs.{name}")),
        ("codecs" | "_codecs", name) if codecs::WideFunction::named(name).is_some() => {
            ctx.builtin(&format!("codecs.{name}"))
        }
        ("_codecs", "_imre_bind_module_search") => ctx.builtin("_codecs._imre_bind_module_search"),
        ("_codecs", "_imre_codec_functions") => ctx.builtin("_codecs._imre_codec_functions"),
        ("_codecs", "_imre_idna_label") => ctx.builtin("_codecs._imre_idna_label"),
        ("_codecs", "_imre_native_search") => ctx.builtin("encodings.search_function"),
        ("_io", "open_code") => ctx.builtin("io.open_code"),
        (
            "math",
            "trunc" | "floor" | "ceil" | "modf" | "copysign" | "acos" | "acosh" | "atan2" | "cos"
            | "dist" | "exp" | "fma" | "frexp" | "fsum" | "gcd" | "hypot" | "isclose" | "isfinite"
            | "isinf" | "isnan" | "isqrt" | "lcm" | "ldexp" | "log" | "log2" | "pow" | "prod"
            | "sin" | "sqrt" | "sumprod" | "tan" | "ulp",
        ) => ctx.builtin(&format!("math.{name}")),
        ("enum", "auto") => ctx.enum_auto_class()?,
        ("enum", "Enum" | "Flag" | "IntEnum" | "IntFlag" | "StrEnum") => {
            ctx.builtin("enum.Enum.__copy__");
            ctx.builtin("enum.Enum.__deepcopy__");
            ctx.builtin("enum.Enum.__reduce_ex__");
            ctx.builtin(&format!("enum.{name}"))
        }
        (
            "_imre_tempfile",
            "abspath" | "mkdir" | "cleanup_directory" | "random_name" | "finalize_at_shutdown",
        ) => ctx.builtin(&format!("{module}.{name}")),
        ("_imre_gzip", "decoder" | "raw_tell" | "raw_read" | "raw_mtime" | "raw_rewind") => {
            ctx.builtin(&format!("{module}.{name}"))
        }
        ("_imre_gzip", "BadGzipFile") => ctx.builtin("gzip.BadGzipFile"),
        ("_locale", "getencoding") => ctx.builtin("_locale.getencoding"),
        ("_locale", "LC_ALL") => ctx.int(libc::LC_ALL.into()),
        ("_locale", "LC_COLLATE") => ctx.int(libc::LC_COLLATE.into()),
        ("_locale", "LC_CTYPE") => ctx.int(libc::LC_CTYPE.into()),
        ("_locale", "LC_MONETARY") => ctx.int(libc::LC_MONETARY.into()),
        ("_locale", "LC_NUMERIC") => ctx.int(libc::LC_NUMERIC.into()),
        ("_locale", "LC_TIME") => ctx.int(libc::LC_TIME.into()),
        ("_locale", "LC_MESSAGES") => ctx.int(libc::LC_MESSAGES.into()),
        ("tempfile", "TemporaryFile") => ctx.builtin("tempfile.TemporaryFile"),
        (
            "gc",
            "enable" | "disable" | "isenabled" | "get_threshold" | "set_threshold" | "is_tracked",
        ) => ctx.builtin(&format!("gc.{name}")),
        ("unicodedata", "unidata_version") => ctx.text(imre_ast::UNICODE_DATA_VERSION),
        (
            "unicodedata",
            "normalize" | "is_normalized" | "decomposition" | "east_asian_width" | "category"
            | "bidirectional",
        ) => ctx.builtin(&format!("unicodedata.{name}")),
        ("_imp", "pyc_magic_number_token") => ctx.int(i64::from(imre_ast::PYC_MAGIC_NUMBER_TOKEN)),
        (
            "_imp",
            "is_frozen"
            | "extension_suffixes"
            | "_override_frozen_modules_for_tests"
            | "_override_multi_interp_extensions_check",
        )
        | (
            "_imre_importlib",
            "find_spec"
            | "native_spec_info"
            | "native_source"
            | "native_code"
            | "frozen_info"
            | "frozen_code"
            | "is_builtin_module"
            | "builtin_names_contains"
            | "create_builtin_module"
            | "exec_builtin_module"
            | "create_native_module"
            | "exec_native_module"
            | "warn_import"
            | "registered_path"
            | "registered_path_spec"
            | "import_module"
            | "namespace_locations"
            | "get_source_data"
            | "uses_builtin_compile"
            | "compile_filename"
            | "open_run_path"
            | "compile_file"
            | "bind_doctest"
            | "compile_single"
            | "get_code"
            | "loader_fields"
            | "path_stats"
            | "source_name"
            | "warn_debug_override"
            | "warn_loader_deprecation"
            | "warn_legacy_load",
        )
        | (
            "_imre_dataclasses",
            "bind_initializer"
            | "is_defaultdict"
            | "loaded_module"
            | "members"
            | "native_iterator"
            | "slot_bases"
            | "slotted_type"
            | "retarget_class_cell",
        ) => ctx.builtin(&format!("{module}.{name}")),
        (
            "_imre_thread",
            "start" | "join" | "alive" | "threads" | "current" | "attach_current" | "shutdown",
        ) => ctx.builtin(&format!("{module}.{name}")),
        ("_thread", "_ThreadHandle") => ctx.builtin(super::thread_handle::CLASS),
        ("_thread", "LockType") => ctx.builtin(super::thread_lock::CLASS),
        ("_weakref", name) if let Some(value) = super::weakref::export(ctx, name) => value,
        ("_thread", "_local") => ctx.builtin(super::thread_local::CLASS),
        ("re", name) if regex::is_flag_attribute(name) => regex::flag_attribute(ctx, name)?,
        ("traceback", "_tb_position") => ctx.builtin("traceback._tb_position"),
        ("struct", "Struct") | ("functools", "partial") => ctx.builtin(&format!("{module}.{name}")),
        ("types", "MappingProxyType") => ctx.builtin("mappingproxy"),
        ("types", "ModuleType") => ctx.builtin("module"),
        ("types", "SimpleNamespace") => super::namespace::class(ctx)?,
        ("types", "TracebackType") => ctx.builtin("traceback"),
        ("types", "_coroutine_flags" | "_mark_generator") => ctx.builtin(&format!("types.{name}")),
        ("types", "AsyncGeneratorType") => ctx.builtin("async_generator"),
        ("types", "GeneratorType") => ctx.builtin("generator"),
        ("types", "CoroutineType") => ctx.builtin("coroutine"),
        ("types", "MethodType") => ctx.builtin("method"),
        ("types", "GetSetDescriptorType") => ctx.builtin("getset_descriptor"),
        ("types", "MemberDescriptorType") => ctx.builtin("member_descriptor"),
        ("types", "WrapperDescriptorType") => ctx.builtin("wrapper_descriptor"),
        ("types", "MethodWrapperType") => ctx.builtin("method-wrapper"),
        ("types", "MethodDescriptorType") => ctx.builtin("method_descriptor"),
        ("types", "FrameType") => ctx.builtin("frame"),
        ("types", "FunctionType") => ctx.builtin("function"),
        ("types", "BuiltinFunctionType" | "BuiltinMethodType") => {
            ctx.builtin("builtin_function_or_method")
        }
        ("types", "ClassMethodDescriptorType") => ctx.builtin("classmethod_descriptor"),
        ("types", "EllipsisType") => ctx.builtin("ellipsis"),
        ("types", "NoneType" | "NotImplementedType") => ctx.builtin(name),
        ("types", "CodeType") => ctx.builtin("code"),
        ("types", "CellType") => ctx.builtin("cell"),
        (
            "_collections",
            "_count_elements"
            | "_ordered_dict_equal"
            | "_ordered_dict_move_to_end"
            | "_ordered_dict_clear"
            | "_ordered_dict_delitem"
            | "_ordered_dict_setitem"
            | "_ordered_dict_pop"
            | "_ordered_dict_popitem"
            | "_ordered_dict_repr"
            | "_ordered_dict_setdefault"
            | "_ordered_dict_sizeof"
            | "_ordered_dict_view",
        ) => ctx.builtin(&format!("_collections.{name}")),
        ("collections" | "_collections", "_tuplegetter") => ctx.builtin(super::namedtuple::GETTER),
        ("collections", "defaultdict") => ctx.builtin("collections.defaultdict"),
        ("types", "GenericAlias") => ctx.builtin("types.GenericAlias"),
        (
            "_typing",
            "_clear_alias_caches"
            | "_no_type_check_member"
            | "_get_alias_kind"
            | "_eval_type_expression"
            | "_starred_forward_ref"
            | "_validate_forward_ref"
            | "_finish_field_ref"
            | "_get_type_annotations"
            | "_check_annotation_syntax"
            | "_install_any_namespace"
            | "_install_generic_alias_iter",
        ) => ctx.builtin(&format!("typing.{name}")),
        ("_typing", "_get_args") => ctx.builtin("typing.get_args"),
        ("_typing", "_get_origin") => ctx.builtin("typing.get_origin"),
        ("_typing", "_idfunc") => ctx.builtin("_typing._idfunc"),
        ("typing", "NoDefault") => super::typing::no_default(ctx),
        ("typing", "TYPE_CHECKING") => FALSE,
        ("typing", "_NamedTuple") => ctx.builtin("typing.NamedTuple"),
        ("typing", "Union") | ("types", "UnionType") => super::union::class(ctx),
        ("typing", "Generic" | "Protocol") => super::typing::named_class(ctx, "typing", name),
        ("_collections_abc", "Callable") => super::mapping_abc::callable(ctx)?,
        ("_collections_abc", "_CallableGenericAlias") => {
            ctx.builtin("collections.abc._CallableGenericAlias")
        }
        ("_collections_abc", "_check_methods") => ctx.builtin("_collections_abc._check_methods"),
        (
            "typing",
            "TypeVar"
            | "ParamSpec"
            | "ParamSpecArgs"
            | "ParamSpecKwargs"
            | "TypeVarTuple"
            | "TypeAliasType"
            | "_new_typeddict"
            | "_UnpackGenericAlias"
            | "_ConcatenateGenericAlias"
            | "_get_type_annotations"
            | "_eval_type_expression"
            | "_protocol_mro"
            | "_protocol_member"
            | "_check_annotation_syntax"
            | "_validate_forward_ref"
            | "_finish_field_ref"
            | "Any"
            | "Callable"
            | "overload"
            | "get_overloads"
            | "clear_overloads"
            | "_overload_dummy"
            | "_GenericAlias"
            | "_CallableGenericAlias"
            | "_LiteralGenericAlias",
        ) => ctx.builtin(&format!("typing.{name}")),
        #[cfg(target_os = "linux")]
        ("time", "CLOCK_REALTIME") => ctx.int(i64::from(libc::CLOCK_REALTIME)),
        ("time", "_STRUCT_TM_ITEMS") => ctx.int(11),
        ("time", name) if time::is_function(name) => ctx.builtin(&format!("time.{name}")),
        ("socket", name) => return socket::attribute(ctx, name),
        ("hashlib", name)
            if name != "md5"
                && crate::objects::HashState::public_constructor(name).is_none()
                && crate::objects::HashState::algorithms().any(|alg| alg == name) =>
        {
            ctx.alloc(Value::Native(format!("hashlib.{name}")))
        }
        ("hashlib", "algorithms_available" | "algorithms_guaranteed") => {
            return hashlib::algorithms(ctx);
        }
        #[cfg(target_os = "linux")]
        ("fcntl", "fcntl") => ctx.builtin("fcntl.fcntl"),
        #[cfg(target_os = "linux")]
        ("resource", "getrlimit" | "setrlimit") => ctx.builtin(&format!("resource.{name}")),
        #[cfg(target_os = "linux")]
        ("resource", "RLIMIT_CORE") => ctx.int(i64::from(libc::RLIMIT_CORE)),
        #[cfg(target_os = "linux")]
        ("resource", "RLIMIT_AS") => ctx.int(i64::from(libc::RLIMIT_AS)),
        #[cfg(target_os = "linux")]
        ("resource", "RLIMIT_FSIZE") => ctx.int(i64::from(libc::RLIMIT_FSIZE)),
        #[cfg(target_os = "linux")]
        ("resource", "RLIMIT_NOFILE") => ctx.int(i64::from(libc::RLIMIT_NOFILE)),
        #[cfg(target_os = "linux")]
        ("resource", "RLIMIT_NPROC") => ctx.int(i64::from(libc::RLIMIT_NPROC)),
        #[cfg(target_os = "linux")]
        ("resource", "RLIMIT_STACK") => ctx.int(i64::from(libc::RLIMIT_STACK)),
        #[cfg(target_os = "linux")]
        ("resource", "RLIM_INFINITY") => resource::limit(ctx, resource::INFINITY),
        ("random", "randrange") => ctx.builtin("random.randrange"),
        ("secrets", "choice") => ctx.builtin("secrets.choice"),
        ("select", "PIPE_BUF") => ctx.int(libc::PIPE_BUF as i64),
        ("select", "select" | "poll") => ctx.builtin(&format!("select.{name}")),
        ("select", name) if let Some(flag) = crate::objects::os::poll_constant(name) => {
            ctx.int(i64::from(flag))
        }
        ("sys", "argv") => ctx.argv,
        ("sys", "warnoptions") => {
            let options: Vec<String> = serde_json::from_value(ctx.config["warnoptions"].clone())
                .expect("startup warning options");
            let items = options.into_iter().map(|option| ctx.text(option)).collect();
            ctx.alloc(Value::List(items))
        }
        ("lzma", "decompress") => ctx.builtin("lzma.decompress"),
        ("lzma", "LZMAError") => ctx.builtin("_lzma.LZMAError"),
        ("zlib", "compressobj") => ctx.builtin("zlib.compressobj"),
        ("zlib", "ZLIB_VERSION" | "ZLIB_RUNTIME_VERSION") => ctx.text(imre_ast::zlib::VERSION),
        ("lzma", "FORMAT_AUTO") => ctx.int(0),
        ("lzma", "FORMAT_XZ") => ctx.int(1),
        ("lzma", "FORMAT_ALONE") => ctx.int(2),
        ("zlib", "adler32" | "crc32" | "compress" | "decompress" | "decompressobj" | "error") => {
            ctx.builtin(&format!("zlib.{name}"))
        }
        (
            "binascii",
            "Error" | "Incomplete" | "a2b_base64" | "b2a_base64" | "a2b_hex" | "unhexlify"
            | "b2a_hex" | "hexlify" | "a2b_qp" | "b2a_qp" | "crc32",
        ) => ctx.builtin(&format!("binascii.{name}")),
        ("sys", "base_prefix" | "base_exec_prefix") => path_text(ctx, &layout(ctx)?.base_prefix)?,
        ("sys", "prefix" | "exec_prefix") => path_text(ctx, &layout(ctx)?.prefix)?,
        ("sys", "platlibdir") => ctx.text("lib"),
        ("sys", "_base_executable") if ctx.config["executable"].is_string() => {
            path_text(ctx, &layout(ctx)?.base_executable)?
        }
        ("sys", "executable" | "_base_executable") => ctx.text(ctx.helper_executable.clone()),
        ("sys", "_imre_executable_suffix") => ctx.text(std::env::consts::EXE_SUFFIX),
        ("sys", "maxsize") => ctx.int(crate::sys::MAXSIZE as i64),
        ("sys", "maxunicode") => ctx.int(0x10FFFF),
        ("sys", "platform") => ctx.text(if cfg!(target_os = "macos") {
            "darwin"
        } else {
            std::env::consts::OS
        }),
        ("sys", "hash_info") => fixed_structseq::current_hash_info(ctx),
        ("sys", "version_info") => version_info(ctx, &full_version(ctx)?)?,
        ("sys", "implementation") => {
            let name = ctx.text("imre");
            let version = version_info(ctx, &compiler_version(ctx))?;
            let class = super::namespace::class(ctx)?;
            let object = ctx.instance(class, None);
            let dict = ctx.instance_dictionary(object, class);
            for (key, value) in [("name", name), ("version", version), ("cache_tag", NONE)] {
                let key = ctx.text(key);
                ctx.put_item(dict, key, value)?;
            }
            object
        }
        ("sys", "pycache_prefix") => {
            let prefix = ctx.config["pycache_prefix"].as_str().map(str::to_owned);
            prefix.map_or(NONE, |path| ctx.text(path))
        }
        ("sys", "dont_write_bytecode") => {
            if ctx.config["dont_write_bytecode"].as_bool().unwrap_or(false) {
                TRUE
            } else {
                FALSE
            }
        }
        ("sys", "flags") => flags::current(ctx),
        ("functools", "reduce") => {
            let provider = ctx.module("_functools")?;
            ctx.lookup_attr(provider, "reduce")?
        }
        ("functools", "_CacheInfo") => functools::cache_info_type(ctx)?,
        ("posixpath", "normpath") => ctx.builtin("posix._path_normpath"),
        ("posixpath", "splitroot") => ctx.builtin("posix._path_splitroot_ex"),
        ("posixpath", "sep") => ctx.text("/"),
        ("posixpath", "altsep") => NONE,
        ("contextvars" | "_contextvars", "ContextVar" | "Context" | "Token" | "copy_context") => {
            ctx.builtin(&format!("_contextvars.{name}"))
        }
        ("functools", "Placeholder") => functools::placeholder(ctx),
        ("functools", "_PlaceholderType") => ctx.builtin("functools._PlaceholderType"),
        ("sys", name) if SYS_LAZY_FUNCTIONS.contains(&name) => ctx.builtin(&format!("sys.{name}")),
        ("sysconfig", "_getuserbase" | "get_path_names" | "get_platform")
        | ("_imre_pkgutil", "filenames" | "isdir" | "find_module" | "find_spec")
        | ("_imre_base32", "encode" | "decode")
        | ("_imre_base85", "encode")
        | ("_imre_csv", "read_row")
        | ("_imre_tokenize", "scan")
        | (
            "_imre_json",
            "scan_once"
            | "scan_with_pairs_hook"
            | "is_constants_lookup"
            | "surrogate_text"
            | "encoder_state"
            | "encoder_call",
        )
        | ("_imre_linecache", "source_lines")
        | (
            "_imre_pyexpat",
            "version_info"
            | "create"
            | "parse"
            | "flush_buffer"
            | "position"
            | "error_code"
            | "error_string"
            | "in_callback"
            | "set_namespace_prefixes"
            | "get_reparse_deferral"
            | "set_reparse_deferral",
        )
        | ("_imre_metadata", "filenames" | "dedent" | "zip_names" | "zip_open" | "zip_read")
        | (
            "_imre_posixpath",
            "extension_start" | "string_args" | "normalized_parts" | "relative_parts"
            | "realpath_parts" | "realpath_parent" | "realpath_kind" | "realpath_readlink"
            | "realpath_notdir" | "realpath_loop",
        )
        | (
            "_imre_zipimport",
            "exists" | "isfile" | "index" | "read" | "contains" | "invalidate" | "names",
        )
        | ("platform", "system" | "node" | "release" | "version" | "machine" | "mac_ver")
        | (
            "struct",
            "calcsize" | "iter_unpack" | "pack" | "pack_into" | "unpack" | "unpack_from",
        )
        | ("base64", "b16encode" | "urlsafe_b64encode" | "urlsafe_b64decode")
        | ("random", "choice" | "choices" | "shuffle" | "randint")
        | ("codecs", "register_error" | "lookup_error")
        | ("gc", "collect")
        | (
            "re",
            "match" | "fullmatch" | "search" | "compile" | "_compile_scanner" | "split" | "findall"
            | "finditer",
        )
        | ("typing", "IO" | "Generator" | "Iterator")
        | ("enum", "_simple_enum")
        | ("functools", "cmp_to_key" | "_cache_wrapper" | "_reduce_iterator")
        | ("_string", "formatter_parser" | "formatter_field_name_split")
        | (
            "inspect",
            "_frame_position"
            | "_abstract_flags"
            | "_descriptor_get"
            | "_native_data_descriptor"
            | "_static_member"
        )
        | (
            "email.utils",
            "parsedate_to_datetime" | "format_datetime" | "formatdate" | "_has_surrogates",
        )
        | (
            "fnmatch",
            "fnmatch" | "fnmatchcase" | "filter" | "filterfalse" | "translate" | "_translate",
        ) => ctx.alloc(Value::Native(format!("{module}.{name}"))),
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

/// PyArg_ParseTupleAndKeywords checks the total count first, then converts each
/// required argument before looking for the next. Defer missing-argument errors
/// until each field is read so user conversions retain their exception precedence.
fn required_args<'a>(
    function: &'a str,
    args: &'a [Id],
    kwargs: &'a [(String, Id)],
    fields: &'a [&str],
) -> Result<impl Fn(usize) -> Result<Id> + 'a> {
    let count = args.len() + kwargs.len();
    let maximum = fields.len();
    if count > maximum {
        let kind = if args.is_empty() { "keyword " } else { "" };
        return Err(Error::new(
            "TypeError",
            format!("{function}() takes at most {maximum} {kind}arguments ({count} given)"),
        ));
    }
    Ok(move |i| {
        let field = fields[i];
        args.get(i)
            .copied()
            .or_else(|| {
                kwargs
                    .iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, value)| *value)
            })
            .ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "{function}() missing required argument '{field}' (pos {})",
                        i + 1
                    ),
                )
            })
    })
}

pub(super) fn index_c_int(ctx: &mut Context, value: Id) -> Result<i32> {
    ctx.index_integer(value)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

fn registered_single_code(ctx: &mut Context, source: Id, filename: Id, test: Id) -> Option<Id> {
    let origin = ctx.raw_instance_field(test, "filename")?;
    if !matches!(ctx.heap.get(origin), Value::Str(_) | Value::SurrogateStr(_)) {
        return None;
    }
    let path = ctx.import_path(origin).ok()?;
    let inherited = ctx.inherited_compile_flags();
    let super::LinkedSource::Doctest(owner) = ctx.heap.linked_sources.get(&test)? else {
        return None;
    };
    if owner.path.as_os_str() != path.as_os_str() {
        return None;
    }
    let (Value::Str(text), Value::Str(name)) = (ctx.heap.get(source), ctx.heap.get(filename))
    else {
        return None;
    };
    let input = owner.single_inputs.as_ref()?.iter().find(|input| {
        input.filename == name && input.source == text.as_bytes() && inherited & !input.flags == 0
    })?;
    Some(ctx.native_module_code(input.code, filename, Some(input.flags)))
}

/// Convert the real builtin's path and options in argument order. A Python compiler owns its operands.
pub(super) fn compile_operands(
    ctx: &mut Context,
    path: Id,
    dont_inherit: Option<Id>,
    optimize: Option<Id>,
) -> Result<(Id, bool)> {
    let spelling = posixpath::fspath(ctx, path)?;
    let filename = match ctx.heap.get(ctx.scalar_value(spelling)) {
        Value::Str(text) if !text.contains('\0') => spelling,
        Value::SurrogateStr(points) if !points.contains(&0) => spelling,
        Value::Bytes(bytes) if !bytes.contains(&0) => {
            ctx.text_codepoints(Context::import_path_points(bytes)?)
        }
        _ => return Err(Error::new("ValueError", "embedded null character")),
    };
    let dont_inherit = dont_inherit
        .map(|value| ctx.truth(value))
        .transpose()?
        .unwrap_or(false);
    if let Some(optimize) = optimize {
        let level = index_c_int(ctx, optimize)?;
        if !(-1..=2).contains(&level) {
            return Err(Error::new(
                "ValueError",
                "compile(): invalid optimize value",
            ));
        }
        let level = if level == -1 {
            ctx.config["optimize"].as_i64().unwrap_or(0)
        } else {
            i64::from(level)
        };
        if level != 0 {
            return Err(Error::new(
                "NotImplementedError",
                "native source compilation supports optimization level 0",
            ));
        }
    }
    Ok((filename, dont_inherit))
}

enum CacheFilename {
    Text(String),
    Object(Id),
}

/// Keep a reflected addition's result for the provider's normal join protocol.
fn cache_filename(ctx: &mut Context, suffixes: Id, mut stem: String) -> Result<CacheFilename> {
    let suffix = if let Value::List(entries) = ctx.heap.get(suffixes)
        && let Some(&first) = entries.first()
    {
        first
    } else {
        let zero = ctx.int(0);
        ctx.get_item(suffixes, zero)?
    };
    if let Value::Str(text) = ctx.heap.get(suffix) {
        stem.push_str(text);
        return Ok(CacheFilename::Text(stem));
    }
    let base = ctx.text(stem);
    ctx.binary("+", base, suffix).map(CacheFilename::Object)
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if let Some(method) = name.strip_prefix("faulthandler.") {
        return faulthandler::call(ctx, method, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("atexit.") {
        return super::atexit::call(ctx, method, args, kwargs);
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if name == "_locale.nl_langinfo" {
        return locale::nl_langinfo(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_tracemalloc.") {
        return tracemalloc::call(ctx, method, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_warnings.") {
        return warnings_accel::call(ctx, method, args, kwargs);
    }
    if name == "_locale.localeconv" {
        return locale::localeconv(ctx, args, kwargs);
    }
    if name == "_locale.setlocale" {
        return locale::setlocale(ctx, args, kwargs);
    }
    if name == "_imre_linecache.source_lines" {
        let ([data], []) = (args, kwargs) else {
            return Err(Error::new(
                "TypeError",
                "source_lines() expects one positional argument",
            ));
        };
        let Value::Bytes(bytes) = ctx.heap.get(ctx.scalar_value(*data)) else {
            return Err(Error::new("TypeError", "source_lines() requires bytes"));
        };
        use crate::objects::SourceDecodeError::*;
        let lines = crate::objects::decode_source_lines(bytes).map_err(|err| match err {
            Allocation => Error::new("MemoryError", ""),
            Null => Error::new(
                "SyntaxError",
                "source code string cannot contain null bytes",
            ),
            Invalid(message, _) => Error::new("SyntaxError", message),
            NonText(message) => Error::new("LookupError", message),
            Unsupported(name) => Error::new(
                "NotImplementedError",
                format!("linecache: native source decoder for '{name}' is unavailable"),
            ),
        })?;
        let lines = lines.into_iter().map(|line| ctx.text(line)).collect();
        return Ok(ctx.alloc(Value::List(lines)));
    }
    if super::lowlevel_threads::function_name(name).is_some() {
        return super::lowlevel_threads::call(ctx, name, args, kwargs);
    }
    #[cfg(unix)]
    if matches!(
        name,
        "_signal.pause"
            | "_signal.sigpending"
            | "_signal.set_wakeup_fd"
            | "_signal.siginterrupt"
            | "_signal.valid_signals"
            | "_imre_signal.make_signals"
            | "_thread.interrupt_main"
    ) {
        return signal::call(ctx, name, args, kwargs);
    }
    if name == "_thread.get_ident"
        || (cfg!(any(target_os = "linux", target_os = "macos")) && name == "_thread.get_native_id")
    {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no keyword arguments"),
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no arguments ({} given)", args.len()),
            ));
        }
        let id = match name {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            "_thread.get_native_id" => crate::objects::locks::native_thread_id() as usize,
            _ => crate::objects::locks::thread_ident(),
        };
        return Ok(ctx.alloc(Value::Int(crate::Int::from_usize(id))));
    }
    if let Some(function) = name.strip_prefix("_imre_thread.") {
        return super::thread_worker::call(ctx, function, args, kwargs);
    }
    if let Some(slot) = name.strip_prefix("functools.KeyWrapper.") {
        return functools::key_method(ctx, slot, args, kwargs);
    }
    #[cfg(unix)]
    if matches!(name, "_signal.signal" | "_signal.getsignal") {
        return ctx.signal_call(name, args, kwargs);
    }
    #[cfg(unix)]
    if name == "_signal.default_int_handler" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "_signal.default_int_handler() takes no keyword arguments",
            ));
        }
        if args.len() != 2 {
            return Err(Error::new(
                "TypeError",
                format!(
                    "default_int_handler expected 2 arguments, got {}",
                    args.len()
                ),
            ));
        }
        os::c_int(ctx, args[0])?;
        return Err(Error::new("KeyboardInterrupt", ""));
    }
    #[cfg(unix)]
    if name == "_signal.alarm" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "_signal.alarm() takes no keyword arguments",
            ));
        }
        let &[seconds] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "_signal.alarm() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        let seconds = os::c_int(ctx, seconds)?;
        // SAFETY: alarm takes a value only. POSIX interprets the C int as unsigned.
        let remaining = unsafe { libc::alarm(seconds as libc::c_uint) };
        return Ok(ctx.int(remaining as libc::c_long));
    }
    if name == "_io.text_encoding" {
        return io::text_encoding(ctx, args, kwargs);
    }
    if name == "io.open_code" {
        return io::open_code(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_bisect.") {
        return bisect::call(ctx, method, &args[1..], kwargs);
    }
    if let Some(method) = name.strip_prefix("_opcode.") {
        return opcode::call(ctx, method, &args[1..], kwargs);
    }
    if let Some(method) = name.strip_prefix("types.SimpleNamespace.") {
        return super::namespace::method(ctx, method, args, kwargs);
    }
    if name == "BaseException.with_traceback" {
        let &[exception, tb] = args else {
            return Err(Error::new(
                "TypeError",
                "with_traceback() requires one argument",
            ));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "with_traceback() takes no keyword arguments",
            ));
        }
        ctx.set_traceback(exception, tb)?;
        return Ok(exception);
    }
    if name == "frame.clear" {
        let &[frame] = args else {
            return Err(Error::new("TypeError", "clear() takes no arguments"));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "clear() takes no keyword arguments",
            ));
        }
        ctx.heap.clear_frame(frame)?;
        return Ok(NONE);
    }
    if name == "traceback._tb_position" {
        let [tb] = required_arguments("_tb_position", ["tb"], args, kwargs)?;
        return Ok(traceback::tb_position(ctx, tb));
    }

    if let Some(method) = name.strip_prefix("collections.defaultdict.")
        && matches!(method, "__missing__" | "copy" | "__copy__" | "__reduce__")
    {
        let Some(&receiver) = args.first() else {
            return Err(Error::new(
                "TypeError",
                format!("unbound method defaultdict.{method}() needs an argument"),
            ));
        };
        if ctx.defaultdict_factory(receiver).is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' for 'collections.defaultdict' objects doesn't apply to a '{}' object",
                    ctx.type_name(receiver)
                ),
            ));
        }
    }
    if let Some(method @ ("__repr__" | "__or__" | "__ror__")) =
        name.strip_prefix("collections.defaultdict.")
    {
        let Some((&object, args)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' of 'collections.defaultdict' object needs an argument"
                ),
            ));
        };
        let Some(storage) = ctx
            .dict_storage(object)
            .filter(|&storage| matches!(ctx.heap.get(storage), Value::DefaultDict { .. }))
        else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' requires a 'collections.defaultdict' object but received a '{}'",
                    ctx.type_name(object)
                ),
            ));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("wrapper {method}() takes no keyword arguments"),
            ));
        }
        if method != "__repr__" {
            let [other] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!("expected 1 argument, got {}", args.len()),
                ));
            };
            let (left, right) = if method == "__ror__" {
                (*other, object)
            } else {
                (object, *other)
            };
            return super::defaultdict::union(ctx, left, right);
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
    if name == "sys._getframemodulename" {
        let [depth] = builtins::bind_method_arguments(
            "_getframemodulename",
            &args[1..],
            kwargs,
            ["depth"],
            1,
        )?;
        let depth = match depth {
            Some(value) => index_c_int(ctx, value)?,
            None => 0,
        };
        let argument = ctx.int(i64::from(depth));
        audit::event(ctx, name, &[argument])?;
        return Ok(ctx
            .heap
            .caller_module_at(depth.max(0) as usize)
            .unwrap_or(NONE));
    }
    if name == "sys.displayhook" {
        let value = builtins::one_positional_arg(name, &args[1..], kwargs)?;
        return super::displayhook::default_hook(ctx, value);
    }
    if name == "sys.excepthook" {
        return super::excepthook::default_hook(ctx, &args[1..], kwargs);
    }
    if name == "sys.unraisablehook" {
        return super::unraisable::default_hook(ctx, args, kwargs);
    }
    if name == "sys.getsizeof" {
        return system_size::getsizeof(ctx, &args[1..], kwargs);
    }
    if name == "sys.set_coroutine_origin_tracking_depth" {
        let [depth] = builtins::bind_builtin_arguments(
            "set_coroutine_origin_tracking_depth",
            args,
            kwargs,
            ["depth"],
            1,
        )?;
        let depth = index_c_int(ctx, depth.expect("required depth argument"))?;
        ctx.threads.coroutine_origin_depth =
            u32::try_from(depth).map_err(|_| Error::new("ValueError", "depth must be >= 0"))?;
        return Ok(NONE);
    }
    if name == "sys.set_asyncgen_hooks" {
        return asyncgen_hooks::set(ctx, &args[1..], kwargs);
    }
    if matches!(
        name,
        "sys.getrecursionlimit" | "sys.is_finalizing" | "sys.get_asyncgen_hooks"
    ) {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no keyword arguments"),
            ));
        }
        if !args[1..].is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no arguments ({} given)", args.len() - 1),
            ));
        }
        if name == "sys.get_asyncgen_hooks" {
            return Ok(fixed_structseq::record(
                ctx,
                fixed_structseq::ASYNCGEN_HOOKS,
                ctx.threads.asyncgen_hooks.to_vec(),
            ));
        }
        if name == "sys.is_finalizing" {
            return Ok(builtins::boolean(
                ctx.interpreter
                    .finalizing
                    .load(std::sync::atomic::Ordering::Acquire),
            ));
        }
        return Ok(ctx.int(i64::from(crate::sys::recursion_limit())));
    }
    if name == "sys.setrecursionlimit" {
        let value = builtins::one_positional_arg(name, &args[1..], kwargs)?;
        let limit = index_c_int(ctx, value)?;
        crate::sys::set_recursion_limit(limit, ctx.heap.python_depth())?;
        return Ok(NONE);
    }
    if name == "sys._getframe" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "sys._getframe() takes no keyword arguments",
            ));
        }
        let depth = match &args[1..] {
            [] => 0,
            [value] => index_c_int(ctx, *value)?,
            extra => {
                return Err(Error::new(
                    "TypeError",
                    format!("_getframe expected at most 1 argument, got {}", extra.len()),
                ));
            }
        };
        let (_, frame) = ctx
            .active_frame(depth.max(0) as usize)?
            .ok_or_else(|| Error::new("ValueError", "call stack is not deep enough"))?;
        audit::event(ctx, "sys._getframe", &[frame])?;
        return Ok(frame);
    }
    if name == "sys.audit" {
        return audit::emit(ctx, &args[1..], kwargs);
    }
    if name == "sys.addaudithook" {
        return audit::register(ctx, &args[1..], kwargs);
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
        ctx.str_value(storage)?;
        let (heap, roots) = ctx.heap_and_roots();
        return Ok(heap.intern(*value, roots));
    }
    if name == "sys.set_int_max_str_digits" {
        return flags::set_int_max_str_digits(ctx, &args[1..], kwargs);
    }
    if matches!(
        name,
        "sys.get_int_max_str_digits"
            | "_struct._clearcache"
            | "sys._get_cpu_count_config"
            | "sys.get_coroutine_origin_tracking_depth"
            | "sys.getdefaultencoding"
            | "_socket.getdefaulttimeout"
            | "_locale.getencoding"
            | "sys.getfilesystemencoding"
            | "sys.getfilesystemencodeerrors"
            | "posix.abort"
            | "posix.getpid"
            | "posix.getuid"
            | "posix.geteuid"
            | "posix.getgid"
            | "posix.getegid"
            | "posix.getresgid"
            | "posix.getresuid"
            | "posix.getgroups"
            | "posix.cpu_count"
            | "sys._jit.is_available"
            | "sys._jit.is_enabled"
            | "sys._jit.is_active"
            | "posix.getcwd"
            | "posix.getcwdb"
            | "posix._create_environ"
    ) || (cfg!(target_os = "linux")
        && matches!(
            name,
            "posix.uname" | "posix.getloadavg" | "posix.ctermid" | "posix.sync"
        ))
    {
        // Strip the implicit receiver of functions bound to their module.
        let args = if name.starts_with("sys._jit.") || name == "sys.get_int_max_str_digits" {
            args.get(1..).ok_or_else(|| {
                Error::new("TypeError", format!("{name}() requires a module receiver"))
            })?
        } else {
            args
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no keyword arguments"),
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no arguments ({} given)", args.len()),
            ));
        }
        return Ok(match name {
            // Module functions parse each format on demand. Struct instances own their layouts.
            "_struct._clearcache" => NONE,
            #[cfg(target_os = "linux")]
            "sys._get_cpu_count_config" => {
                ctx.int(i64::from(crate::sys::cpu_count::configured().unwrap_or(-1)))
            }
            "sys.get_coroutine_origin_tracking_depth" => {
                ctx.int(i64::from(ctx.threads.coroutine_origin_depth))
            }
            "_socket.getdefaulttimeout" => {
                let timeout = *ctx.interpreter.socket_default_timeout.lock().unwrap();
                timeout.map_or(NONE, |d| ctx.alloc(Value::Float(d.as_secs_f64())))
            }
            #[cfg(target_os = "linux")]
            "posix.uname" => {
                let values = system::with_uname_fields(|fields| {
                    let codec =
                        crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
                    fields
                        .into_iter()
                        .map(|field| Ok(ctx.text_codepoints(codec.decode_filesystem(field)?)))
                        .collect::<Result<Vec<_>>>()
                })
                .map_err(os_error)?;
                fixed_structseq::record(ctx, fixed_structseq::UNAME_RESULT, values?)
            }
            #[cfg(target_os = "linux")]
            "posix.getloadavg" => {
                let mut averages = [0.0; 3];
                // SAFETY: the array has exactly three writable doubles.
                if unsafe { libc::getloadavg(averages.as_mut_ptr(), 3) } != 3 {
                    return Err(Error::new("OSError", "Load averages are unobtainable"));
                }
                let values = averages.map(|load| ctx.alloc(Value::Float(load)));
                ctx.alloc(Value::Tuple(values.into()))
            }
            #[cfg(target_os = "linux")]
            "posix.ctermid" => {
                let bytes = system::ctermid().map_err(os_error)?;
                let codec =
                    crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
                ctx.text_codepoints(codec.decode_filesystem(&bytes)?)
            }
            #[cfg(target_os = "linux")]
            "posix.sync" => {
                // SAFETY: sync takes no arguments and has no error result.
                unsafe { libc::sync() };
                NONE
            }
            "posix._create_environ" => os::environment(ctx),
            "posix.getcwd" | "posix.getcwdb" => os::getcwd(ctx, name == "posix.getcwdb")?,
            "sys.getfilesystemencoding" => ctx
                .heap
                .code_name(crate::sys::filesystem_encoding()?.as_str()),
            "sys.getfilesystemencodeerrors" => {
                ctx.heap.code_name(crate::sys::filesystem_errors().as_str())
            }
            "_locale.getencoding" => ctx.text(crate::sys::file_encoding(Some("locale"))),
            // SAFETY: C abort takes no arguments and terminates the calling process.
            #[cfg(unix)]
            "posix.abort" => unsafe { libc::abort() },
            "posix.getpid" => ctx.int(i64::from(std::process::id())),
            "posix.geteuid" => ctx.alloc(Value::Int(crate::objects::os::geteuid())),
            "posix.getuid" => ctx.alloc(Value::Int(crate::objects::os::getuid())),
            "posix.getgid" => ctx.alloc(Value::Int(crate::objects::os::getgid())),
            "posix.getegid" => ctx.alloc(Value::Int(crate::objects::os::getegid())),
            #[cfg(target_os = "linux")]
            "posix.getresgid" | "posix.getresuid" => {
                let ids = if name == "posix.getresgid" {
                    crate::objects::os::getresgid()?
                } else {
                    crate::objects::os::getresuid()?
                };
                let values = ids
                    .into_iter()
                    .map(|id| ctx.alloc(Value::Int(id)))
                    .collect();
                ctx.alloc(Value::Tuple(values))
            }
            "posix.getgroups" => {
                let groups = crate::objects::os::getgroups()?
                    .into_iter()
                    .map(|group| ctx.alloc(Value::Int(group)))
                    .collect();
                ctx.alloc(Value::List(groups))
            }
            #[cfg(target_os = "linux")]
            "posix.cpu_count" => {
                crate::sys::cpu_count::online().map_or(NONE, |n| ctx.int(i64::from(n)))
            }
            "sys.getdefaultencoding" => ctx.default_encoding,
            "sys.get_int_max_str_digits" => {
                ctx.int(i64::from(crate::integer_text::decimal_digit_limit()))
            }
            // As in typed SysJitPredicate, Imre compiles before the process starts.
            _ => FALSE,
        });
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(function @ ("getgrall" | "getgrgid" | "getgrnam")) = name.strip_prefix("grp.") {
        return grp::call(ctx, function, args, kwargs);
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(function @ ("getpwall" | "getpwnam" | "getpwuid")) = name.strip_prefix("pwd.") {
        return pwd::call(ctx, function, args, kwargs);
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(function @ ("tcgetattr" | "tcsetattr")) = name.strip_prefix("termios.") {
        return termios::call(ctx, function, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "fcntl.fcntl" {
        return fcntl::call(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    match name {
        "resource.getrlimit" => return resource::getrlimit(ctx, args, kwargs),
        "resource.setrlimit" => return resource::setrlimit(ctx, args, kwargs),
        _ => {}
    }
    if name == "random.seed" {
        return random::seed(ctx, args, kwargs);
    }
    if name == "random.getrandbits" {
        let count = random::bit_count(ctx, args, kwargs)?;
        return Ok(
            ctx.alloc(Value::Int(crate::Int::new(crate::objects::random_bits(
                &count,
            )?))),
        );
    }
    if name == "random.random" {
        random::random_args(args, kwargs)?;
        return Ok(ctx.alloc(Value::Float(crate::objects::imre_random_float())));
    }
    if name == "random.shuffle" {
        return random::shuffle(ctx, args, kwargs);
    }
    if name == "random.randint" {
        return random::randint(ctx, args, kwargs);
    }
    if name == "random.randrange" {
        return random::randrange(ctx, args, kwargs);
    }
    if name == "random.choices" {
        return random::choices(ctx, args, kwargs);
    }
    if name == "random._choices" {
        return random::instance_choices(ctx, args, kwargs);
    }
    if matches!(name, "random.choice" | "secrets.choice") {
        return random::choice(ctx, args, kwargs, name == "secrets.choice");
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
    if name == "string.templatelib.Template.get_values" {
        let [receiver] = args else {
            unreachable!("template values getter")
        };
        return ctx.read_member(super::value::TEMPLATE_TYPE, "values", *receiver);
    }
    if matches!(
        name,
        super::value::TEMPLATE_TYPE | super::value::INTERPOLATION_TYPE
    ) {
        return super::templatelib::construct(ctx, name, args, kwargs);
    }
    if matches!(
        name,
        "_io._BytesIOBuffer"
            | os::scandir::ENTRY
            | os::scandir::ITER
            | super::value::TEMPLATE_ITER_TYPE
    ) {
        return Err(Error::new(
            "TypeError",
            format!("cannot create '{name}' instances"),
        ));
    }
    if let Some((owner, method)) = name.rsplit_once('.')
        && matches!(owner, os::scandir::ENTRY | os::scandir::ITER)
    {
        return os::scandir::method(ctx, owner, method, args, kwargs);
    }
    if matches!(name, "_thread.allocate_lock" | "_thread.allocate")
        || name == super::thread_lock::CLASS
    {
        return super::thread_lock::construct(ctx, name, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_thread._local.") {
        return super::thread_local::method(ctx, method, args, kwargs);
    }
    if name == super::thread_handle::CLASS || name == "_thread._ThreadHandle.__new__" {
        if name.ends_with(".__new__") {
            let class = super::thread_handle::CLASS;
            let Some(&receiver) = args.first() else {
                return Err(Error::new(
                    "TypeError",
                    format!("{name}(): not enough arguments"),
                ));
            };
            if receiver != ctx.builtin(class) {
                return Err(ctx.invalid_new_subtype(class, receiver));
            }
        }
        return Ok(ctx.alloc(Value::ThreadHandle(Default::default())));
    }
    if let Some(method) = name.strip_prefix("_thread._ThreadHandle.") {
        return super::thread_handle::method(ctx, method, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_thread.lock.") {
        return super::thread_lock::method(ctx, method, args, kwargs);
    }
    if name == super::thread_rlock::CLASS {
        return Ok(ctx.alloc(Value::HandlerLock(Default::default())));
    }
    if let Some(method) = name.strip_prefix("_thread.RLock.") {
        return super::thread_rlock::method(ctx, method, args, kwargs);
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
    if name == "struct.iter_unpack" {
        return structure::module_iter_unpack(ctx, args, kwargs);
    }
    if name == "struct.unpack_from" {
        return structure::module_unpack_from(ctx, args, kwargs);
    }
    if matches!(name, "struct.pack" | "struct.pack_into") {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("_{name}() takes no keyword arguments"),
            ));
        }
        let Some((&format, values)) = args.split_first() else {
            return Err(Error::new("TypeError", "missing format argument"));
        };
        let (_, layout) = structure::parse(ctx, format)?;
        return if name == "struct.pack_into" {
            structure::pack_into(ctx, &layout, values)
        } else {
            structure::pack(ctx, &layout, values)
        };
    }
    if name == "struct.Struct" {
        return structure::new(ctx, args, kwargs);
    }
    if name == "unicodedata.east_asian_width" {
        return unicodedata::east_asian_width(ctx, args, !kwargs.is_empty());
    }
    if name == "unicodedata.bidirectional" {
        return unicodedata::bidirectional(ctx, args, !kwargs.is_empty());
    }
    if name == "unicodedata.category" {
        return unicodedata::category(ctx, args, !kwargs.is_empty());
    }
    if name == "unicodedata.decomposition" {
        return unicodedata::decomposition(ctx, args, !kwargs.is_empty());
    }
    if matches!(name, "unicodedata.normalize" | "unicodedata.is_normalized") {
        return unicodedata::normalization(
            ctx,
            "unicodedata",
            &name["unicodedata.".len()..],
            args,
            !kwargs.is_empty(),
        );
    }
    if let Some(operation) = name.strip_prefix("math.") {
        return math::call(ctx, operation, args, kwargs);
    }
    if name == "base64.b16encode" {
        let [data] = required_arguments("b16encode", ["s"], args, kwargs)?;
        return binascii::b16encode(ctx, data);
    }
    if matches!(name, "binascii.crc32" | "zlib.crc32" | "zlib.adler32") {
        return binascii::checksum(ctx, name, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("binascii.") {
        return binascii::call(ctx, method, args, kwargs);
    }
    if name == "zlib.compressobj" {
        return zlib::compressor(ctx, "new", args, kwargs);
    }
    if let Some(method) = name.strip_prefix("zlib.Compress.") {
        return zlib::compressor(ctx, method, args, kwargs);
    }
    if let Some(member) = name.strip_prefix("compression.zstd.")
        && (member.starts_with("ZstdCompressor") || member.starts_with("ZstdDecompressor"))
    {
        return zstd::call(ctx, member, args, kwargs);
    }
    if let Some(member) = name.strip_prefix("_bz2.") {
        return bz2::call(ctx, member, args, kwargs);
    }
    if let Some(member) = name.strip_prefix("_lzma.") {
        match member {
            "LZMACompressor" => return lzma::compressor(ctx, "new", args, kwargs),
            "LZMADecompressor" => return lzma::decompressor(ctx, "new", args, kwargs),
            "is_check_supported" => return lzma::is_check_supported(ctx, args, kwargs),
            "_encode_filter_properties" | "_decode_filter_properties" => {
                return lzma::properties(ctx, member, args, kwargs);
            }
            _ => {}
        }
        if let Some(method) = member.strip_prefix("LZMACompressor.") {
            return lzma::compressor(ctx, method, args, kwargs);
        }
        if let Some(method) = member.strip_prefix("LZMADecompressor.") {
            return lzma::decompressor(ctx, method, args, kwargs);
        }
    }
    if name == "lzma.decompress" {
        return lzma::call(ctx, "decompress", args, kwargs);
    }
    if name == "zlib.decompressobj" {
        return zlib::decompressor(ctx, "new", args, kwargs);
    }
    if let Some(method) = name.strip_prefix("zlib.Decompress.")
        && (matches!(method, "decompress" | "flush")
            || value::ZLIB_DECOMPRESS_COPY_METHODS.contains(&method))
    {
        return zlib::decompressor(ctx, method, args, kwargs);
    }
    if let Some(method @ ("compress" | "decompress")) = name.strip_prefix("zlib.") {
        return zlib::call(ctx, method, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("struct.Struct.") {
        return structure::method(ctx, method, args, kwargs);
    }
    if let Some(op) = name.strip_prefix("_imre_zipimport.") {
        return zipimport::call(ctx, op, args, kwargs);
    }
    if matches!(
        name,
        "posix.fspath" | "posix._path_normpath" | "posix._path_splitroot_ex"
    ) || name.starts_with("_imre_posixpath.")
    {
        return posixpath::call(ctx, name, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if let Some(op) = name.strip_prefix("posix.")
        && os::xattr::FUNCTIONS.contains(&op)
    {
        return os::xattr::call(ctx, op, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_imre_pyexpat.") {
        return pyexpat::call(ctx, method, args, kwargs);
    }
    if let Some(operation) = name.strip_prefix("_imre_metadata.") {
        return metadata::call(ctx, operation, args, kwargs);
    }
    if let Some(class) = name.strip_suffix(".__new__")
        && let Some(layout) = structseq::layout(class)
    {
        let Some((&ty, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!("{class}.__new__(): not enough arguments"),
            ));
        };
        if ty != ctx.builtin(class) {
            return Err(ctx.invalid_new_subtype(class, ty));
        }
        return structseq::construct(ctx, layout, rest, kwargs);
    }
    if let Some((class, method)) = name.rsplit_once('.')
        && let Some(layout) = structseq::layout(class)
        && layout.methods().contains(&method)
    {
        if method == "__repr__" {
            return builtins::call(ctx, name, args, kwargs);
        }
        return structseq::method(ctx, layout, method, args, kwargs);
    }
    if let Some(layout) = structseq::layout(name) {
        return structseq::construct(ctx, layout, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if let Some(op @ ("pipe2" | "posix_openpt" | "grantpt" | "unlockpt" | "ptsname")) =
        name.strip_prefix("posix.")
    {
        return os::descriptor(ctx, op, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if let Some(op @ ("statvfs" | "fstatvfs")) = name.strip_prefix("posix.") {
        return os::statvfs::call(ctx, op, args, kwargs);
    }
    if let Some(op @ ("stat" | "lstat" | "fstat")) = name.strip_prefix("posix.") {
        return os::stat::call(ctx, op, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "posix.execve" {
        return os::execve::call(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "posix.fork" {
        return os::fork(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "posix.register_at_fork" {
        return super::at_fork::register(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "posix._spawn_exec" {
        return os::spawn_exec(ctx, args, kwargs);
    }
    if let Some(op) = name.strip_prefix("posix.")
        && (os::attributes::is_descriptor_function(op) || (cfg!(unix) && op == "getpgrp"))
    {
        return os::descriptor(ctx, op, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if matches!(name, "posix.pathconf" | "posix.fpathconf") {
        return os::sysconf::pathconf(ctx, name, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if matches!(name, "posix.confstr" | "posix.sysconf") {
        return os::sysconf::call(ctx, name, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if let Some(op @ ("sched_get_priority_min" | "sched_get_priority_max")) =
        name.strip_prefix("posix.")
    {
        return os::scheduler::priority_bound(ctx, op, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "posix.sched_yield" {
        return os::scheduler::yield_cpu(args, kwargs);
    }
    #[cfg(target_os = "linux")]
    match name {
        "posix.sched_getaffinity" => return os::scheduler::affinity::get(ctx, args, kwargs),
        "posix.sched_setaffinity" => return os::scheduler::affinity::set(ctx, args, kwargs),
        "posix.sched_rr_get_interval" => return os::scheduler::rr_interval(ctx, args, kwargs),
        _ => {}
    }
    #[cfg(target_os = "linux")]
    if let Some(op @ ("sched_getparam" | "sched_setparam" | "sched_setscheduler")) =
        name.strip_prefix("posix.")
    {
        return os::scheduler::parameters::call(ctx, op, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "posix.sched_getscheduler" {
        return os::scheduler::get_policy(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if let Some(op @ ("major" | "minor" | "makedev")) = name.strip_prefix("posix.") {
        return os::device::call(ctx, op, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if let Some(op @ ("eventfd" | "eventfd_read" | "eventfd_write")) = name.strip_prefix("posix.") {
        return os::eventfd::call(ctx, op, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if let Some(op @ ("posix_spawn" | "posix_spawnp")) = name.strip_prefix("posix.") {
        return posix_spawn::call(ctx, op, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "posix.getgrouplist" {
        return grp::getgrouplist(ctx, args, kwargs);
    }
    if name == "posix.strerror" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "posix.strerror() takes no keyword arguments",
            ));
        }
        let [code] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "posix.strerror() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        let code = os::c_int(ctx, *code)?;
        return Ok(ctx.text_codepoints(crate::sys::strerror(code)?));
    }
    #[cfg(target_os = "linux")]
    if name == "posix.getrandom" {
        return os::getrandom(ctx, args, kwargs);
    }
    if name == "posix.urandom" {
        return os::urandom(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "posix.memfd_create" {
        return os::memfd_create(ctx, args, kwargs);
    }
    if name == "posix.listdir" {
        return os::listdir(ctx, args, kwargs);
    }
    if matches!(name, "posix.putenv" | "posix.unsetenv") {
        return os::change_environment(ctx, name, args, kwargs);
    }
    if name == "array.array" {
        return super::array::new(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("array.array.") {
        return super::array::method(ctx, method, args, kwargs);
    }
    if let Some(operation) = name.strip_prefix("_imre_pkgutil.") {
        return pkgutil::call(ctx, operation, args, kwargs);
    }
    if name == "_imre_tokenize.scan" {
        return tokenize::scan(ctx, args, kwargs);
    }
    if matches!(name, "_imre_base32.encode" | "_imre_base32.decode") {
        return binascii::base32(ctx, args, kwargs, name == "_imre_base32.decode");
    }
    if name == "_imre_base85.encode" {
        return binascii::encode85(ctx, args, kwargs);
    }
    if name == "_imre_csv.read_row" {
        return csv::read_row(ctx, args, kwargs);
    }
    if let Some(op) = name.strip_prefix("fnmatch.") {
        return fnmatch::call(ctx, op, args, kwargs);
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
    if name == "itertools.count" {
        return itertools::count(ctx, args, kwargs);
    }
    if name == "_imre_fractions._hash_function" {
        if let &[function] = args
            && kwargs.is_empty()
            && matches!(ctx.heap.get(function), Value::Function { .. })
        {
            return Ok(ctx.alloc(Value::FractionHash(function)));
        }
        return Err(Error::new(
            "TypeError",
            "_hash_function requires a function",
        ));
    }
    if name == "_imre_fractions._limit_candidates" {
        return math::fraction_limit_candidates(ctx, args, kwargs);
    }
    if name == "_imre_itertools._accumulate_allocate" {
        return itertools::accumulate_allocate(ctx, args, kwargs);
    }
    if name == "_imre_itertools._accumulate_next" {
        return itertools::accumulate_next(ctx, args, kwargs);
    }
    if name == "_imre_itertools._tee_allocate" {
        return itertools::tee_allocate(ctx, args, kwargs);
    }
    if name == "_imre_itertools._tee_next" {
        return itertools::tee_next(ctx, args, kwargs);
    }
    if name == "_imre_itertools._cycle_allocate" {
        return itertools::cycle_allocate(ctx, args, kwargs);
    }
    if name == "_imre_itertools._cycle_next" {
        return itertools::cycle_next(ctx, args, kwargs);
    }
    if name == "_imre_itertools._pairwise_allocate" {
        return itertools::pairwise_allocate(ctx, args, kwargs);
    }
    if name == "_imre_itertools._pairwise_next" {
        return itertools::pairwise_next(ctx, args, kwargs);
    }
    if name == "_imre_itertools._starmap_allocate" {
        return itertools::starmap_allocate(ctx, args, kwargs);
    }
    if name == "_imre_itertools._starmap_next" {
        return itertools::starmap_next(ctx, args, kwargs);
    }
    if name == "_imre_itertools._predicate_allocate" {
        return itertools::predicate_allocate(ctx, args, kwargs);
    }
    if name == "_imre_itertools._predicate_state" {
        return itertools::iterator_state(ctx, args, kwargs, "predicate iterator");
    }
    if name == "_imre_itertools._zip_longest_allocate" {
        return itertools::zip_longest_allocate(ctx, args, kwargs);
    }
    if name == "_imre_itertools._zip_longest_state" {
        return itertools::iterator_state(ctx, args, kwargs, "zip_longest");
    }
    if name == "_imre_itertools._uses_base_init" {
        if let [base, class] = args
            && kwargs.is_empty()
            && ctx.is_type_object(*base)
            && ctx.is_type_object(*class)
        {
            return Ok(builtins::boolean(itertools::uses_base_init(
                ctx, *base, *class,
            )));
        }
        return Err(Error::new("TypeError", "expected two classes"));
    }
    if name == "_imre_itertools._chain_allocate" {
        let [base, class, source] = args else {
            return Err(Error::new(
                "TypeError",
                "invalid chain allocation arguments",
            ));
        };
        let valid = matches!(ctx.heap.get(*class), Value::Class { ancestors, .. }
            if *class == *base || ancestors.contains(base));
        if !valid {
            return Err(Error::new("TypeError", "expected a chain subclass"));
        }
        // A subclass initializer may consume extra keywords after allocation.
        if !kwargs.is_empty() && itertools::uses_base_init(ctx, *base, *class) {
            return Err(Error::new(
                "TypeError",
                "chain() takes no keyword arguments",
            ));
        }
        let result = ctx.instance(*class, None);
        if let Value::Instance { fields, .. } = ctx.heap.get_mut(result) {
            fields.insert("_sources".to_owned(), *source);
            fields.insert("_active".to_owned(), NONE);
        }
        return Ok(result);
    }
    if name == "_imre_itertools._chain_next" {
        return itertools::chain_next(ctx, args, kwargs);
    }
    if let Some(class) = super::deque::iterator_class(name) {
        return super::deque::construct_iterator(ctx, class, args);
    }
    if let Some((class, method)) = name.rsplit_once('.')
        && super::deque::iterator_class(class).is_some()
        && super::deque::ITERATOR_METHODS.contains(&method)
    {
        return super::deque::iterator_method(ctx, class, method, args, kwargs);
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
    if let Some((owner, method)) = name.rsplit_once('.')
        && let Some(owner) = itertools::cursor_type(owner)
    {
        return itertools::method(ctx, owner, method, args, kwargs);
    }
    if name == "types._coroutine_flags" {
        use imre_ast::code_flags::{COROUTINE, GENERATOR, ITERABLE_COROUTINE};
        let [code] = required_arguments("_coroutine_flags", ["code"], args, kwargs)?;
        if let Value::Code(code) = ctx.heap.get(code) {
            // Source functions track execution kind and the iterable-coroutine
            // bit even when their full public co_flags value is unavailable.
            let flags = code.definition.flags & (COROUTINE | GENERATOR | ITERABLE_COROUTINE);
            return Ok(ctx.int(i64::from(flags)));
        }
        return ctx.lookup_attr(code, "co_flags");
    }
    if name == "types._mark_generator" {
        let [func, code] = *args else {
            return Err(Error::new(
                "TypeError",
                "_mark_generator expects a function and code",
            ));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "_mark_generator accepts no keyword arguments",
            ));
        }
        let Value::Code(original) = ctx.heap.get(code) else {
            return Err(Error::new(
                "TypeError",
                "_mark_generator expects a code object",
            ));
        };
        if matches!(original.body, super::value::Body::Native(..)) {
            return Err(Error::new(
                "NotImplementedError",
                "cannot mark this native generator function",
            ));
        }
        let mut marked = original.clone();
        marked.freevar_names = NONE;
        marked.definition.flags |= imre_ast::code_flags::ITERABLE_COROUTINE;
        let marked = ctx.alloc(Value::Code(marked));
        let func = ctx.weak_proxy_target(func)?;
        if let Value::Function { code: current, .. } = ctx.heap.get_mut(func)
            && *current == code
        {
            // Changing only a flag preserves this nested function's closure layout.
            *current = marked;
        } else {
            ctx.set_attr(func, "__code__", marked)?;
        }
        return Ok(NONE);
    }
    if let Some(function) = name.strip_prefix("inspect.") {
        return inspect::call(ctx, function, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_imp.") {
        return ctx.imp_call(method, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_imre_importlib.") {
        match (method, args, kwargs) {
            ("source_name", [path, prefix, suffixes, join], []) => {
                use crate::objects::cache_paths;
                let prefix = (*prefix != NONE)
                    .then(|| ctx.str_value(*prefix))
                    .transpose()?;
                let (dir, base) = cache_paths::source_parts(ctx.str_value(*path)?, prefix)?;
                let (dir, base) = (dir.to_owned(), base.to_owned());
                match cache_filename(ctx, *suffixes, base)? {
                    CacheFilename::Text(name) => {
                        let result = cache_paths::source_location(&dir, &name);
                        return Ok(ctx.text(result.to_string()));
                    }
                    CacheFilename::Object(name) => {
                        let head = ctx.text(dir);
                        return ctx.call(*join, vec![head, name], Vec::new());
                    }
                }
            }
            ("warn_legacy_load", [source_globals], []) => {
                ctx.warn_legacy_load(*source_globals)?;
                return Ok(NONE);
            }
            ("warn_loader_deprecation", [globals, mismatch, frozen], []) => {
                let (line, message) = if ctx.truth(*mismatch)? {
                    (670, "Module globals; __loader__ != __spec__.loader")
                } else {
                    (663, "Module globals is missing a __spec__.loader")
                };
                let frozen = ctx.truth(*frozen)?;
                ctx.warn_bootstrap_loader(*globals, line, message, frozen)?;
                return Ok(NONE);
            }
            ("warn_debug_override", [globals, frozen], []) => {
                let frozen = ctx.truth(*frozen)?;
                ctx.warn_bootstrap_loader(
                    *globals,
                    258,
                    crate::objects::cache_paths::DEBUG_OVERRIDE_WARNING,
                    frozen,
                )?;
                return Ok(NONE);
            }
            ("native_code", [name, bundled], []) => {
                return ctx.native_code(*name, *bundled == TRUE);
            }
            ("native_source", [name, bundled], []) => {
                return ctx.native_source(*name, *bundled == TRUE);
            }
            ("native_spec_info", [name, path, bundled], []) => {
                let name = ctx.str_value(*name)?.to_owned();
                return ctx.native_spec_info(&name, *path, *bundled == TRUE);
            }
            ("registered_path", [path], []) => return ctx.registered_path(*path),
            ("registered_path_spec", [name, path], []) => {
                return ctx.registered_path_spec(*name, *path);
            }
            ("frozen_info" | "frozen_code", [name], []) => {
                return if method == "frozen_info" {
                    let name = ctx.str_value(*name)?.to_owned();
                    Ok(ctx.frozen_info(&name))
                } else {
                    ctx.frozen_code(*name)
                };
            }
            ("builtin_names_contains", [sys, key], []) => {
                return Ok(super::builtins::boolean(
                    ctx.builtin_names_contains(*sys, *key)?,
                ));
            }
            ("is_builtin_module", [name], []) => {
                return Ok(builtins::boolean(
                    ctx.is_builtin_module(ctx.str_value(*name)?),
                ));
            }
            ("create_builtin_module", [name], []) => {
                let name = ctx.str_value(*name)?.to_owned();
                return ctx.create_builtin_module(&name);
            }
            ("exec_builtin_module", [module], []) => return ctx.exec_builtin_module(*module),
            ("create_native_module", [name], []) => {
                let name = ctx.str_value(*name)?.to_owned();
                return ctx.create_native_module(&name);
            }
            ("exec_native_module", [module], []) => return ctx.exec_native_module(*module),
            ("warn_import", [globals, message, line] | [globals, message, line, _], []) => {
                let message = ctx.str_value(*message)?.to_owned();
                let line = ctx
                    .index_integer(*line)?
                    .to_i64()
                    .and_then(|line| u32::try_from(line).ok())
                    .ok_or_else(|| Error::new("OverflowError", "invalid warning line"))?;
                ctx.warn_import(*globals, &message, line, args.get(3).map(|id| *id == TRUE))?;
                return Ok(NONE);
            }
            ("import_module", [key] | [key, _] | [key, _, _], []) => {
                let name = ctx.str_value(*key)?.to_owned();
                let source_globals = args.get(1).copied().unwrap_or(NONE);
                return ctx
                    .find_module_from(&name, *key, source_globals, args.get(2).copied())?
                    .ok_or_else(|| Context::missing_module(&name, *key));
            }
            ("find_spec", [fullname, original, finder], []) => {
                return ctx.find_spec(*fullname, *original, false, Some(*finder));
            }
            ("namespace_locations", [name, path], []) => {
                let name = ctx.str_value(*name)?.to_owned();
                return ctx.namespace_locations(&name, *path);
            }
            ("get_source_data", [loader, filename], []) => {
                let data = ctx.registered_source_data(*loader, *filename);
                return Ok(data.map_or(NONE, |bytes| ctx.alloc(Value::Bytes(bytes.to_vec()))));
            }
            ("compile_filename", [path, optimize], []) => {
                return Ok(compile_operands(ctx, *path, None, Some(*optimize))?.0);
            }
            ("uses_builtin_compile", [converter], []) => {
                return Ok(builtins::boolean(ctx.uses_builtin_compile(*converter)));
            }
            ("bind_doctest", [test, module], []) => {
                ctx.bind_doctest_source(*test, *module);
                return Ok(NONE);
            }
            ("compile_single", [compiler, source, filename, test], []) => {
                if *compiler == ctx.builtin("compile")
                    && ctx.config["optimize"].as_i64().unwrap_or(0) < 2
                    && let Some(code) = registered_single_code(ctx, *source, *filename, *test)
                {
                    return Ok(code);
                }
                let mode = ctx.text("single");
                return ctx.call(*compiler, vec![*source, *filename, mode], Vec::new());
            }
            ("compile_file", [compiler, data, filename], options) => {
                let mode = ctx.text("exec");
                if *compiler == ctx.builtin("compile") {
                    return builtins::compile(ctx, &[*data, *filename, mode], options, true);
                }
                return ctx.call(*compiler, vec![*data, *filename, mode], options.to_vec());
            }
            ("open_run_path", [opener, path, filename], []) => {
                return io::open_run_path(ctx, *opener, *path, *filename);
            }
            ("get_code", [loader, path], []) => return ctx.registered_code(*loader, *path, None),
            ("get_code", [loader, path, name, read, convert], []) => {
                return ctx.registered_code(*loader, *path, Some((*name, *read, *convert)));
            }
            ("path_stats", [path], []) => return os::source_path_stats(ctx, *path),
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
            ("bind_initializer", args, []) => {
                return super::dataclass_init::bind_initializer(ctx, args);
            }
            ("is_defaultdict", [obj], []) => {
                // Boxed defaultdict exposes its factory on instances, including derived instances.
                return Ok(if ctx.defaultdict_factory(*obj).is_some() {
                    TRUE
                } else {
                    FALSE
                });
            }
            ("loaded_module", [name], []) => {
                return Ok(ctx.public_registered_module(*name)?.unwrap_or(NONE));
            }
            ("retarget_class_cell", [method, old, new], []) => {
                // Repair the method's shared __class__ cell only while it still owns the old type.
                if let Value::Function {
                    class_cell: Some(cell),
                    code,
                    ..
                } = *ctx.heap.get(*method)
                    && matches!(ctx.heap.get(cell), Value::Cell(Some(owner)) if owner == old)
                {
                    let parameters = ctx.code_object(code).parameters;
                    let sig: serde_json::Value =
                        serde_json::from_str(ctx.str_value(parameters)?)
                            .map_err(|e| Error::new("SystemError", e.to_string()))?;
                    if sig["class_cell"] == true {
                        ctx.heap.replace_cell(cell, Some(*new));
                        return Ok(TRUE);
                    }
                }
                return Ok(FALSE);
            }
            ("native_iterator", [obj], []) => {
                return Ok(if ctx.is_native_iterator(*obj) {
                    TRUE
                } else {
                    FALSE
                });
            }
            ("slot_bases", [class], []) => return ctx.dataclass_slot_bases(*class),
            ("slotted_type", [old, namespace], []) => {
                return ctx.slotted_dataclass(*old, *namespace);
            }
            ("members", [class] | [class, TRUE], []) => {
                return ctx.dataclass_members(*class, args.len() == 2);
            }
            _ => {}
        }
        return Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        ));
    }
    if let Some(operation) = name.strip_prefix("_imre_subprocess.") {
        return subprocess::call(ctx, operation, args, kwargs);
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
    if name == "_typing._idfunc" {
        return builtins::one_positional_arg(name, args, kwargs);
    }
    if let Some(operation) = name.strip_prefix("typing.") {
        return super::typing::call(ctx, operation, args, kwargs);
    }
    if name == "collections.abc._CallableGenericAlias" {
        return super::typing::abc_callable_alias(ctx, args, kwargs);
    }
    if let Some(owner @ ("types.GenericAlias" | "generic_alias_iterator")) =
        name.strip_suffix(".__reduce__")
    {
        let receiver = builtins::noargs_method_receiver(ctx, owner, "__reduce__", args, kwargs)?;
        return super::typing::reduce_builtin_alias(ctx, receiver);
    }
    if matches!(name, "types.GenericAlias" | "types.GenericAlias.__new__") {
        let base = ctx.builtin("types.GenericAlias");
        let (class, args) = if name.ends_with(".__new__") {
            let Some((&class, rest)) = args.split_first() else {
                return Err(Error::new(
                    "TypeError",
                    "types.GenericAlias.__new__(): not enough arguments",
                ));
            };
            if !ctx.subclass(class, base) {
                return Err(ctx.invalid_new_subtype("types.GenericAlias", class));
            }
            (class, rest)
        } else {
            (base, args)
        };
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
        let alias = super::typing::builtin_alias(ctx, *origin, *parameters);
        if class == base {
            return Ok(alias);
        }
        let instance = ctx.instance(class, None);
        let Value::Instance { scalar, .. } = ctx.heap.get_mut(instance) else {
            unreachable!()
        };
        *scalar = Some(alias);
        return Ok(instance);
    }
    if let Some(name) = name.strip_prefix("operator.") {
        if name == "attrgetter" {
            return operator::attrgetter(ctx, args, kwargs);
        }
        if name == "itemgetter" {
            return operator::itemgetter(ctx, args, kwargs);
        }
        if name == "methodcaller" {
            return operator::methodcaller(ctx, args, kwargs);
        }
        if name == "attrgetter.__reduce__" {
            return operator::reduce_attrgetter(ctx, args, kwargs);
        }
        if name == "methodcaller.__reduce__" {
            return operator::reduce_methodcaller(ctx, args, kwargs);
        }
        if name == "itemgetter.__reduce__" {
            return operator::reduce_itemgetter(ctx, args, kwargs);
        }
        if let Some(op) = operator::Operation::named(name) {
            return op.call(ctx, name, args, kwargs);
        }
    }
    #[cfg(target_os = "linux")]
    if name == "mmap.mmap" {
        return mmap::create(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if let Some(method) = name.strip_prefix("mmap.mmap.") {
        return mmap::call(ctx, method, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if name == "select.epoll" {
        return epoll::create(ctx, args, kwargs);
    }
    #[cfg(target_os = "linux")]
    if let Some(method) = name.strip_prefix("select.epoll.") {
        return epoll::call(ctx, method, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("select.") {
        return select::call(ctx, method, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("selectors.") {
        return readiness::call(ctx, method, args, kwargs);
    }
    if matches!(name, "enum.auto.__init__" | "enum.auto.__repr__") {
        return ctx.auto_method(name, args, kwargs);
    }
    if matches!(
        name,
        "enum.Enum.__repr__" | "enum.Flag.__repr__" | "enum.Enum.__str__" | "enum.Enum.__format__"
    ) {
        return ctx.float_enum_display(name, args, kwargs);
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
    if name == "enum.Enum.__reduce_ex__" {
        let [member, _] =
            builtins::bind_builtin_arguments("__reduce_ex__", args, kwargs, ["self", "proto"], 2)?;
        let member = member.unwrap();
        let class = ctx.lookup_attr(member, "__class__")?;
        // Both lookups may run Python; keep the first result live through the second.
        let mark = ctx.heap.temporary_mark();
        ctx.heap.keep(class);
        let value = ctx.lookup_attr(member, "_value_");
        ctx.heap.release_temporaries_since(mark);
        let arguments = ctx.alloc(Value::Tuple(vec![value?]));
        return Ok(ctx.alloc(Value::Tuple(vec![class, arguments])));
    }
    if name == "enum.Flag._missing_" {
        let [class, value] =
            builtins::bind_builtin_arguments("_missing_", args, kwargs, ["cls", "value"], 2)?;
        return ctx.flag_missing(class.unwrap(), value.unwrap());
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
    if name == "_codecs._imre_idna_label" {
        return idna::label(ctx, args, kwargs);
    }
    if name == "_codecs._imre_bind_module_search" {
        return codecs::bind_module_search(ctx, args, kwargs);
    }
    if name == "_codecs._imre_codec_functions" {
        return codecs::entry_functions(ctx, args, kwargs);
    }
    if name == "encodings.search_function" {
        return codecs::native_search(ctx, args, kwargs);
    }
    if let Some(function) = name.strip_prefix("codecs.") {
        return match function {
            "encode" | "decode" => codecs::transform(ctx, function, args, kwargs),
            "escape_decode" => codecs::escape_decode(ctx, args, kwargs),
            "utf_8_decode" => codecs::utf8_decode(ctx, args, kwargs),
            "lookup" => codecs::lookup(ctx, args, kwargs),
            "register" | "unregister" => codecs::search_registry(ctx, function, args, kwargs),
            "register_error" | "lookup_error" => {
                codecs::error_registry(ctx, function, args, kwargs)
            }
            _ => match codecs::WideFunction::named(function) {
                Some(wide) => wide.call(ctx, function, args, kwargs),
                None => codecs::builtin_handler(ctx, function, args, kwargs),
            },
        };
    }
    #[cfg(any(all(target_os = "linux", target_env = "gnu"), target_os = "macos"))]
    if name == "_socket.gethostbyname_ex" {
        return socket::call(ctx, "gethostbyname_ex", args, kwargs);
    }
    if name == "_socket.getprotobyname" {
        return socket::getprotobyname(ctx, args, kwargs);
    }
    if name == "_socket.getaddrinfo" {
        return socket::getaddrinfo(ctx, args, kwargs);
    }
    if matches!(
        name,
        "_socket.htons" | "_socket.ntohs" | "_socket.htonl" | "_socket.ntohl"
    ) {
        return socket::convert_byte_order(ctx, name, args, kwargs);
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(function @ ("CMSG_LEN" | "CMSG_SPACE")) = name.strip_prefix("_socket.") {
        return socket::cmsg_size(ctx, function, args, kwargs);
    }
    if name == "_socket.inet_aton" {
        return socket::inet_aton(ctx, args, kwargs);
    }
    if name == "_socket.inet_pton" {
        return socket::inet_pton(ctx, args, kwargs);
    }
    if matches!(name, "_socket.inet_ntoa" | "_socket.inet_ntop") {
        return socket::format_packed_address(ctx, name, args, kwargs);
    }
    if name == "_socket.socketpair" {
        return socket::socketpair(ctx, args, kwargs);
    }
    if name == "_socket.setdefaulttimeout" {
        return socket::setdefaulttimeout(ctx, args, kwargs);
    }
    if name == "_socket.close" {
        return socket::close(ctx, args, kwargs);
    }
    if let Some(name) = name.strip_prefix("socket.") {
        return socket::call(ctx, name, args, kwargs);
    }
    if let Some(class) = super::typing::generic_alias_method_owner(name) {
        let Some((&owner, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!("descriptor '__class_getitem__' of '{class}' object needs an argument"),
            ));
        };
        datetime::validate_classmethod_receiver(ctx, class, "__class_getitem__", owner)?;
        let qualified = format!("{}.__class_getitem__", ctx.class_qualname(owner));
        let index = builtins::one_positional_arg(&qualified, rest, kwargs)?;
        return Ok(super::typing::builtin_alias(ctx, owner, index));
    }
    if let Some(method) = name.strip_prefix("_contextvars.") {
        return contextvars::call(ctx, method, args, kwargs);
    }
    if name == "getset_descriptor.__get__" {
        return hashlib::descriptor_get(ctx, args, kwargs);
    }
    if let Some((class, method)) = name.rsplit_once('.')
        && crate::objects::HashState::is_provider_class(class)
    {
        return hashlib::provider_method(ctx, class, method, args, kwargs);
    }
    if let Some(class) = crate::objects::HashState::provider_class(name) {
        return hashlib::hash(ctx, class.rsplit_once('.').unwrap().1, args, kwargs);
    }
    if matches!(
        name,
        "_operator._compare_digest" | "_hashlib.compare_digest"
    ) {
        return hmac::compare(ctx, name, args, kwargs);
    }
    #[cfg(unix)]
    if let Some(function) = name.strip_prefix("_ssl.") {
        return ssl::provider::objects::call(ctx, function, args, kwargs);
    }
    #[cfg(unix)]
    if let Some(function) = name.strip_prefix("_imre_ssl.") {
        return ssl::call(ctx, function, args, kwargs);
    }
    if let Some(function) = name.strip_prefix("_hashlib.") {
        return hashlib::call_openssl(ctx, function, args, kwargs);
    }
    if let Some(algorithm) = name.strip_prefix("hashlib.")
        && crate::objects::HashState::algorithms().any(|name| name == algorithm)
    {
        return hashlib::hash(ctx, algorithm, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("_imre_gzip.") {
        return gzip::call(ctx, method, args, kwargs);
    }
    if let Some(name) = name.strip_prefix("_imre_tempfile.") {
        return tempfile::path_operation(ctx, name, args, kwargs);
    }
    if name == "tempfile.TemporaryFile" {
        return tempfile::open(ctx, args, kwargs);
    }
    if let Some(method) = name.strip_prefix("BufferedRandom.") {
        let receiver = argument(args, 0)?;
        return tempfile::method(ctx, receiver, method, &args[1..], kwargs);
    }
    if matches!(
        name,
        "gc.enable" | "gc.disable" | "gc.isenabled" | "gc.get_threshold"
    ) {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no keyword arguments"),
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no arguments ({} given)", args.len()),
            ));
        }
        if name == "gc.get_threshold" {
            let values = ctx.heap.gc_thresholds().map(|n| ctx.int(i64::from(n)));
            return Ok(ctx.alloc(Value::Tuple(values.into())));
        }
        let enabled = &ctx.heap.gc_settings.enabled;
        if name == "gc.isenabled" {
            return Ok(builtins::boolean(
                enabled.load(std::sync::atomic::Ordering::Relaxed),
            ));
        }
        enabled.store(name == "gc.enable", std::sync::atomic::Ordering::Relaxed);
        return Ok(NONE);
    }
    if name == "gc.set_threshold" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "set_threshold() takes no keyword arguments",
            ));
        }
        if !(1..=3).contains(&args.len()) {
            return Err(Error::new(
                "TypeError",
                "gc.set_threshold requires 1 to 3 arguments",
            ));
        }
        let mut values = [0; 3];
        for (slot, &arg) in values.iter_mut().zip(args) {
            *slot = ctx.index_c_int(arg)?;
        }
        ctx.heap.set_gc_thresholds(&values[..args.len()]);
        return Ok(NONE);
    }
    if name == "gc.is_tracked" {
        let obj = builtins::one_positional_arg(name, args, kwargs)?;
        return Ok(builtins::boolean(ctx.heap.is_gc_tracked(obj)));
    }
    if name == "gc.collect" {
        let generation = if args.is_empty() && kwargs.is_empty() {
            2
        } else {
            let [value] = required_arguments("collect", ["generation"], args, kwargs)?;
            let generation = ctx
                .index_integer(value)?
                .to_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C int")
                })?;
            if !(0..=2).contains(&generation) {
                return Err(Error::new("ValueError", "invalid generation"));
            }
            generation as u8
        };
        let reclaimed = ctx.collect_generation(generation)?;
        return Ok(ctx.int(reclaimed as i64));
    }
    if name == "marshal.dumps" {
        return marshal::dumps(ctx, args, kwargs);
    }
    if name == "marshal.loads" {
        return marshal::loads(ctx, args, kwargs);
    }
    if name == "_pickle.dumps" {
        return pickle::dumps(ctx, args, kwargs);
    }
    if name == "functools._PlaceholderType" {
        if !args.is_empty() || !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "PlaceholderType takes no arguments",
            ));
        }
        return Ok(functools::placeholder(ctx));
    }
    if name == "functools.partial.__reduce__" {
        let receiver = builtins::noargs_method_receiver(
            ctx,
            functools::PARTIAL_CLASS,
            "__reduce__",
            args,
            kwargs,
        )?;
        return functools::partial_reduce(ctx, receiver);
    }
    if name == "functools.partial.__setstate__" {
        return functools::partial_setstate(ctx, args, kwargs);
    }
    if name == "functools.partial.__repr__" {
        let receiver = builtins::noargs_method_receiver(
            ctx,
            functools::PARTIAL_CLASS,
            "__repr__",
            args,
            kwargs,
        )?;
        return functools::partial_repr(ctx, receiver);
    }
    if name == "functools.partial.__call__" {
        let Some((&receiver, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "descriptor '__call__' of 'functools.partial' object needs an argument",
            ));
        };
        let partial = ctx.scalar_value(receiver);
        if !matches!(ctx.heap.get(partial), Value::Partial { .. }) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '__call__' requires a 'functools.partial' object but received a '{}'",
                    ctx.type_name(receiver)
                ),
            ));
        }
        return ctx.call(partial, rest.to_vec(), kwargs.to_vec());
    }
    if matches!(name, functools::PARTIAL_CLASS | "functools.partial.__new__") {
        let base = ctx.builtin(functools::PARTIAL_CLASS);
        let (class, args) = if name == "functools.partial.__new__" {
            let Some((&class, rest)) = args.split_first() else {
                return Err(Error::new(
                    "TypeError",
                    "functools.partial.__new__(): not enough arguments",
                ));
            };
            if !ctx.direct_subclass(class, base) {
                return Err(ctx.invalid_new_subtype(functools::PARTIAL_CLASS, class));
            }
            (class, rest)
        } else {
            (base, args)
        };
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
        // Flatten a nested partial only until its own instance dictionary is materialized.
        let inner = match ctx.heap.get(function) {
            Value::Instance {
                scalar: Some(storage),
                namespace: None,
                ..
            } => *storage,
            _ => function,
        };
        let (function, mut args, keywords) = match ctx.heap.get(inner).clone() {
            Value::Partial {
                function,
                args,
                keywords,
                dictionary: None,
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
        let partial = ctx.alloc(Value::Partial {
            function,
            args,
            keywords,
            dictionary: None,
        });
        if class == base {
            return Ok(partial);
        }
        let object = ctx.instance(class, None);
        let Value::Instance { scalar, .. } = ctx.heap.get_mut(object) else {
            unreachable!()
        };
        *scalar = Some(partial);
        return Ok(object);
    }
    if let Some(method @ ("digest" | "hexdigest")) = name.strip_prefix("hash.") {
        return hashlib::digest(ctx, method, args, kwargs);
    }
    if name == "hashlib._file_digest_readinto" {
        let [file, digest, size] =
            required_arguments(name, ["file", "digest", "size"], args, kwargs)?;
        return hashlib::readinto(ctx, file, digest, size);
    }
    if name == "hashlib.new" {
        return hashlib::new(ctx, args, kwargs);
    }
    if let Some(method @ ("match" | "fullmatch" | "search" | "scanner" | "finditer" | "findall")) =
        name.strip_prefix("pattern.")
    {
        return regex::search(ctx, method, args, kwargs);
    }
    if let Some(method @ ("sub" | "subn")) = name.strip_prefix("pattern.") {
        return regex::pattern_sub(ctx, method, args, kwargs);
    }
    if let Some((owner @ (value::REGEX_PATTERN_TYPE | value::REGEX_MATCH_TYPE), method)) =
        name.rsplit_once('.')
        && value::REGEX_COPY_METHODS.contains(&method)
    {
        let (receiver, rest) = builtins::method_receiver_args(ctx, owner, method, args, kwargs)?;
        let qualified = name.strip_prefix("re.").unwrap();
        if method == "__deepcopy__" {
            builtins::one_positional_arg(qualified, rest, &[])?;
        } else if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{qualified}() takes no arguments ({} given)", rest.len()),
            ));
        }
        return Ok(receiver);
    }
    if let Some((owner @ functools::LRU_CACHE_CLASS, method)) = name.rsplit_once('.')
        && functools::LRU_METHODS.contains(&method)
    {
        if matches!(method, "__copy__" | "__deepcopy__") {
            let (wrapper, _) = builtins::method_receiver_args(ctx, owner, method, args, kwargs)?;
            return Ok(wrapper);
        }
        let wrapper = builtins::noargs_method_receiver(ctx, owner, method, args, kwargs)?;
        return if method == "__reduce__" {
            ctx.lookup_attr(wrapper, "__qualname__")
        } else {
            functools::cache_method(ctx, method, wrapper)
        };
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
        "functools.cmp_to_key" => {
            let comparator = functools::key_argument("cmp_to_key", "mycmp", &args[1..], kwargs)?;
            return Ok(ctx.alloc(Value::KeyWrapper {
                comparator,
                object: None,
            }));
        }
        "_functools.reduce" => return functools::reduce(ctx, &args[1..], kwargs),
        "functools._reduce_iterator" => (3, &[]),
        "functools._cache_wrapper" => (3, &[]),
        "_json.scanstring" => return json::scanstring(ctx, args, kwargs),
        "_json.encode_basestring" | "_json.encode_basestring_ascii" => {
            return json::encode_basestring(ctx, name, args, kwargs);
        }
        "_imre_json.encoder_state" => return json::encoder_state(ctx, args, kwargs),
        "_imre_json.encoder_call" => return json::encoder_call(ctx, args, kwargs),
        "_imre_json.surrogate_text" => (1, &[]),
        "_imre_json.is_constants_lookup" => (2, &[]),
        "_imre_json.scan_once" => (3, &[]),
        "_imre_json.scan_with_pairs_hook" => (4, &[]),
        "hmac._digest_native" => (3, &[]),
        "hmac.new" | "hmac.HMAC" => (3, &["key", "msg", "digestmod"]),
        "hmac.HMAC.update" => (2, &[]),
        "hmac.HMAC.digest" | "hmac.HMAC.hexdigest" | "hmac.HMAC.copy" => (1, &[]),
        "hash.update" => (2, &[]),
        "hash.copy" => (1, &[]),
        "file.writelines" | "_IOBase.writelines" => {
            let receiver = argument(args, 0)?;
            return io::lines(
                ctx,
                receiver,
                name.strip_prefix("_IOBase.").unwrap_or(name),
                &args[1..],
                kwargs,
            );
        }
        "file.__exit__" => (4, &[]),
        "struct.unpack" => (2, &[]),
        "match.expand" => return regex::match_expand(ctx, args, kwargs),
        "match.group" => (usize::MAX, &[]),
        "collections.defaultdict.__missing__" => (2, &[]),
        "match.groups" | "match.groupdict" => (2, &["default"]),
        "match.start" | "match.end" | "match.span" => (2, &[]),
        "scanner.match" | "scanner.search" => (1, &[]),
        "sys.exc_info" | "sys.exception" => (0, &[]),
        "sys.exit"
        | "site._get_path"
        | "os.confstr"
        | "struct.calcsize"
        | "_string.formatter_parser"
        | "_string.formatter_field_name_split"
        | "collections.defaultdict.copy"
        | "collections.defaultdict.__copy__"
        | "collections.defaultdict.__reduce__"
        | "file.__enter__" => (1, &[]),
        _ => (0, &[]),
    };
    if !urlsafe
        && (args.len() > max_args
            || kwargs
                .iter()
                .any(|(key, _)| !keywords.contains(&key.as_str())))
    {
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
        _ if urlsafe => {
            let [id] = required_arguments(name.trim_start_matches("base64."), ["s"], args, kwargs)?;
            let decode = name == "base64.urlsafe_b64decode";
            let (id, urlsafe) = if decode
                && matches!(ctx.heap.get(id), Value::Instance { .. })
                && ctx.string_storage(id).is_some()
            {
                urlsafe_input(ctx, id)?
            } else {
                (id, true)
            };
            let gathered;
            let bytes = match ctx.heap.get(id) {
                _ if !decode => ctx.native_buffer_bytes(id)?,
                Value::MemoryView { .. } => {
                    gathered = ctx.memoryview_bytes(id)?;
                    gathered.as_slice()
                }
                Value::Str(text) if text.is_ascii() => text.as_bytes(),
                Value::Str(_) => {
                    return Err(Error::new(
                        "ValueError",
                        "string argument should contain only ASCII characters",
                    ));
                }
                _ => ctx.native_buffer_bytes(id).map_err(|error| {
                    if error.kind == "TypeError" {
                        let ty = ctx.type_name(id);
                        Error::new(
                            "TypeError",
                            format!(
                                "argument should be a bytes-like object or ASCII string, not '{ty}'"
                            ),
                        )
                    } else {
                        error
                    }
                })?,
            };
            let result = if decode {
                crate::objects::base64_decode(bytes, urlsafe)?
            } else {
                crate::objects::urlsafe_encode(bytes)?
            };
            ctx.alloc(Value::Bytes(result))
        }
        "hmac._digest_native" => return hmac::digest_native(ctx, args),
        "hmac.new" | "hmac.HMAC" => return hmac::new(ctx, args, kwargs),
        method if method.starts_with("hmac.HMAC.") => return hmac::method(ctx, method, args),
        "hash.update" | "hash.copy" => {
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
        #[cfg(not(target_os = "linux"))]
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
        "sysconfig.get_path_names" => tuple_text(ctx, system::PATH_NAMES),
        "sysconfig._getuserbase" => match system::InterpreterLayout::user_base() {
            Some(base) => {
                ctx.text_codepoints(system::fsdecode(base.as_os_str().as_encoded_bytes()).collect())
            }
            None => NONE,
        },
        "sysconfig.get_platform" => ctx.text(system::sysconfig_platform().map_err(os_error)?),
        "site._get_path" => {
            let base = ctx.str_value(argument(args, 0)?)?;
            let path = layout(ctx)?.user_site(Path::new(base));
            path_text(ctx, &path)?
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
            let (_, layout) = structure::parse(ctx, argument(args, 0)?)?;
            ctx.int(layout.size as i64)
        }
        "struct.unpack" => {
            let (_, layout) = structure::parse(ctx, argument(args, 0)?)?;
            structure::unpack(ctx, &layout, argument(args, 1)?)?
        }
        "_imre_json.surrogate_text" => {
            let storage = ctx.scalar_value(argument(args, 0)?);
            if matches!(ctx.heap.get(storage), Value::SurrogateStr(_)) {
                storage
            } else {
                NONE
            }
        }
        "_imre_json.is_constants_lookup" => builtins::boolean(json::is_constants_lookup(
            ctx,
            argument(args, 0)?,
            argument(args, 1)?,
        )),
        "_imre_json.scan_once" => json::scan_once(
            ctx,
            argument(args, 0)?,
            argument(args, 1)?,
            argument(args, 2)?,
            NONE,
        )?,
        "_imre_json.scan_with_pairs_hook" => json::scan_once(
            ctx,
            argument(args, 0)?,
            argument(args, 2)?,
            argument(args, 3)?,
            argument(args, 1)?,
        )?,
        "functools._reduce_iterator" => {
            let function = argument(args, 0)?;
            let iterator = functools::ReduceInput::PythonIterator(argument(args, 1)?);
            functools::fold(ctx, function, iterator, args.get(2).copied())?
        }
        "functools._cache_wrapper" => functools::make_cache(
            ctx,
            argument(args, 0)?,
            argument(args, 1)?,
            argument(args, 2)?,
        )?,
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
        "sys.exception" => ctx.heap.active_exception(),
        "scanner.match" | "scanner.search" => {
            return regex::scanner_next(ctx, args[0], name == "scanner.match");
        }
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
                    Value::Raised(error) if ctx.is_builtin_error(error, "SystemExit") => {
                        let mut error = error.clone();
                        error.tracing().instance = Some(*arg);
                        return Err(error);
                    }
                    _ => {}
                }
            }
            return Err(ctx.exception("SystemExit", args));
        }
        "file.__enter__" | "file.__exit__" => {
            let receiver = argument(args, 0)?;
            return files::binary_method(
                ctx,
                receiver,
                name.trim_start_matches("file."),
                &args[1..],
            );
        }
        "match.groups" | "match.groupdict" => {
            let receiver = argument(args, 0)?;
            let default = option(args, 1, kwargs, "default")?.unwrap_or(NONE);
            return if name == "match.groupdict" {
                regex::match_groupdict(ctx, receiver, default)
            } else {
                regex::match_groups(ctx, receiver, default)
            };
        }
        "match.group" | "match.start" | "match.end" | "match.span" => {
            return regex::match_method(ctx, name, args);
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
    fn equal_prompt_names_and_bytes_keep_their_compiled_module() {
        use crate::dynamic::{ffi, value::Code};

        unsafe extern "C" fn left(_: *mut Context, _: Id) -> Id {
            TRUE
        }
        unsafe extern "C" fn right(_: *mut Context, _: Id) -> Id {
            FALSE
        }

        let mut ctx = ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let filename = ctx.text("<doctest shared[0]>");
        let text = ctx.text("3\n");
        let object = ctx.builtin("object");
        let owners = [
            (c"left", c"left.py", left as Code),
            (c"right", c"right.py", right as Code),
        ];
        let tests = owners.map(|(name, file, entry)| {
            // SAFETY: both entries have the registered ABI, with static source bytes and names.
            unsafe {
                ffi::imre_dyn_register(&mut ctx, name.as_ptr(), file.as_ptr(), entry);
                ffi::imre_dyn_register_module_single(
                    &mut ctx,
                    name.as_ptr(),
                    c"<doctest shared[0]>".as_ptr(),
                    b"3\n".as_ptr(),
                    2,
                    0,
                    entry,
                );
            }
            let (module, scope, module_name, _) = ctx
                .allocate_import_module(name.to_str().unwrap(), &[], None)
                .unwrap();
            let path = ctx.text(file.to_str().unwrap());
            let test = ctx.instance(object, None);
            let Value::Instance { fields, .. } = ctx.heap.get_mut(test) else {
                unreachable!()
            };
            fields.insert("filename".into(), path);
            ctx.bind_doctest_source(test, module);
            assert!(registered_single_code(&mut ctx, text, filename, test).is_none());
            let code = ctx.native_module_code(entry, path, Some(0));
            ctx.heap
                .enter(scope, code, super::super::FrameOwner::Module(module_name));
            ctx.op("bind_doctest_module", scope, module_name, NONE)
                .unwrap();
            ctx.heap.leave(NONE, true);
            ctx.bind_doctest_source(test, module);
            (test, path)
        });

        for ((test, _), (_, _, entry)) in tests.into_iter().zip(owners) {
            let code = registered_single_code(&mut ctx, text, filename, test).unwrap();
            let value::Body::Native(actual, _) = ctx.code_object(code).body else {
                panic!("expected native prompt")
            };
            assert!(std::ptr::fn_addr_eq(actual, entry));
        }
        let Value::Instance { fields, .. } = ctx.heap.get_mut(tests[0].0) else {
            unreachable!()
        };
        fields.insert("filename".into(), tests[1].1);
        assert!(registered_single_code(&mut ctx, text, filename, tests[0].0).is_none());
    }

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
