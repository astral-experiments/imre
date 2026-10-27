use std::path::Path;

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

/// Supply filesystem operations and real module initialization to the shared discovery code.
pub(super) fn call(
    ctx: &mut Context,
    operation: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if operation == "missing_error" {
        let ([name, message], []) = (args, kwargs) else {
            return Err(Error::new(
                "TypeError",
                "expected a module name and message",
            ));
        };
        let error = Error::new("ModuleNotFoundError", ctx.str_value(*message)?).with_import(
            Some(ctx.str_value(*name)?),
            None,
            None,
        );
        return Ok(ctx.alloc(Value::Raised(error)));
    }
    let ([value], []) = (args, kwargs) else {
        return Err(Error::new("TypeError", "expected one positional argument"));
    };
    let text = ctx.str_value(*value)?;
    match operation {
        "filenames" => {
            let mut names = crate::objects::directory_entries(text, |entry| entry.file_name())
                .unwrap_or_default();
            names.sort_unstable();
            let names = names
                .into_iter()
                .map(|name| Ok(ctx.text(crate::objects::path_utf8(name)?.to_string())))
                .collect::<Result<_>>()?;
            Ok(ctx.alloc(Value::List(names)))
        }
        "isdir" => Ok(if Path::new(text).is_dir() {
            TRUE
        } else {
            FALSE
        }),
        "find_module" => {
            let name = text.to_owned();
            Ok(ctx.find_module(&name)?.unwrap_or(NONE))
        }
        "read_bytes" => {
            let bytes = crate::objects::read_path_bytes(text)?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        _ => unreachable!(),
    }
}
