//! Emit independent boxed code for the sealed modules available to this native target.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::{fs, io};

use super::{RUNTIME_PROVIDERS, bundled_module_source, emit, imports_for_module, resolve};

const STDLIB_NAMES: &str = include_str!("../../../imre-runtime/native/stdlib-module-names.txt");

pub(crate) fn write(directory: &Path) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(|err| err.to_string())?;
    let write_module = |name: &str, file: &str, source: &str| -> io::Result<()> {
        let path = Path::new("<imre stdlib>").join(file);
        match emit::provider(name, &path, source, true) {
            Ok(Some((symbol, ir))) => fs::write(directory.join(format!("{symbol}.ll")), ir),
            Ok(None) => Ok(()),
            Err(reason) => {
                // The ordinary compiler still diagnoses a module it cannot lower when selected.
                eprintln!("skipping bundled provider {name}: {reason}");
                Ok(())
            }
        }
    };
    let mut pending: VecDeque<_> = STDLIB_NAMES
        .lines()
        .chain(RUNTIME_PROVIDERS.iter().copied())
        .map(str::to_owned)
        .collect();
    let mut seen = HashSet::new();
    while let Some(name) = pending.pop_front() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some((file, source)) = bundled_module_source(&name) else {
            continue;
        };
        write_module(&name, file, &source).map_err(|err| err.to_string())?;
        let path = Path::new(file);
        if let Ok(imports) = imports_for_module(path, &source) {
            let package = path.ends_with("__init__.py");
            for (level, imported) in imports.names {
                if let Some(resolved) = resolve(&name, package, level, &imported) {
                    for (dot, _) in resolved.match_indices('.') {
                        pending.push_back(resolved[..dot].to_owned());
                    }
                    pending.push_back(resolved);
                }
            }
        }
    }
    // These sealed variants need no source-level import. Full collections has separate code.
    write_module(
        "collections",
        "collections/__init__.py",
        super::NAMEDTUPLE_COLLECTIONS,
    )
    .and_then(|()| write_module("io", "io.py", super::SEALED_IO))
    .map_err(|err| err.to_string())
}
