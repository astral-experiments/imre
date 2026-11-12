//! Prepare the registered source units selected by import resolution.
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::ModuleSyntaxError;
use super::emit::{ModuleOrigin, PhysicalFile, PreparedBody, RegisteredBodies};
use super::provider_archive::Providers;
use crate::frontend::module_source::ParsedModule;
use crate::ir::PhysicalModuleKind;

pub(super) type ModuleSource = (String, PathBuf, String);

pub(super) enum Initializer<'a> {
    Checked(Box<PreparedBody<'a>>),
    Archived(&'a str),
}

pub(super) struct PreparedUnit<'a> {
    pub(super) module: &'a ModuleSource,
    pub(super) builtin: bool,
    pub(super) physical: Option<(&'a PhysicalFile, PhysicalModuleKind<'a>)>,
    pub(super) zip: Option<&'a ([u8; 32], u32)>,
    pub(super) initializer: Result<Initializer<'a>, &'a ModuleSyntaxError>,
}

/// Check consumes this iterator one unit at a time. A build retains the same
/// initializers until every source has passed preparation.
pub(super) fn units<'a>(
    (files, builtins): (&'a [ModuleSource], &'a [ModuleSource]),
    (mut entry_source, mut normalized_files): (
        Option<ParsedModule<'a, 'a>>,
        HashMap<usize, super::LoweredRoot>,
    ),
    python_optimization: i32,
    (bundled_root, providers): (&'a Path, Option<&'a Providers>),
    module_errors: &'a HashMap<PathBuf, Box<ModuleSyntaxError>>,
    registrations: &'a RegisteredBodies<'a>,
    mut layouts: Vec<crate::analysis::RegisteredLayouts>,
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
        let types = layouts.iter_mut().find_map(|facts| facts.take(unit));
        let bundled = path.starts_with(bundled_root);
        let origin = if bundled {
            ModuleOrigin::Bundled
        } else if python_providers.contains(&unit) {
            ModuleOrigin::Provider
        } else {
            ModuleOrigin::Program(checked.and_then(|unit| unit.functions))
        };
        let initializer = if !builtin && let Some(error) = module_errors.get(path) {
            Err(error.as_ref())
        } else if unit != 0
            && python_optimization == 0
            && matches!(origin, ModuleOrigin::Bundled)
            && physical.is_none()
            && zip.is_none()
            && discovered.is_none()
            && checked.is_none()
            && types.is_none()
            && let Some(symbol) = providers.and_then(|archive| archive.sealed(name, path, source))
        {
            // The standalone producer checked these exact bytes without application facts.
            Ok(Initializer::Archived(symbol))
        } else {
            // Preserve the loader's digest while compiling its normalized source.
            let file_digest = physical_files.get(path).and_then(|file| file.digest);
            let (text, syntax) = if let Some(entry) = discovered {
                (entry.text, Some(entry.parsed))
            } else {
                let text = if physical.is_some() && file_digest.is_none() {
                    crate::normalize_source(source, crate::SourceKind::File)?
                } else if file_digest.is_some() || zip.is_some() {
                    crate::normalize_source(source, crate::SourceKind::DecodedFile)?
                } else {
                    Cow::Borrowed(source.as_str())
                };
                (text, None)
            };
            let types = types.filter(|types| types.matches_source(&text));
            let indexed = types.map(|types| types.parsed());
            // An indexed parser from another check owns different expression and statement IDs.
            let normalized = normalized_files.remove(&unit).filter(|_| {
                syntax.as_deref().is_some_and(|original| {
                    indexed
                        .as_deref()
                        .is_none_or(|indexed| std::ptr::eq(indexed, original))
                })
            });
            let module = match indexed.as_deref().map(Cow::Borrowed).or(syntax) {
                Some(parsed) => ParsedModule { text, parsed },
                None => ParsedModule::parse(path, text)?,
            };
            let mut body = PreparedBody::from_parsed(
                name,
                path.as_path(),
                (module, normalized),
                python_optimization,
                origin,
                checked
                    .map(|unit| unit.literal_sites.clone())
                    .unwrap_or_default(),
                types,
            )?;
            if let Some(module) = checked.and_then(|unit| unit.prompt_module) {
                body.prompts = &registrations.modules[module].doctest_prompts;
            }
            Ok(Initializer::Checked(Box::new(body)))
        };
        // Release each checker database after its last selected first-party file.
        layouts.retain(|facts| !facts.is_empty());
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
