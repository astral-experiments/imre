use super::{Context, Error, Id, Result, Value};
use crate::dynamic::builtins;

/// Select the original element through Python's length and subscription protocols.
pub(super) fn choice(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    secure: bool,
) -> Result<Id> {
    let mut seq = args.first().copied();
    for (key, value) in kwargs {
        match key.as_str() {
            "seq" if seq.is_none() => seq = Some(*value),
            "self" | "seq" => {
                return Err(Error::new(
                    "TypeError",
                    format!("Random.choice() got multiple values for argument '{key}'"),
                ));
            }
            _ => {
                let mut message =
                    format!("Random.choice() got an unexpected keyword argument '{key}'");
                if let Some(hint) =
                    crate::objects::exceptions::keyword_suggestion(key, &["self", "seq"])
                {
                    message.push_str(&format!(". Did you mean '{hint}'?"));
                }
                return Err(Error::new("TypeError", message));
            }
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
    let index = crate::objects::choice_index(&ctx.as_int(bound)?, secure)?;
    let key = ctx.alloc(Value::Int(crate::Int::new(index)));
    ctx.get_item(seq, key)
}
