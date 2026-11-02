use std::rc::Rc;

use crate::objects::ZipArchive;

use super::{Context, Error, Id, NONE, Result, Value, os_error};

/// Supply filesystem operations and header dedenting to the shared distribution lookup.
pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if let ("zip_names" | "zip_read", [handle, rest @ ..], []) = (name, args, kwargs) {
        let Value::MetadataArchive(archive) = ctx.heap.get(*handle) else {
            return Err(Error::new("TypeError", "expected a distribution archive"));
        };
        let archive = Rc::clone(archive);
        return match (name, rest) {
            ("zip_names", []) => {
                let mut names = Vec::new();
                archive.visit_names(|name| names.push(ctx.text(name.as_str())));
                Ok(ctx.alloc(Value::List(names)))
            }
            ("zip_read", [key]) => {
                let data = archive.read_named(&ctx.str_value(*key)?.into())?;
                Ok(data.map_or(NONE, |bytes| ctx.alloc(Value::Bytes(bytes))))
            }
            _ => Err(Error::new(
                "TypeError",
                "invalid distribution archive arguments",
            )),
        };
    }
    let ([value], []) = (args, kwargs) else {
        return Err(Error::new("TypeError", "expected one positional argument"));
    };
    let text = ctx.str_value(*value)?;
    match name {
        "zip_open" => {
            let archive = ZipArchive::open_path(text)?;
            Ok(ctx.alloc(Value::MetadataArchive(Rc::new(archive))))
        }
        "filenames" => {
            let entries = crate::objects::directory_entries(text, |entry| entry.file_name())
                .map_err(os_error)?;
            let names = entries
                .into_iter()
                .map(|entry| Ok(ctx.text(crate::objects::path_utf8(entry)?.to_string())))
                .collect::<Result<_>>()?;
            Ok(ctx.alloc(Value::List(names)))
        }
        "dedent" => Ok(ctx.text(crate::objects::dedent(text).to_string())),
        _ => unreachable!(),
    }
}
