//! Prepare the registered source units selected by import resolution.
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::ModuleSyntaxError;
use super::emit::{ModuleOrigin, PhysicalFile, PreparedBody, RegisteredBodies};
use crate::frontend::module_source::ParsedModule;
use crate::ir::PhysicalModuleKind;
use crate::modules::{CheckedInitializers, ModuleId};

pub(crate) type ModuleSource<'a> = (String, PathBuf, Cow<'a, str>);

pub(super) enum Shipped<'a> {
    Checked(Result<(), String>),
    Archived(&'a str),
    Parsed(ruff_db::parsed::ParsedModuleRef),
}

#[derive(Default)]
pub(super) struct SourceInputs<'a> {
    pub(super) literal_sites: BTreeMap<usize, super::literal::LiteralSites>,
    pub(super) entry_source: Option<ParsedModule<'a, 'a>>,
    pub(super) entry: Option<(ModuleId, Option<usize>)>,
    pub(super) initializers: CheckedInitializers,
    pub(super) scanned_modules: HashMap<usize, super::ScannedModule>,
    pub(super) shipped_checks: HashMap<usize, Shipped<'a>>,
}

pub(super) enum Initializer<'a> {
    Checked(Rc<PreparedBody<'a>>),
    Archived(&'a str),
}

pub(super) struct PreparedUnit<'a> {
    pub(super) module: &'a ModuleSource<'a>,
    pub(super) builtin: bool,
    pub(super) physical: Option<(&'a PhysicalFile, PhysicalModuleKind<'a>)>,
    pub(super) zip: Option<&'a [u8; 32]>,
    pub(super) initializer: Result<Initializer<'a>, &'a ModuleSyntaxError>,
}

/// Prepare an SDK initializer that needs no application-owned inputs.
pub(super) fn shipped_body<'a>(
    name: &'a str,
    path: &'a Path,
    input: super::emit::ParsedInput<'a, '_>,
    python_optimization: i32,
) -> Result<PreparedBody<'a>, String> {
    PreparedBody::from_parsed(
        name,
        path,
        input,
        python_optimization,
        ModuleOrigin::Bundled(None),
        Default::default(),
        Default::default(),
    )
}

/// Lower each source on demand for checking or native emission.
pub(super) fn units<'a>(
    (files, builtins): (&'a [ModuleSource<'a>], &'a [ModuleSource<'a>]),
    SourceInputs {
        mut entry_source,
        entry,
        mut initializers,
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
    // Only retain preparation for a physical SDK path selected under multiple names.
    let mut aliases: HashMap<_, Option<Rc<PreparedBody<'_>>>> = {
        let mut seen = HashSet::new();
        modules
            .clone()
            .filter_map(|((_, path, _), _)| {
                (path.starts_with(bundled_root) && !seen.insert(path)).then_some((path, None))
            })
            .collect()
    };
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
            let bundled = path.starts_with(bundled_root);
            let origin = registrations
                .provider_origin(unit, bundled)
                .unwrap_or_else(|| {
                    ModuleOrigin::Program(module_id.and_then(|id| registrations.functions.get(&id)))
                });
            // Only the selected bundled initializer can own its documented inputs.
            let bundled_prompts = bundled
                .then(|| registrations.bundled_doctest_prompts.get(name))
                .flatten();
            let standalone = bundled_prompts.is_none()
                && unit != 0
                && matches!(origin, ModuleOrigin::Bundled(_))
                && physical.is_none()
                && zip.is_none()
                // A checked parser for this SDK source also serves the standalone initializer.
                && checked.is_none_or(|unit| unit.run_path.is_none())
                && literals.is_none()
                && types.is_none();
            let alias =
                (standalone && checked.is_none() && matches!(origin, ModuleOrigin::Bundled(None)))
                    .then(|| aliases.get_mut(path))
                    .flatten();
            let shipped = shipped_checks.remove(&unit).filter(|_| standalone);
            let initializer = if !builtin && let Some(error) = module_errors.get(path) {
                Err(error.as_ref())
            } else if let Some(Shipped::Checked(check)) = shipped {
                return check.map(|()| None);
            } else if let Some(Shipped::Archived(symbol)) = shipped {
                Ok(Initializer::Archived(symbol))
            } else if let Some(body) = alias.as_deref().and_then(Option::as_ref)
                && body.shares_standalone_source(name, source)
            {
                Ok(Initializer::Checked(Rc::clone(body)))
            } else {
                // Preserve the loader's digest while compiling its normalized source.
                let (text, syntax) = if let Some(entry) = discovered {
                    (entry.text, Some(entry.parsed))
                } else if let Some(file) = physical {
                    let kind = file.source_kind.unwrap_or(crate::SourceKind::File);
                    (crate::normalize_source(source, kind)?, None)
                } else if zip.is_some()
                    || physical_files
                        .get(path)
                        .is_some_and(|file| file.digest.is_some())
                {
                    (
                        crate::normalize_source(source, crate::SourceKind::DecodedFile)?,
                        None,
                    )
                } else {
                    let parsed = match &shipped {
                        Some(Shipped::Parsed(parsed)) => Some(Cow::Borrowed(&**parsed)),
                        _ => None,
                    };
                    (Cow::Borrowed(source.as_ref()), parsed)
                };
                let types = types.filter(|types| types.matches_source(&text));
                let indexed = types.map(|types| types.parsed());
                let declaration;
                let mut indexed_input = None;
                let (syntax, normalized) = if let Some(scanned) = scanned {
                    match (scanned.parsed, syntax) {
                        (Some(super::ScannedParser::Owned(parsed)), None) => {
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
                // Indexed parses of one physical Python input share source-order node IDs.
                let normalized = normalized.filter(|_| {
                    syntax.as_deref().map_or(standalone, |original| {
                        types.zip(indexed.as_ref()).is_none_or(|(types, indexed)| {
                            std::ptr::eq(&**indexed, original)
                                || indexed_input
                                    .is_some_and(|input| input == types.indexed_input(indexed))
                        })
                    })
                });
                let parser = indexed.as_deref().map(Cow::Borrowed).or(syntax);
                let module = ParsedModule::with_parser(path, text, parser)?;
                let checked_source = file.map(|file| file.module(registrations.modules));
                let initializer = file.zip(checked_source).and_then(|(file, source)| {
                    let id = file.id();
                    let owner = match origin {
                        ModuleOrigin::Program(_) => {
                            module_id == Some(id) && source.is_program_source()
                        }
                        // Only imported initializers were checked with their provider globals.
                        ModuleOrigin::Provider(_) => entry.is_none_or(|(root, _)| root != id),
                        _ => false,
                    };
                    let winner = match entry {
                        Some((root, main)) if root == id => main == Some(unit),
                        _ => source.name == *name,
                    };
                    if owner && winner {
                        initializers.remove(&id)
                    } else {
                        None
                    }
                });
                let mut body = PreparedBody::from_parsed(
                    name,
                    path.as_path(),
                    (module, normalized),
                    python_optimization,
                    origin,
                    literals.unwrap_or_default(),
                    (
                        types,
                        checked_source,
                        initializer,
                        module_id.and_then(|id| registrations.call_operands.get(&id)),
                    ),
                )?;
                body.prompts = bundled_prompts
                    .or_else(|| {
                        checked_source
                            .filter(|_| !bundled)
                            .map(|source| &source.doctest_prompts)
                    })
                    .or_else(|| module_id.map(|id| &registrations.modules[id].doctest_prompts))
                    .map_or(&[], Vec::as_slice);
                let body = Rc::new(body);
                if let Some(alias) = alias {
                    *alias = Some(Rc::clone(&body));
                }
                Ok(Initializer::Checked(body))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_sdk_source_keeps_both_registrations() {
        let root = Path::new("<imre stdlib>");
        let path = root.join("importlib/_bootstrap_external.py");
        let source = "def _path_stat(path):\n    return _os.stat(path)\n";
        let modules = [
            ("__main__".into(), "app.py".into(), "".into()),
            ("first".into(), root.join("first.py"), "x = 1".into()),
            (
                "importlib._bootstrap_external".into(),
                path.clone(),
                source.into(),
            ),
            (
                "_frozen_importlib_external".into(),
                path.clone(),
                source.into(),
            ),
            ("other".into(), path, "def changed(): return 42".into()),
        ];
        let errors = HashMap::new();
        let registered = RegisteredBodies::default();
        let prepare = || {
            units(
                (&modules, &[]),
                SourceInputs::default(),
                0,
                root,
                &errors,
                &registered,
                vec![],
            )
        };
        let bodies = prepare()
            .skip(1)
            .map(|unit| match unit.unwrap().initializer {
                Ok(Initializer::Checked(body)) => body,
                _ => panic!("expected the checked source"),
            })
            .collect::<Vec<_>>();
        assert!(Rc::ptr_eq(&bodies[1], &bodies[2]));
        assert!(!Rc::ptr_eq(&bodies[2], &bodies[3]));
        let standalone: Vec<_> = bodies
            .iter()
            .zip(&modules[1..])
            .map(|(body, (name, _, _))| {
                let (symbol, code) = body.provider_body(name, None).unwrap();
                let ir = code.llvm_ir(&symbol, "");
                assert_eq!(
                    ir.contains("<frozen importlib._bootstrap_external>"),
                    name == "_frozen_importlib_external"
                );
                (symbol, ir)
            })
            .collect();

        // The only archived provider can be before, within, or after the shared pair.
        for archived in std::iter::once(None).chain((0..standalone.len()).map(Some)) {
            let providers = archived
                .map(|i| standalone[i].0.clone())
                .into_iter()
                .collect();
            let emit = |layout| {
                let mut output = crate::ProgramOutput::new(layout);
                let used = output
                    .emit_registered(prepare(), None, "{}", 0, &providers)
                    .unwrap();
                assert_eq!(used, archived.is_some());
                output
            };
            let whole = emit(crate::CodegenLayout::Whole);
            let split = emit(crate::CodegenLayout::SplitWithWhole);
            let ir = &whole.compilation.units[0].llvm_ir;
            assert!(
                split.whole.as_deref() == Some(ir.as_str()),
                "whole LLVM differs with archive {archived:?}"
            );
            for (i, ((symbol, expected), (name, _, _))) in
                standalone.iter().zip(&modules[1..]).enumerate()
            {
                let unit = split.compilation.units.iter().find(|u| &u.name == name);
                if archived == Some(i) {
                    assert!(unit.is_none() && ir.contains(&format!("ptr @{symbol})")));
                } else {
                    assert!(
                        unit.is_some_and(|u| &u.llvm_ir == expected),
                        "standalone {name} with archive {archived:?}"
                    );
                }
            }
            if archived.is_none() {
                let entries: Vec<_> = ir
                    .lines()
                    .filter(|line| line.contains("call void @imre_dyn_register_source("))
                    .map(|line| line.rsplit_once("ptr ").unwrap().1.trim_end_matches(')'))
                    .collect();
                let [_main, _first, file, frozen, changed] = entries.as_slice() else {
                    panic!("expected five source registrations");
                };
                assert_ne!(file, frozen);
                assert_ne!(frozen, changed);
                assert!(ir.contains(&format!(
                    "define internal i64 {frozen}(ptr %ctx, i64 %scope) {{\n  %result = tail call i64 {file}("
                )));
                assert_eq!(ir.matches(" = tail call i64 @code").count(), 1);
            }
        }
    }
}
