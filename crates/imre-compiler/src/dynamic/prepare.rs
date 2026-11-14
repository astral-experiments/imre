//! Prepare the registered source units selected by import resolution.
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::ModuleSyntaxError;
use super::emit::{ModuleOrigin, PhysicalFile, PreparedBody, RegisteredBodies};
use crate::frontend::module_source::ParsedModule;
use crate::ir::PhysicalModuleKind;

pub(crate) type ModuleSource<'a> = (String, PathBuf, Cow<'a, str>);

pub(super) enum Shipped<'a> {
    Checked(Result<(), String>),
    Archived(&'a str),
}

#[derive(Default)]
pub(super) struct SourceInputs<'a> {
    pub(super) literal_sites: BTreeMap<usize, super::literal::LiteralSites>,
    pub(super) entry_source: Option<ParsedModule<'a, 'a>>,
    pub(super) scanned_modules: HashMap<usize, super::ScannedModule>,
    pub(super) shipped_checks: HashMap<usize, Shipped<'a>>,
}

pub(super) enum Initializer<'a> {
    Checked(Box<PreparedBody<'a>>),
    Archived(&'a str),
}

pub(super) struct PreparedUnit<'a> {
    pub(super) module: &'a ModuleSource<'a>,
    pub(super) builtin: bool,
    pub(super) physical: Option<(&'a PhysicalFile, PhysicalModuleKind<'a>)>,
    pub(super) zip: Option<&'a [u8; 32]>,
    pub(super) initializer: Result<Initializer<'a>, &'a ModuleSyntaxError>,
}

/// Check a self-contained initializer without retaining its prepared body.
pub(super) fn check_shipped(
    name: &str,
    path: &Path,
    input: super::emit::ParsedInput<'_, '_>,
    python_optimization: i32,
) -> Result<(), String> {
    PreparedBody::from_parsed(
        name,
        path,
        input,
        python_optimization,
        ModuleOrigin::Bundled,
        Default::default(),
        Default::default(),
    )
    .map(drop)
}

/// Lower each source on demand for checking or native emission.
pub(super) fn units<'a>(
    (files, builtins): (&'a [ModuleSource<'a>], &'a [ModuleSource<'a>]),
    SourceInputs {
        mut entry_source,
        mut literal_sites,
        mut scanned_modules,
        mut shipped_checks,
    }: SourceInputs<'a>,
    python_optimization: i32,
    bundled_root: &'a Path,
    module_errors: &'a HashMap<PathBuf, Box<ModuleSyntaxError>>,
    registrations: &'a RegisteredBodies<'a>,
    mut layouts: Vec<crate::analysis::RegisteredLayouts>,
) -> impl Iterator<Item = Result<PreparedUnit<'a>, String>> {
    let RegisteredBodies {
        checked_units,
        physical_files,
        archive_members,
        ..
    } = registrations;
    let modules = files
        .iter()
        .map(|module| (module, false))
        .chain(builtins.iter().map(|module| (module, true)));
    modules
        .enumerate()
        .map(move |(unit, (original, builtin))| {
            let (name, path, source) = original;
            let checked = checked_units.get(&unit);
            let file = checked.and_then(|unit| unit.source);
            let module_id = checked.and_then(|unit| unit.module);
            let literals = literal_sites.remove(&unit);
            let scanned = scanned_modules.remove(&unit);
            let discovered = entry_source
                .take()
                .or_else(|| file.map(|file| file.parsed(registrations.modules)));
            let zip = archive_members.get(path);
            let physical = physical_files.get(path).filter(|_| !builtin);
            let types = layouts.iter_mut().find_map(|facts| facts.take(unit));
            let origin = registrations
                .provider_origin(unit, path.starts_with(bundled_root))
                .unwrap_or_else(|| {
                    ModuleOrigin::Program(module_id.and_then(|id| registrations.functions.get(&id)))
                });
            let standalone = unit != 0
                && matches!(origin, ModuleOrigin::Bundled)
                && physical.is_none()
                && zip.is_none()
                && discovered.is_none()
                && checked.is_none()
                && literals.is_none()
                && types.is_none();
            let initializer = if !builtin && let Some(error) = module_errors.get(path) {
                Err(error.as_ref())
            } else if standalone && let Some(shipped) = shipped_checks.remove(&unit) {
                match shipped {
                    Shipped::Checked(check) => return check.map(|()| None),
                    Shipped::Archived(symbol) => Ok(Initializer::Archived(symbol)),
                }
            } else {
                // Preserve the loader's digest while compiling its normalized source.
                let (text, syntax) = if let Some(entry) = discovered {
                    (entry.text, Some(entry.parsed))
                } else {
                    let text = if let Some(file) = physical {
                        crate::normalize_source(
                            source,
                            file.source_kind.unwrap_or(crate::SourceKind::File),
                        )?
                    } else if zip.is_some()
                        || physical_files
                            .get(path)
                            .is_some_and(|file| file.digest.is_some())
                    {
                        crate::normalize_source(source, crate::SourceKind::DecodedFile)?
                    } else {
                        Cow::Borrowed(source.as_ref())
                    };
                    (text, None)
                };
                let types = types.filter(|types| types.matches_source(&text));
                let indexed = types.map(|types| types.parsed());
                let mut owned_parser = false;
                let declaration;
                let mut indexed_input = None;
                let (syntax, normalized) = if let Some(scanned) = scanned {
                    match (scanned.parsed, syntax) {
                        (Some(super::ScannedParser::Owned(parsed)), None) => {
                            owned_parser = indexed.is_none();
                            (Some(Cow::Owned(parsed)), Some(scanned.lowered))
                        }
                        (Some(super::ScannedParser::Declaration(parser)), None) => {
                            declaration = parser;
                            (Some(Cow::Borrowed(&*declaration)), Some(scanned.lowered))
                        }
                        (Some(super::ScannedParser::Indexed(input)), syntax) => {
                            indexed_input = Some(input);
                            (syntax, Some(scanned.lowered))
                        }
                        (None, syntax) => (syntax, Some(scanned.lowered)),
                        (_, syntax) => (syntax, None),
                    }
                } else {
                    (syntax, None)
                };
                let normalized = normalized.filter(|root| match root {
                    super::LoweredRoot::Unchanged(_) => standalone || owned_parser,
                    // Indexed parses of one physical Python input share source-order node IDs.
                    super::LoweredRoot::Complete(..) => syntax.as_deref().is_some_and(|original| {
                        types.zip(indexed.as_ref()).is_none_or(|(types, indexed)| {
                            std::ptr::eq(&**indexed, original)
                                || indexed_input
                                    .is_some_and(|input| input == types.indexed_input(indexed))
                        })
                    }),
                });
                let module = match indexed.as_deref().map(Cow::Borrowed).or(syntax) {
                    Some(parsed) => ParsedModule { text, parsed },
                    None => ParsedModule::parse(path, text)?,
                };
                let checked_source = match origin {
                    ModuleOrigin::Program(_) => file.map(|file| file.module(registrations.modules)),
                    _ => None,
                };
                let mut body = PreparedBody::from_parsed(
                    name,
                    path.as_path(),
                    (module, normalized),
                    python_optimization,
                    origin,
                    literals.unwrap_or_default(),
                    (types, checked_source),
                )?;
                if python_optimization < 2
                    && let Some(module) = module_id
                {
                    body.prompts = &registrations.modules[module].doctest_prompts;
                }
                Ok(Initializer::Checked(Box::new(body)))
            };
            // Release each checker database after its last selected first-party file.
            layouts.retain(|facts| !facts.is_empty());
            Ok(Some(PreparedUnit {
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
            }))
        })
        .filter_map(Result::transpose)
}
