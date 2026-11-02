//! Fixed-length struct sequences whose named fields are all visible in the tuple.
use super::{Context, Error, Id, Result, Value};

pub(in crate::dynamic) const HASH_NAME: &str = "sys.hash_info";
pub(in crate::dynamic) const TERMINAL_NAME: &str = "os.terminal_size";

pub(super) const HASH_INFO: Record = Record {
    name: HASH_NAME,
    fields: &[
        "width",
        "modulus",
        "inf",
        "nan",
        "imag",
        "algorithm",
        "hash_bits",
        "seed_bits",
        "cutoff",
    ],
};
pub(super) const TERMINAL_SIZE: Record = Record {
    name: TERMINAL_NAME,
    fields: &["columns", "lines"],
};

pub(in crate::dynamic) struct Record {
    pub(super) name: &'static str,
    fields: &'static [&'static str],
}

pub(in crate::dynamic) fn lookup(name: &str) -> Option<&'static Record> {
    match name {
        HASH_NAME => Some(&HASH_INFO),
        TERMINAL_NAME => Some(&TERMINAL_SIZE),
        _ => None,
    }
}

impl Record {
    pub(super) fn record(&self, ctx: &mut Context, values: Vec<Id>) -> Id {
        ctx.alloc(Value::NamedTuple {
            class: Some(self.name),
            names: self.fields.iter().map(|s| (*s).to_owned()).collect(),
            values,
            extras: Vec::new(),
        })
    }

    pub(in crate::dynamic) fn construct(
        &self,
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
        let iterator = ctx
            .iterator(sequence.expect("required sequence bound"))
            .map_err(|error| {
                if ctx.is_builtin_error(&error, "TypeError") {
                    Error::new("TypeError", "constructor requires a sequence")
                } else {
                    error
                }
            })?;
        let values = ctx.items(iterator)?;
        let extra_fields = match dictionary {
            None => false,
            Some(dict) => !ctx
                .dict_entries(dict)
                .ok_or_else(|| {
                    Error::new(
                        "TypeError",
                        format!("{}() takes a dict as second arg, if any", self.name),
                    )
                })?
                .is_empty(),
        };
        if values.len() != self.fields.len() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{}() takes a {}-sequence ({}-sequence given)",
                    self.name,
                    self.fields.len(),
                    values.len()
                ),
            ));
        }
        if extra_fields {
            return Err(Error::new(
                "TypeError",
                format!("{}() got duplicate or unexpected field name(s)", self.name),
            ));
        }
        Ok(self.record(ctx, values))
    }

    pub(in crate::dynamic) fn attribute(
        &self,
        ctx: &mut Context,
        name: &str,
        on_type: bool,
    ) -> Option<Id> {
        let (module, short) = self.name.split_once('.').unwrap();
        Some(match name {
            "__name__" | "__qualname__" if on_type => ctx.text(short),
            "__module__" if on_type => ctx.text(module),
            "__base__" if on_type => ctx.builtin("tuple"),
            "n_fields" | "n_sequence_fields" => ctx.int(self.fields.len() as i64),
            "n_unnamed_fields" => ctx.int(0),
            "__match_args__" => {
                let values = self.fields.iter().map(|&field| ctx.text(field)).collect();
                ctx.alloc(Value::Tuple(values))
            }
            _ if on_type => {
                ctx.readonly_member(self.name, self.fields.iter().copied().find(|&f| f == name)?)
            }
            _ => return None,
        })
    }
}
