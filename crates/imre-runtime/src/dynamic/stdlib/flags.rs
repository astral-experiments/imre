//! The GIL-enabled process's immutable sys.flags struct sequence.
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

pub(in crate::dynamic) const TYPE: &str = "sys.flags";
const FIELDS: [&str; 18] = [
    "debug",
    "inspect",
    "interactive",
    "optimize",
    "dont_write_bytecode",
    "no_user_site",
    "no_site",
    "ignore_environment",
    "verbose",
    "bytes_warning",
    "quiet",
    "hash_randomization",
    "isolated",
    "dev_mode",
    "utf8_mode",
    "warn_default_encoding",
    "safe_path",
    "int_max_str_digits",
];
const EXTRA: [&str; 3] = ["gil", "thread_inherit_context", "context_aware_warnings"];

pub(super) fn current(ctx: &mut Context) -> Id {
    let enabled = |key: &str| ctx.config[key].as_bool().unwrap_or(false);
    let safe_path = if enabled("safe_path") { TRUE } else { FALSE };
    let isolated = enabled("isolated");
    let ignore_environment = isolated || enabled("ignore_environment");
    let optimize = ctx.config["optimize"].as_i64().unwrap_or(0);
    let dont_write_bytecode = enabled("dont_write_bytecode");
    let no_user_site = enabled("no_user_site");
    let warn_default_encoding = enabled("warn_default_encoding");
    let zero = ctx.int(0);
    let values = vec![
        zero,
        zero,
        zero,
        ctx.int(optimize),
        ctx.int(i64::from(dont_write_bytecode)),
        ctx.int(i64::from(no_user_site)),
        zero,
        ctx.int(i64::from(ignore_environment)),
        zero,
        zero,
        zero,
        ctx.int(i64::from(crate::python_hash::randomized())),
        ctx.int(i64::from(isolated)),
        FALSE,
        ctx.int(i64::from(crate::sys::imre_sys_utf8_mode())),
        ctx.int(i64::from(warn_default_encoding)),
        safe_path,
        ctx.int(i64::from(crate::integer_text::decimal_digit_limit())),
    ];
    let extra_values = [
        1,
        0,
        crate::sys::context_warnings::imre_sys_context_aware_warnings(),
    ];
    let extras = EXTRA
        .into_iter()
        .zip(extra_values)
        .map(|(key, n)| (key, ctx.int(n)))
        .collect();
    ctx.alloc(Value::NamedTuple {
        class: Some(TYPE),
        names: FIELDS.map(str::to_owned).into(),
        values,
        extras,
    })
}

/// Set the non-binary integer conversion limit and update the live sys.flags sequence.
pub(super) fn set_int_max_str_digits(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let count = args.len() + kwargs.len();
    if count > 1 {
        return Err(Error::new(
            "TypeError",
            format!("set_int_max_str_digits() takes at most 1 argument ({count} given)"),
        ));
    }
    let value = match (args, kwargs) {
        ([value], []) => *value,
        ([], [(key, value)]) if key == "maxdigits" => *value,
        _ => {
            return Err(Error::new(
                "TypeError",
                "set_int_max_str_digits() missing required argument 'maxdigits' (pos 1)",
            ));
        }
    };
    let limit = ctx
        .index_integer(value)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))?;
    if limit != 0 && limit < 640 {
        return Err(Error::new(
            "ValueError",
            "maxdigits must be >= 640 or 0 for unlimited",
        ));
    }
    let sys = match ctx.threads.system_module {
        Some(sys) => sys,
        None => ctx.module("sys")?,
    };
    let flags = ctx.lookup_attr(sys, "flags")?;
    let field = ctx.int(i64::from(limit));
    let Value::NamedTuple {
        class: Some(TYPE),
        values,
        ..
    } = ctx.heap.get_mut(flags)
    else {
        return Err(Error::new("TypeError", "expected sys.flags"));
    };
    let position = FIELDS
        .iter()
        .position(|&name| name == "int_max_str_digits")
        .unwrap();
    values[position] = field;
    crate::integer_text::set_decimal_digit_limit(limit as u32);
    Ok(NONE)
}

pub(in crate::dynamic) fn member(ctx: &mut Context, name: &str) -> Option<Id> {
    FIELDS
        .iter()
        .chain(&EXTRA)
        .find(|&&field| field == name)
        .map(|field| ctx.readonly_member(TYPE, field))
}

pub(in crate::dynamic) fn type_attribute(
    ctx: &mut Context,
    name: &str,
    on_type: bool,
) -> Option<Id> {
    Some(match name {
        "__name__" | "__qualname__" if on_type => ctx.text("flags"),
        "__module__" if on_type => ctx.text("sys"),
        "__base__" if on_type => ctx.builtin("tuple"),
        "__new__" if on_type => ctx.builtin("tuple.__new__"),
        "n_fields" => ctx.int((FIELDS.len() + EXTRA.len()) as i64),
        "n_sequence_fields" => ctx.int(FIELDS.len() as i64),
        "n_unnamed_fields" => ctx.int(0),
        "__match_args__" => {
            let names = FIELDS.map(|field| ctx.text(field)).into();
            ctx.alloc(Value::Tuple(names))
        }
        _ if on_type => return member(ctx, name),
        _ => return None,
    })
}
