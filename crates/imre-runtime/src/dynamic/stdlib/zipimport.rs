//! Archive resources for a runtime-selected zipimporter.

use std::collections::hash_map::Entry;

use super::super::builtins::{boolean, exception_subclass};
use super::{Context, Error, FALSE, Id, NONE, Result, Value};
use crate::objects::ZipArchive;

fn directory(entry: Entry<'_, String, ZipArchive>) -> Result<Option<&ZipArchive>> {
    match entry {
        Entry::Occupied(slot) => Ok(Some(slot.into_mut())),
        Entry::Vacant(slot) => match ZipArchive::index_import(slot.key()).map_err(Error::from) {
            Ok(archive) => Ok(Some(slot.insert(archive))),
            Err(error) if error.kind == "BadZipFile" => Ok(None),
            Err(error) => Err(error),
        },
    }
}

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
            let archive = directory(ctx.zip_directories.entry(path))?;
            Ok(boolean(archive.is_some()))
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
            let path = ctx.str_value(*path)?.to_owned();
            // The directory lookup takes precedence over conversion of the member name.
            let key = ctx.copy_name(*key);
            let archive = match directory(ctx.zip_directories.entry(path)) {
                Err(error) if exception_subclass(&error.kind, "OSError") == Some(true) => None,
                result => result?,
            };
            let Some(archive) = archive else {
                return Ok(if name == "contains" { FALSE } else { NONE });
            };
            let key = key?;
            if name == "contains" {
                return Ok(boolean(archive.contains_import(&key)));
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
