//! Fixed-size struct sequences whose named fields are all visible tuple elements.

use super::{Context, Error, Id, Result, Value};
use crate::python_hash;

pub(in crate::dynamic) const HASH_INFO: &str = "sys.hash_info";
pub(in crate::dynamic) const TERMINAL_SIZE: &str = "os.terminal_size";
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
        HASH_INFO => Some(HASH_INFO),
        UNRAISABLE_ARGS => Some(UNRAISABLE_ARGS),
        TERMINAL_SIZE => Some(TERMINAL_SIZE),
        _ => None,
    }
}

pub(in crate::dynamic) fn fields(class: &str) -> &'static [&'static str] {
    match class {
        HASH_INFO => &HASH_FIELDS,
        UNRAISABLE_ARGS => &[
            "exc_type",
            "exc_value",
            "exc_traceback",
            "err_msg",
            "object",
        ],
        TERMINAL_SIZE => &["columns", "lines"],
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
        "__doc__" if class == UNRAISABLE_ARGS => {
            ctx.text("UnraisableHookArgs\n\nType used to pass arguments to sys.unraisablehook.")
        }
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
