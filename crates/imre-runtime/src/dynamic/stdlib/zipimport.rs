//! Archive resources for a runtime-selected zipimporter.

use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};

use super::super::{ModuleInitializer, SourceModule};

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

fn archive_member(source: &SourceModule) -> bool {
    matches!(
        source.code,
        ModuleInitializer::Zip(..)
            | ModuleInitializer::Invalid {
                archive_digest: Some(_),
                ..
            }
    )
}

fn absolute(path: &Path) -> Option<PathBuf> {
    std::path::absolute(path).ok()
}

impl Context {
    /// Index only the virtual path slots and namespace prefixes proven by a linked member.
    /// The code itself remains in the canonical qualified-name source registry.
    pub(in crate::dynamic) fn index_linked_zip_source(
        &mut self,
        name: &'static str,
        file: &'static Path,
        code: ModuleInitializer,
    ) {
        if !matches!(
            code,
            ModuleInitializer::Zip(..)
                | ModuleInitializer::Invalid {
                    archive_digest: Some(_),
                    ..
                }
        ) {
            return;
        }
        let Some(file) = absolute(file) else {
            return;
        };
        let Some((archive, identity)) = self
            .linked_zip_archives
            .iter()
            .copied()
            .filter_map(|archive| {
                let identity = absolute(archive)?;
                file.starts_with(&identity).then_some((archive, identity))
            })
            .max_by_key(|(_, identity)| identity.components().count())
        else {
            return;
        };
        let Some(mut root) = file.parent().map(Path::to_owned) else {
            return;
        };
        if file.ends_with("__init__.py") {
            let Some(parent) = root.parent() else {
                return;
            };
            root = parent.to_owned();
        }
        let mut current = name;
        loop {
            if !root.starts_with(&identity) {
                break;
            }
            self.linked_zip_paths
                .entry(root.clone())
                .or_insert_with(|| super::super::LinkedZipPath {
                    archive,
                    namespaces: Default::default(),
                });
            let Some((parent_name, _)) = current.rsplit_once('.') else {
                break;
            };
            let Some(parent_root) = root.parent().map(Path::to_owned) else {
                break;
            };
            if !parent_root.starts_with(&identity) {
                break;
            }
            self.linked_zip_paths
                .entry(parent_root.clone())
                .or_insert_with(|| super::super::LinkedZipPath {
                    archive,
                    namespaces: Default::default(),
                })
                .namespaces
                .insert(parent_name);
            current = parent_name;
            root = parent_root;
        }
    }

    /// Resolve one emitted virtual path slot without inspecting the filesystem or unrelated sources.
    pub(in crate::dynamic) fn linked_zip_archive(
        &self,
        root: &Path,
    ) -> Option<(&'static Path, PathBuf)> {
        let root = absolute(root)?;
        let indexed = self.linked_zip_paths.get(&root)?;
        let identity = absolute(indexed.archive)?;
        Some((
            indexed.archive,
            root.strip_prefix(identity).ok()?.to_owned(),
        ))
    }

    /// Select only a registered ZIP initializer at the current import path and qualified name.
    fn linked_zip_member(&self, fullname: &str, root: &Path) -> Option<&SourceModule> {
        let root = absolute(root)?;
        let leaf = fullname.rsplit('.').next()?;
        if Path::new(leaf).file_name() != Some(std::ffi::OsStr::new(leaf)) {
            return None;
        }
        let stem = root.join(leaf);
        let candidates = [stem.join("__init__.py"), stem.with_extension("py")];
        let entries = self.sources.get(fullname)?;
        candidates.iter().find_map(|candidate| {
            entries.iter().find(|source| {
                archive_member(source) && absolute(&source.path).as_ref() == Some(candidate)
            })
        })
    }

    /// Return a namespace portion only when registration proved a descendant at this path slot.
    fn linked_zip_namespace(&self, fullname: &str, root: &Path) -> Option<PathBuf> {
        let root = absolute(root)?;
        let indexed = self.linked_zip_paths.get(&root)?;
        indexed
            .namespaces
            .contains(fullname)
            .then(|| root.join(fullname.rsplit('.').next().unwrap()))
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
        ("linked_archive", [path], []) => {
            let path = ctx.import_path(*path)?;
            let Some((archive, prefix)) = ctx.linked_zip_archive(&path) else {
                return Ok(NONE);
            };
            let archive = ctx.import_path_text(archive)?;
            let mut prefix = prefix.to_string_lossy().into_owned();
            if !prefix.is_empty() {
                prefix.push('/');
            }
            let prefix = ctx.text(prefix);
            Ok(ctx.alloc(Value::Tuple(vec![archive, prefix])))
        }
        ("linked_member", [root, name], []) => {
            let root = ctx.import_path(*root)?;
            let fullname = ctx.str_value(*name)?;
            let Some(source) = ctx.linked_zip_member(fullname, &root) else {
                return Ok(NONE);
            };
            let path = source.path.to_path_buf();
            ctx.import_path_text(&path)
        }
        ("linked_namespace", [root, name], []) => {
            let root = ctx.import_path(*root)?;
            let Some(path) = ctx.linked_zip_namespace(ctx.str_value(*name)?, &root) else {
                return Ok(NONE);
            };
            ctx.import_path_text(&path)
        }
        ("linked_code", [name, filename], []) => {
            let path = ctx.import_path(*filename)?;
            let fullname = ctx.str_value(*name)?;
            let mut matching = ctx
                .sources
                .get(fullname)
                .into_iter()
                .flatten()
                .filter(|source| archive_member(source) && source.path.as_ref() == path)
                .filter_map(|source| match source.code {
                    code @ ModuleInitializer::Zip(entry, digest, flags) => {
                        Some((code, Some(entry), digest, Some(flags)))
                    }
                    code @ ModuleInitializer::Invalid {
                        archive_digest: Some(digest),
                        ..
                    } => Some((code, None, digest, None)),
                    _ => None,
                });
            let Some((code, entry, digest, flags)) = matching.next() else {
                return Ok(NONE);
            };
            if matching.any(|(_, other, hash, other_flags)| {
                !super::super::importlib::matching_linked_compilation(
                    (entry, digest, flags),
                    (other, hash, other_flags),
                    true,
                )
            }) {
                return Err(Error::new(
                    "ImportError",
                    format!("ambiguous linked ZIP source for {fullname:?}"),
                ));
            }
            ctx.module_code(code, &path)
        }
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
            let key = ctx.str_value(*path)?.to_owned();
            let Some(archive) = directory(ctx.zip_directories.entry(key))? else {
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
