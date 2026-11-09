//! Emit independent boxed code for the sealed modules available to this native target.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::{fs, io};

use super::{
    Imports, add_late_bundled_imports, bundled_module_source, emit, imports_for_module,
    runtime_sources,
};

const STDLIB_NAMES: &str = include_str!("../../../imre-runtime/native/stdlib-module-names.txt");

pub(crate) fn write(directory: &Path) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(|err| err.to_string())?;
    let write_module = |name: &str, file: &str, source: &str| -> io::Result<()> {
        let path = Path::new("<imre stdlib>").join(file);
        match emit::initializer(name, &path, source, true, emit::ModuleOrigin::Bundled) {
            Ok(Some((symbol, ir))) => fs::write(directory.join(format!("{symbol}.ll")), ir),
            Ok(None) => Ok(()),
            Err(reason) => {
                // The ordinary compiler still diagnoses a module it cannot lower when selected.
                eprintln!("skipping bundled provider {name}: {reason}");
                Ok(())
            }
        }
    };
    // The launcher also registers sources without an import, including sealed variants.
    let defaults: Vec<_> = runtime_sources(|_| false, None).collect();
    let mut pending: VecDeque<_> = STDLIB_NAMES
        .lines()
        .chain(defaults.iter().map(|(name, _, _)| *name))
        .map(str::to_owned)
        .collect();
    let mut seen = HashSet::new();
    loop {
        let Some(name) = pending.pop_front() else {
            // A release can serve callers with any of the supported late-import paths.
            let mut late = Imports::default();
            add_late_bundled_imports(&mut late, &seen, true, true, true, true);
            if late.names.is_empty() {
                break;
            }
            for (_, name) in late.names {
                enqueue(&mut pending, name);
            }
            continue;
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some((file, source)) = bundled_module_source(&name, true) else {
            continue;
        };
        write_module(&name, file, &source).map_err(|err| err.to_string())?;
        let path = Path::new(file);
        if let Ok(imports) = imports_for_module(path, &source) {
            let package = crate::modules::import_package(&name, path.ends_with("__init__.py"));
            for (level, imported) in imports.names {
                if let Some(resolved) = crate::modules::resolve_import(package, level, &imported) {
                    enqueue(&mut pending, resolved);
                }
            }
        }
    }
    for (name, file, source) in super::builtin_sources(std::env::consts::OS) {
        write_module(name, file, source).map_err(|err| err.to_string())?;
    }
    for (name, file, source) in defaults {
        if bundled_module_source(name, true)
            .as_ref()
            .is_none_or(|(path, text)| *path != file || text.as_ref() != source)
        {
            write_module(name, file, source).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

fn enqueue(pending: &mut VecDeque<String>, name: String) {
    for (dot, _) in name.match_indices('.') {
        pending.push_back(name[..dot].to_owned());
    }
    pending.push_back(name);
}
