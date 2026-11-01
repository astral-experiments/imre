//! The runtime's hash configuration, presented as Python's nine-field struct sequence.

use super::{Context, Error, Id, Result, Value};
use crate::python_hash;

pub(in crate::dynamic) const TYPE: &str = "sys.hash_info";
const FIELDS: [&str; 9] = [
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

fn record(ctx: &mut Context, values: Vec<Id>) -> Id {
    ctx.alloc(Value::NamedTuple {
        class: Some(TYPE),
        names: FIELDS.map(str::to_owned).into(),
        values,
        extras: Vec::new(),
    })
}

pub(super) fn current(ctx: &mut Context) -> Id {
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
    record(ctx, values)
}

pub(in crate::dynamic) fn construct(
    ctx: &mut Context,
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
    let values = ctx.items(sequence.expect("required sequence bound"))?;
    if values.len() != FIELDS.len() {
        return Err(Error::new(
            "TypeError",
            format!(
                "{TYPE}() takes a {}-sequence ({}-sequence given)",
                FIELDS.len(),
                values.len(),
            ),
        ));
    }
    if let Some(dict) = dictionary {
        let entries = ctx.dict_entries(dict).ok_or_else(|| {
            Error::new(
                "TypeError",
                "sys.hash_info() takes a dict as second arg, if any",
            )
        })?;
        if !entries.is_empty() {
            return Err(Error::new(
                "TypeError",
                "sys.hash_info() got duplicate or unexpected field name(s)",
            ));
        }
    }
    Ok(record(ctx, values))
}

pub(in crate::dynamic) fn type_attribute(
    ctx: &mut Context,
    name: &str,
    on_type: bool,
) -> Option<Id> {
    Some(match name {
        "__name__" | "__qualname__" if on_type => ctx.text("hash_info"),
        "__module__" if on_type => ctx.text("sys"),
        "__base__" if on_type => ctx.builtin("tuple"),
        "n_fields" | "n_sequence_fields" => ctx.int(FIELDS.len() as i64),
        "n_unnamed_fields" => ctx.int(0),
        "__match_args__" => {
            let values = FIELDS.map(|field| ctx.text(field)).into();
            ctx.alloc(Value::Tuple(values))
        }
        _ => return None,
    })
}
