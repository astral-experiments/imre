use super::{Context, Error, FALSE, Id, Result, TRUE, Value, os_error};

/// Supply filesystem operations and header dedenting to the shared distribution lookup.
pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let ([value], []) = (args, kwargs) else {
        return Err(Error::new("TypeError", "expected one positional argument"));
    };
    let text = ctx.str_value(*value)?;
    match name {
        "filenames" => {
            let entries = crate::objects::directory_entries(text, |entry| entry.file_name())
                .map_err(os_error)?;
            let names = entries
                .into_iter()
                .map(|entry| Ok(ctx.text(crate::objects::path_utf8(entry)?.to_string())))
                .collect::<Result<_>>()?;
            Ok(ctx.alloc(Value::List(names)))
        }
        "exists" => Ok(if std::path::Path::new(text).exists() {
            TRUE
        } else {
            FALSE
        }),
        "read_text" => Ok(ctx.text(crate::objects::read_path_text(text)?.to_string())),
        "dedent" => Ok(ctx.text(crate::objects::dedent(text).to_string())),
        _ => unreachable!(),
    }
}
