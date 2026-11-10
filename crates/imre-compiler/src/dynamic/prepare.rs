//! Prepare the registered source units selected by import resolution.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::ModuleSyntaxError;
use super::emit::{ModuleOrigin, PhysicalFile, PreparedBody, RegisteredBodies};
use crate::frontend::module_source::ParsedModule;
use crate::ir::PhysicalModuleKind;

pub(super) type ModuleSource = (String, PathBuf, String);

pub(super) struct PreparedUnit<'a> {
    pub(super) module: &'a ModuleSource,
    pub(super) builtin: bool,
    pub(super) physical: Option<(&'a PhysicalFile, PhysicalModuleKind<'a>)>,
    pub(super) zip: Option<&'a ([u8; 32], u32)>,
    pub(super) initializer: Result<PreparedBody<'a>, &'a ModuleSyntaxError>,
}

/// Check consumes this iterator one unit at a time. A build retains the same
/// initializers until every source has passed preparation.
pub(super) fn units<'a>(
    files: &'a [ModuleSource],
    builtins: &'a [ModuleSource],
    mut entry_source: Option<ParsedModule<'a>>,
    python_optimization: i32,
    bundled_root: &'a Path,
    module_errors: &'a HashMap<PathBuf, Box<ModuleSyntaxError>>,
    registrations: &'a RegisteredBodies<'a>,
) -> impl Iterator<Item = Result<PreparedUnit<'a>, String>> {
    let RegisteredBodies {
        checked_units,
        physical_files,
        archive_members,
        python_providers,
        ..
    } = registrations;
    let modules = files
        .iter()
        .map(|module| (module, false))
        .chain(builtins.iter().map(|module| (module, true)));
    modules.enumerate().map(move |(unit, (original, builtin))| {
        let (name, path, source) = original;
        let checked = checked_units.get(&unit);
        let discovered = entry_source.take().or_else(|| registrations.parsed(unit));
        let zip = archive_members.get(path);
        let physical = physical_files.get(path).filter(|_| !builtin);
        let initializer = if !builtin && let Some(error) = module_errors.get(path) {
            Err(error.as_ref())
        } else {
            let bundled = path.starts_with(bundled_root);
            let origin = if bundled {
                ModuleOrigin::Bundled
            } else if python_providers.contains(&unit) {
                ModuleOrigin::Provider
            } else {
                ModuleOrigin::Program(checked.and_then(|unit| unit.functions))
            };
            // Preserve the loader's digest while compiling its normalized source.
            let file_digest = physical_files.get(path).and_then(|file| file.digest);
            let module = if let Some(entry) = discovered {
                entry
            } else {
                let text = if physical.is_some() && file_digest.is_none() {
                    crate::normalize_source(source, crate::SourceKind::File)?
                } else if file_digest.is_some() || zip.is_some() {
                    crate::normalize_source(source, crate::SourceKind::DecodedFile)?
                } else {
                    std::borrow::Cow::Borrowed(source.as_str())
                };
                ParsedModule::parse(path, text)?
            };
            Ok(PreparedBody::from_parsed(
                name,
                path,
                module,
                python_optimization,
                origin,
            )?)
        };
        Ok(PreparedUnit {
            module: original,
            builtin,
            physical: physical.map(|file| {
                let kind = checked
                    .and_then(|unit| unit.run_path.as_deref())
                    .map_or(PhysicalModuleKind::File, PhysicalModuleKind::DeclaredFile);
                (file, kind)
            }),
            zip,
            initializer,
        })
    })
}
