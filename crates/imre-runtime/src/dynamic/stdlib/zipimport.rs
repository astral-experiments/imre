//! Archive resources for a runtime-selected zipimporter.

use std::collections::hash_map::Entry;

use super::super::builtins::boolean;
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};
use crate::objects::ZipArchive;

/// Use the same cached directory and member reader as the typed loader.
pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match (name, args, kwargs) {
        ("exists", [path], []) => Ok(boolean(ctx.import_path(*path)?.exists())),
        ("isfile", [path], []) => Ok(boolean(ctx.import_path(*path)?.is_file())),
        ("invalidate", [path], []) => {
            let path = ctx.str_value(*path)?.to_owned();
            ctx.zip_directories.remove(&path);
            Ok(NONE)
        }
        ("index", [path], []) => {
            let path = ctx.str_value(*path)?.to_owned();
            if let Entry::Vacant(slot) = ctx.zip_directories.entry(path) {
                match ZipArchive::index_import(slot.key()).map_err(Error::from) {
                    Ok(index) => {
                        slot.insert(index);
                    }
                    Err(error) if error.kind == "BadZipFile" => return Ok(FALSE),
                    Err(error) => return Err(error),
                }
            }
            Ok(TRUE)
        }
        ("names", [path], []) => {
            let Some(archive) = ctx.zip_directories.get(ctx.str_value(*path)?) else {
                return Err(ctx.lookup_error("KeyError", *path));
            };
            let mut names = Vec::new();
            archive.visit_names(|name| names.push(ctx.heap.owned_text(name.as_str().into())));
            Ok(ctx.alloc(Value::List(names)))
        }
        ("read" | "contains", [path, key], []) => {
            let path = ctx.str_value(*path)?;
            let key = ctx.str_value(*key)?;
            let archive = ctx
                .zip_directories
                .get(path)
                .ok_or_else(|| Error::new("ValueError", "archive has not been indexed"))?;
            let key = key.into();
            if name == "contains" {
                return Ok(if archive.contains_import(&key) {
                    TRUE
                } else {
                    FALSE
                });
            }
            let bytes = archive.import_data(&key)?;
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to _imre_zipimport.{name}"),
        )),
    }
}
