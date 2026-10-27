use super::{Context, Error, Id, Result, Value};
use crate::dynamic::builtins;

/// Select the original element through Python's length and subscription protocols.
pub(super) fn choice(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let mut seq = args.first().copied();
    for (key, value) in kwargs {
        if key != "seq" {
            return Err(Error::new(
                "TypeError",
                format!("Random.choice() got an unexpected keyword argument '{key}'"),
            ));
        }
        if seq.replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                "Random.choice() got multiple values for argument 'seq'",
            ));
        }
    }
    if args.len() > 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "Random.choice() takes 2 positional arguments but {} were given",
                args.len() + 1
            ),
        ));
    }
    let seq = seq.ok_or_else(|| {
        Error::new(
            "TypeError",
            "Random.choice() missing 1 required positional argument: 'seq'",
        )
    })?;
    // Random.choice checks for emptiness and then reads the sampling bound separately.
    let first = builtins::call(ctx, "len", &[seq], &[])?;
    let bound = if ctx.truth(first)? {
        builtins::call(ctx, "len", &[seq], &[])?
    } else {
        first
    };
    let index = crate::objects::secrets_choice_index(&ctx.as_int(bound)?)?;
    let key = ctx.alloc(Value::Int(crate::Int::new(index)));
    ctx.get_item(seq, key)
}
