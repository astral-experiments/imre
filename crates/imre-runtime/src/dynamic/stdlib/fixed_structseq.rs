//! Fixed-size struct sequences whose named fields are all visible tuple elements.

use super::{Context, Error, Id, NONE, Result, Value};
use crate::python_hash;

pub(in crate::dynamic) const GROUP: &str = "grp.struct_group";
pub(in crate::dynamic) const PASSWD: &str = "pwd.struct_passwd";
pub(in crate::dynamic) const HASH_INFO: &str = "sys.hash_info";
pub(in crate::dynamic) const THREAD_INFO: &str = "sys.thread_info";
pub(in crate::dynamic) const SCHED_PARAM: &str = "posix.sched_param";
pub(in crate::dynamic) const TERMINAL_SIZE: &str = "os.terminal_size";
pub(in crate::dynamic) const ASYNCGEN_HOOKS: &str = "asyncgen_hooks";
pub(in crate::dynamic) const UNRAISABLE_ARGS: &str = "UnraisableHookArgs";
const HASH_FIELDS: [&str; 9] = [
    "width",
    "modulus",
    "inf",
    "nan",
    "imag",
    "algorithm",
    "hash_bits",
    "seed_bits",
    "cutoff",
];

pub(in crate::dynamic) fn fixed_type(name: &str) -> Option<&'static str> {
    match name {
        GROUP if cfg!(any(target_os = "linux", target_os = "macos")) => Some(GROUP),
        PASSWD if cfg!(any(target_os = "linux", target_os = "macos")) => Some(PASSWD),
        HASH_INFO => Some(HASH_INFO),
        THREAD_INFO => Some(THREAD_INFO),
        ASYNCGEN_HOOKS => Some(ASYNCGEN_HOOKS),
        UNRAISABLE_ARGS => Some(UNRAISABLE_ARGS),
        TERMINAL_SIZE => Some(TERMINAL_SIZE),
        SCHED_PARAM if cfg!(target_os = "linux") => Some(SCHED_PARAM),
        _ => None,
    }
}

pub(in crate::dynamic) fn fields(class: &str) -> &'static [&'static str] {
    match class {
        GROUP => &["gr_name", "gr_passwd", "gr_gid", "gr_mem"],
        PASSWD => &[
            "pw_name",
            "pw_passwd",
            "pw_uid",
            "pw_gid",
            "pw_gecos",
            "pw_dir",
            "pw_shell",
        ],
        HASH_INFO => &HASH_FIELDS,
        THREAD_INFO => &["name", "lock", "version"],
        ASYNCGEN_HOOKS => &["firstiter", "finalizer"],
        UNRAISABLE_ARGS => &[
            "exc_type",
            "exc_value",
            "exc_traceback",
            "err_msg",
            "object",
        ],
        TERMINAL_SIZE => &["columns", "lines"],
        SCHED_PARAM => &["sched_priority"],
        _ => unreachable!("unknown fixed struct sequence"),
    }
}

pub(in crate::dynamic) fn member(ctx: &mut Context, class: &str, name: &str) -> Option<Id> {
    let owner = fixed_type(class)?;
    let field = fields(owner).iter().copied().find(|&field| field == name)?;
    Some(ctx.readonly_member(owner, field))
}

pub(in crate::dynamic) fn record(ctx: &mut Context, class: &'static str, values: Vec<Id>) -> Id {
    ctx.alloc(Value::NamedTuple {
        class: Some(class),
        names: fields(class)
            .iter()
            .map(|&field| field.to_owned())
            .collect(),
        values,
        extras: Vec::new(),
    })
}

pub(in crate::dynamic) fn thread_info(ctx: &mut Context) -> Id {
    let name = crate::sys::thread_name().map_or(NONE, |value| ctx.text(value));
    let lock = crate::sys::thread_lock_backend().map_or(NONE, |value| ctx.text(value));
    let version = crate::sys::thread_version().map_or(NONE, |value| ctx.text(value));
    record(ctx, THREAD_INFO, vec![name, lock, version])
}

pub(super) fn current_hash_info(ctx: &mut Context) -> Id {
    let values = vec![
        ctx.int(i64::from(i64::BITS)),
        ctx.int(python_hash::MODULUS),
        ctx.int(python_hash::INF),
        // Retained for compatibility; NaNs hash by identity.
        ctx.int(0),
        ctx.int(python_hash::IMAG),
        ctx.text(python_hash::ALGORITHM),
        ctx.int(i64::from(python_hash::HASH_BITS)),
        ctx.int(i64::from(python_hash::SEED_BITS)),
        ctx.int(i64::from(python_hash::CUTOFF)),
    ];
    record(ctx, HASH_INFO, values)
}

/// Materialize a struct sequence's positional argument with PySequence_Fast semantics.
pub(super) fn sequence_values(ctx: &mut Context, sequence: Id) -> Result<Vec<Id>> {
    if matches!(ctx.heap.get(sequence), Value::List(_) | Value::Tuple(_)) {
        ctx.items(sequence)
    } else {
        let iter = ctx.iterator(sequence).map_err(|err| {
            let type_error = ctx.builtin("TypeError");
            if ctx.raised_error_matches(&err, type_error) {
                Error::new("TypeError", "constructor requires a sequence")
            } else {
                err
            }
        })?;
        // PySequence_Fast consumes this iterator as a list, including its length hint.
        let list = ctx.alloc(Value::List(Vec::new()));
        super::super::builtins::list_extend(ctx, list, iter)?;
        ctx.items(list)
    }
}

pub(in crate::dynamic) fn construct(
    ctx: &mut Context,
    class: &'static str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    // CPython's sched_param has a scalar constructor, unlike other struct sequences.
    if class == SCHED_PARAM {
        let [priority] = super::super::builtins::bind_builtin_arguments(
            "sched_param",
            args,
            kwargs,
            ["sched_priority"],
            1,
        )?;
        return Ok(record(
            ctx,
            class,
            vec![priority.expect("required priority bound")],
        ));
    }
    let [sequence, dictionary] = super::super::builtins::bind_builtin_arguments(
        "structseq",
        args,
        kwargs,
        ["sequence", "dict"],
        1,
    )?;
    let sequence = sequence.expect("required sequence bound");
    let values = sequence_values(ctx, sequence)?;
    let extras = if let Some(dict) = dictionary {
        ctx.dict_entries(dict)
            .ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!("{class}() takes a dict as second arg, if any"),
                )
            })?
            .len()
    } else {
        0
    };
    let expected = fields(class).len();
    if values.len() != expected {
        return Err(Error::new(
            "TypeError",
            format!(
                "{class}() takes a {expected}-sequence ({}-sequence given)",
                values.len()
            ),
        ));
    }
    if extras != 0 {
        return Err(Error::new(
            "TypeError",
            format!("{class}() got duplicate or unexpected field name(s)"),
        ));
    }
    Ok(record(ctx, class, values))
}

/// sched_param reduces to one scalar constructor argument and can replace just that field.
pub(in crate::dynamic) fn sched_param_method(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method {SCHED_PARAM}.{method}() needs an argument"),
        ));
    };
    if method == "__new__" {
        if receiver != ctx.builtin(SCHED_PARAM) {
            return Err(Error::new(
                "TypeError",
                "sched_param.__new__() requires the sched_param type",
            ));
        }
        return construct(ctx, SCHED_PARAM, rest, kwargs);
    }
    let Value::NamedTuple {
        class: Some(SCHED_PARAM),
        values,
        ..
    } = ctx.heap.get(receiver)
    else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{method}' for '{SCHED_PARAM}' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    };
    let mut priority = values[0];
    match method {
        "__reduce__" => {
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "sched_param.__reduce__() takes no keyword arguments",
                ));
            }
            if !rest.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "sched_param.__reduce__() takes no arguments ({} given)",
                        rest.len()
                    ),
                ));
            }
            let class = ctx.builtin(SCHED_PARAM);
            let args = ctx.alloc(Value::Tuple(vec![priority]));
            Ok(ctx.alloc(Value::Tuple(vec![class, args])))
        }
        "__replace__" => {
            if !rest.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "__replace__() takes no positional arguments",
                ));
            }
            let mut unknown = Vec::new();
            for (key, value) in kwargs {
                if key == "sched_priority" {
                    priority = *value;
                } else {
                    unknown.push(ctx.text(key));
                }
            }
            if !unknown.is_empty() {
                let keys = ctx.alloc(Value::List(unknown));
                return Err(Error::new(
                    "TypeError",
                    format!("Got unexpected field name(s): {}", ctx.repr(keys)),
                ));
            }
            Ok(record(ctx, SCHED_PARAM, vec![priority]))
        }
        _ => Err(Error::new("AttributeError", method.to_owned())),
    }
}

pub(in crate::dynamic) fn type_attribute(
    ctx: &mut Context,
    class: &'static str,
    name: &str,
    on_type: bool,
) -> Option<Id> {
    let (module, qualname) = class.split_once('.').unwrap_or(("builtins", class));
    Some(match name {
        "__name__" | "__qualname__" if on_type => ctx.text(qualname),
        "__module__" => ctx.text(module),
        "__new__" if class == SCHED_PARAM => ctx.builtin("posix.sched_param.__new__"),
        "__reduce__" | "__replace__" if on_type && class == SCHED_PARAM => {
            ctx.builtin(&format!("{class}.{name}"))
        }
        "__reduce_ex__" if on_type && class == SCHED_PARAM => ctx.builtin("object.__reduce_ex__"),
        "__doc__" if class == SCHED_PARAM => {
            ctx.text("Currently has only one field: sched_priority")
        }
        "__doc__" if class == ASYNCGEN_HOOKS => ctx.text(concat!(
            "asyncgen_hooks\n\nA named tuple providing information about asynchronous\n",
            "generators hooks.  The attributes are read only."
        )),
        "__doc__" if class == UNRAISABLE_ARGS => {
            ctx.text("UnraisableHookArgs\n\nType used to pass arguments to sys.unraisablehook.")
        }
        "__doc__" if class == THREAD_INFO => ctx.text(
            "sys.thread_info\n\nA named tuple holding information about the thread implementation.",
        ),
        "__doc__" if class == GROUP => ctx.text(concat!(
            "grp.struct_group: Results from getgr*() routines.\n\n",
            "This object may be accessed either as a tuple of\n",
            "  (gr_name,gr_passwd,gr_gid,gr_mem)\n",
            "or via the object attributes as named in the above tuple.\n"
        )),
        "__doc__" if class == PASSWD => ctx.text(concat!(
            "pwd.struct_passwd: Results from getpw*() routines.\n\n",
            "This object may be accessed either as a tuple of\n",
            "  (pw_name,pw_passwd,pw_uid,pw_gid,pw_gecos,pw_dir,pw_shell)\n",
            "or via the object attributes as named in the above tuple."
        )),
        "__doc__" if class == TERMINAL_SIZE => {
            ctx.text("A tuple of (columns, lines) for holding terminal window size")
        }
        "__bases__" if on_type => {
            let base = ctx.builtin("tuple");
            ctx.alloc(Value::Tuple(vec![base]))
        }
        "__mro__" if on_type => {
            let owner = ctx.builtin(class);
            let base = ctx.builtin("tuple");
            let object = ctx.builtin("object");
            ctx.alloc(Value::Tuple(vec![owner, base, object]))
        }
        "__base__" if on_type => ctx.builtin("tuple"),
        "n_fields" | "n_sequence_fields" => ctx.int(fields(class).len() as i64),
        "n_unnamed_fields" => ctx.int(0),
        "__match_args__" => {
            let values = fields(class).iter().map(|&field| ctx.text(field)).collect();
            ctx.alloc(Value::Tuple(values))
        }
        _ if on_type => return member(ctx, class, name),
        _ => return None,
    })
}
