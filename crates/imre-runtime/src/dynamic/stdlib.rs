//! Python system modules available to native functions with runtime-selected imports.

use std::path::Path;

use super::{
    Context,
    value::{Error, FALSE, Id, NONE, Result, TRUE, Value},
};
use crate::objects::interpreter_stdlib as system;

mod contextvars;
mod files;
mod json;
mod regex;

pub(super) fn is_module(name: &str) -> bool {
    matches!(
        name,
        "sys"
            | "sysconfig"
            | "os"
            | "os.path"
            | "platform"
            | "site"
            | "struct"
            | "json"
            | "re"
            | "importlib"
            | "importlib.machinery"
            | "functools"
            | "collections"
            | "enum"
            | "typing"
            | "contextlib"
            | "contextvars"
            | "warnings"
            | "subprocess"
    )
}

fn os_error(err: std::io::Error) -> Error {
    Error::new("OSError", err.to_string())
}

fn argument(args: &[Id], index: usize) -> Result<Id> {
    args.get(index)
        .copied()
        .ok_or_else(|| Error::new("TypeError", "missing required argument"))
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
        names: fields.iter().map(|(name, _)| (*name).to_owned()).collect(),
        values: fields.iter().map(|&(_, value)| value).collect(),
    })
}

fn instance(ctx: &mut Context, type_name: &str, fields: &[(&str, Id)]) -> Id {
    let class = ctx.alloc(Value::Native(type_name.to_owned()));
    ctx.alloc(Value::Instance {
        class,
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
    if matches!((module, name), ("importlib", "machinery") | ("os", "path")) {
        return ctx.module(&format!("{module}.{name}"));
    }
    let id = match (module, name) {
        ("sys", "argv") => ctx.argv,
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
        ("os", "name") => ctx.text("posix"),
        ("os" | "os.path", "sep") => ctx.text("/"),
        ("importlib.machinery", "EXTENSION_SUFFIXES") => {
            let suffix = ctx.text(
                ctx.config["archive_suffix"]
                    .as_str()
                    .unwrap_or(".imre.a")
                    .to_owned(),
            );
            ctx.alloc(Value::List(vec![suffix]))
        }
        ("subprocess", "PIPE") => ctx.int(-1),
        ("sys", "exit")
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
        | ("os", "confstr" | "fsdecode" | "uname")
        | ("os.path", "join" | "abspath" | "dirname")
        | (
            "platform",
            "system"
            | "node"
            | "release"
            | "version"
            | "machine"
            | "mac_ver"
            | "python_version"
            | "python_version_tuple"
            | "python_implementation",
        )
        | ("struct", "calcsize" | "unpack" | "error")
        | ("json", "dumps")
        | ("re", "match" | "compile")
        | ("typing", "IO" | "NamedTuple" | "Generator" | "Iterator" | "Sequence")
        | ("enum", "IntEnum")
        | ("functools", "lru_cache")
        | ("collections", "defaultdict")
        | ("contextlib", "contextmanager")
        | ("contextvars", "ContextVar" | "Context" | "Token" | "copy_context")
        | ("warnings", "warn")
        | ("subprocess", "run") => ctx.alloc(Value::Native(format!("{module}.{name}"))),
        _ => {
            return Err(Error::new(
                "AttributeError",
                format!("module '{module}' has no attribute '{name}'"),
            ));
        }
    };
    Ok(id)
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    // Reject unsupported options before a host primitive can silently use its default.
    if let Some(method) = name.strip_prefix("contextvars.") {
        return contextvars::call(ctx, method, args, kwargs);
    }
    let (max_args, keywords): (usize, &[&str]) = match name {
        "sysconfig.get_path" => (4, &["name", "scheme", "vars", "expand"]),
        "sysconfig.get_paths" => (3, &["scheme", "vars", "expand"]),
        "json.dumps" => (1, &["ensure_ascii"]),
        "subprocess.run" => (1, &["text", "capture_output", "stdout", "stderr", "check"]),
        "file.seek" => (3, &["whence"]),
        "file.__exit__" => (4, &[]),
        "struct.unpack" | "re.match" | "pattern.match" | "pattern.findall" | "file.read"
        | "warnings.warn" => (2, &[]),
        "match.group" | "os.path.join" => (usize::MAX, &[]),
        "sys.exit"
        | "sysconfig.get_config_var"
        | "sysconfig.get_preferred_scheme"
        | "os.confstr"
        | "os.fsdecode"
        | "os.path.abspath"
        | "os.path.dirname"
        | "struct.calcsize"
        | "re.compile"
        | "collections.defaultdict"
        | "functools.lru_cache"
        | "functools._cached"
        | "contextlib.contextmanager"
        | "file.close"
        | "file.tell"
        | "file.__enter__" => (1, &[]),
        _ => (0, &[]),
    };
    if args.len() > max_args
        || kwargs
            .iter()
            .any(|(key, _)| !keywords.contains(&key.as_str()))
    {
        return Err(Error::new(
            "TypeError",
            format!("unsupported arguments to {name}"),
        ));
    }
    let value = match name {
        "platform.python_implementation" => ctx.text("Imre"),
        "platform.python_version" => ctx.text(full_version(ctx)?),
        "platform.python_version_tuple" => {
            let version = full_version(ctx)?;
            tuple_text(ctx, &version.split('.').collect::<Vec<_>>())
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
            let expand = option(args, index + 2, kwargs, "expand")?.is_none_or(|id| ctx.truth(id));
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
        "os.fsdecode" => {
            let item = argument(args, 0)?;
            match ctx.heap.get(item) {
                Value::Str(_) => item,
                Value::Bytes(bytes) => {
                    let decoded: Option<String> =
                        system::fsdecode(bytes).map(char::from_u32).collect();
                    ctx.text(decoded.ok_or_else(|| {
                        Error::new(
                            "UnicodeError",
                            "surrogate-escaped filenames are not representable as Imre strings",
                        )
                    })?)
                }
                _ => return Err(Error::new("TypeError", "expected a path string or bytes")),
            }
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
            let items = values
                .into_iter()
                .map(|n| ctx.alloc(Value::Int(crate::Int::from_usize(n as usize))))
                .collect();
            ctx.alloc(Value::Tuple(items))
        }
        "json.dumps" => {
            let ascii = kwargs.first().is_none_or(|&(_, id)| ctx.truth(id));
            ctx.text(json::dumps(ctx, argument(args, 0)?, ascii)?)
        }
        "functools.lru_cache" => {
            if args.is_empty() && kwargs.is_empty() {
                ctx.alloc(Value::Native("functools._cached".into()))
            } else if args.len() == 1
                && matches!(ctx.heap.get(args[0]), Value::Function { .. })
                && kwargs.is_empty()
            {
                ctx.alloc(Value::Cached {
                    function: args[0],
                    entries: Vec::new(),
                })
            } else {
                return Err(Error::new(
                    "NotImplementedError",
                    "custom lru_cache parameters are unsupported",
                ));
            }
        }
        "functools._cached" => ctx.alloc(Value::Cached {
            function: argument(args, 0)?,
            entries: Vec::new(),
        }),
        "collections.defaultdict" => ctx.alloc(Value::DefaultDict {
            factory: args.first().copied().unwrap_or(NONE),
            entries: Vec::new(),
        }),
        "contextlib.contextmanager" => {
            let callable = ctx.alloc(Value::Native("contextlib._manager".into()));
            ctx.alloc(Value::Method {
                callable,
                receiver: argument(args, 0)?,
            })
        }
        "warnings.warn" => {
            eprintln!("Warning: {}", ctx.str_value(argument(args, 0)?)?);
            NONE
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
        name if name.starts_with("re.")
            || name.starts_with("pattern.")
            || name == "match.group" =>
        {
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
