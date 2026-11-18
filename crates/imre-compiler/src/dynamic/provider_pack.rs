//! Emit independent boxed code for the sealed modules available to this native target.

use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::{fs, io};

use super::{
    Imports, add_late_bundled_imports, bundled_module_source, prepare, runtime_sources,
    scan_imports_for_preparation,
};
use crate::SourceKind;

use crate::frontend::module_source::ParsedModule;

const STDLIB_NAMES: &str = include_str!("../../../imre-runtime/native/stdlib-module-names.txt");

pub(crate) fn write(directory: &Path) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(|err| err.to_string())?;
    let mut syntax = None;
    let mut write_module = |name: &str,
                            file: &str,
                            source: &str,
                            pending: Option<&mut ProviderQueue>|
     -> io::Result<()> {
        let module = (
            name.to_owned(),
            Path::new(super::BUNDLED_SOURCE_ROOT).join(file),
            source.into(),
        );
        let path = &module.1;
        let checked = crate::analysis::check_native_syntax(
            Path::new("/"),
            &mut syntax,
            (None, &[]),
            &module,
            true,
            |_| false,
            &mut Default::default(),
        )
        .ok()
        .flatten();
        let discovered = pending.and_then(|pending| {
            let (imports, input) = scan_imports_for_preparation(
                source,
                SourceKind::File,
                |_| checked.as_deref().filter(|_| !source.contains('\r')),
                None,
                Some(source),
                None,
            )
            .ok()?;
            let package = crate::modules::import_package(name, path.ends_with("__init__.py"));
            for (level, imported) in imports.names {
                if let Some(resolved) = crate::modules::resolve_import(package, level, &imported) {
                    pending.enqueue_import(&resolved);
                }
            }
            input
        });
        let input = match discovered {
            Some(input) => Ok(input),
            None => {
                let parser = checked.as_deref().map(Cow::Borrowed);
                ParsedModule::with_parser(path, source.into(), parser).map(Into::into)
            }
        };
        let compiled = input
            .and_then(|input| prepare::shipped_body(name, path, input, 0))
            .and_then(|body| {
                body.has_body_statements
                    .then(|| body.provider_body(name, None))
                    .transpose()
            });
        match compiled {
            // The launcher emits entries for statement-free modules in its own unit.
            Ok(None) => Ok(()),
            Ok(Some((symbol, body))) => {
                let seal = checked
                    .as_ref()
                    .and_then(|_| super::provider_archive::seal(&symbol, name, path, source));
                fs::write(
                    directory.join(format!("{symbol}.ll")),
                    body.llvm_ir(&symbol, seal.as_deref().unwrap_or_default()),
                )
            }
            Err(reason) => {
                // The ordinary compiler still diagnoses a module it cannot lower when selected.
                eprintln!("skipping bundled provider {name}: {reason}");
                Ok(())
            }
        }
    };
    // The launcher also registers sources without an import, including sealed variants.
    let defaults: Vec<_> = runtime_sources(|_| false, None).collect();
    let mut queue = ProviderQueue::default();
    for name in STDLIB_NAMES
        .lines()
        .chain(defaults.iter().map(|(name, _, _)| *name))
    {
        queue.insert(name);
    }
    loop {
        let Some(name) = queue.pending.pop_front() else {
            // A release can serve callers with any of the supported late-import paths.
            let mut late = Imports::default();
            add_late_bundled_imports(&mut late, &queue.seen, true, true, true, true);
            if late.names.is_empty() {
                break;
            }
            for (_, name) in late.names {
                queue.enqueue_import(&name);
            }
            continue;
        };
        let (file, source) = if let Some(provider) = bundled_module_source(&name, true) {
            provider
        } else if let Some(member) = crate::zip_entry::packaged_member(&name) {
            (member, crate::zip_entry::installed_source(member)?)
        } else {
            continue;
        };
        write_module(&name, file, source, Some(&mut queue)).map_err(|err| err.to_string())?;
    }
    for (name, file, source) in super::builtin_sources(std::env::consts::OS) {
        write_module(name, file, source, None).map_err(|err| err.to_string())?;
    }
    for (name, file, source) in defaults {
        if bundled_module_source(name, true) != Some((file, source)) {
            write_module(name, file, source, None).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

#[derive(Default)]
struct ProviderQueue {
    seen: HashSet<String>,
    pending: VecDeque<String>,
}

impl ProviderQueue {
    fn insert(&mut self, name: &str) {
        if !self.seen.contains(name) {
            self.seen.insert(name.to_owned());
            self.pending.push_back(name.to_owned());
        }
    }

    fn enqueue_import(&mut self, name: &str) {
        for (dot, _) in name.match_indices('.') {
            self.insert(&name[..dot]);
        }
        self.insert(name);
    }
}
