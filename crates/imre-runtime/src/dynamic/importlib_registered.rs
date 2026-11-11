//! Expose the compiled sources belonging to one entry of Python's import path.
use std::ffi::OsStr;
use std::path::Path;

use super::{Context, Id, NONE, Result, TRUE, Value};

impl Context {
    /// Map the launcher's absolute bundle entry to its compiled source registry.
    pub(super) fn registry_root<'a>(&'a self, path: &'a Path) -> &'a Path {
        if self.config["bundle_entry"].as_str().map(Path::new) == Some(path)
            && let Some(bundle) = self.config["bundled_root"].as_str()
        {
            Path::new(bundle)
        } else {
            path
        }
    }

    /// Pin a compiled search directory when the path hook creates its finder.
    pub(super) fn registered_path(&mut self, path: Id) -> Result<Id> {
        let Ok(path) = self.import_path(path) else {
            return Ok(NONE);
        };
        let path = self.registry_root(&path);
        let bundle = self.config["bundled_root"].as_str().map(Path::new);
        // The finder exposes this spelling in ModuleSpec.origin and __file__.
        let root = if path.is_absolute() || bundle.is_some_and(|dir| path.starts_with(dir)) {
            path.to_owned()
        } else if path.as_os_str().is_empty() {
            let Ok(cwd) = std::env::current_dir() else {
                return Ok(NONE);
            };
            cwd
        } else {
            let Ok(root) = std::path::absolute(path) else {
                return Ok(NONE);
            };
            root
        };
        if let Some((current, saved)) = &self.saved_entry_root
            && current == &root
        {
            let saved = self.text_codepoints(saved.clone());
            let root = self.import_path_text(&root)?;
            return Ok(self.alloc(Value::Tuple(vec![root, saved])));
        }
        let registered = self.sources.iter().any(|(name, entries)| {
            let leaf = OsStr::new(name.rsplit('.').next().unwrap());
            entries.iter().any(|entry| {
                if !entry.in_file_finder() {
                    return false;
                }
                let parent = entry.path.parent();
                parent == Some(&root)
                    || (entry.path.ends_with("__init__.py")
                        && parent.and_then(Path::parent) == Some(&root))
                    || (entry.path == root && entry.path.file_name() == Some(leaf))
            })
        });
        if registered {
            self.import_path_text(&root)
        } else {
            Ok(NONE)
        }
    }

    /// Return a ModuleSpec for a compiled package, module or namespace portion on this entry.
    pub(super) fn registered_path_spec(&mut self, name: Id, path: Id) -> Result<Id> {
        let path = self.import_path(path)?;
        let fullname = self.str_value(name)?;
        let leaf = fullname.rsplit('.').next().unwrap();
        if Path::new(leaf).file_name() != Some(OsStr::new(leaf)) {
            return Ok(NONE);
        }
        let stem = path.join(leaf);
        let init = stem.join("__init__.py");
        let file = stem.with_extension("py");
        let contains = |path: &Path| self.file_sources(fullname).any(|entry| entry.path == path);
        let has_init = contains(&init);
        // The bundled loader installs a native package's exports before executing its
        // registered __init__.py. Native extensions can also exist without a Python file.
        if self.config["bundled_root"].as_str().map(OsStr::new) == Some(path.as_os_str())
            && (imre_ast::providers::is_native_fallback(fullname)
                || (has_init && imre_ast::providers::is_native_package(fullname)))
        {
            let loader = self.lookup_attr(self.bootstrap_module, "_BundledImporter")?;
            let find = self.lookup_attr(loader, "find_spec")?;
            return self.call(find, vec![name], Vec::new());
        }

        let (origin, package) = if has_init {
            (Some(init), true)
        } else if contains(&file) {
            (Some(file), false)
        } else if contains(&stem) {
            (None, true)
        } else {
            return Ok(NONE);
        };

        // Use the installing core, including during a fresh import of the public importlib package.
        // spec_from_file_location would make the virtual bundled filename absolute.
        let external = self.finder_external;
        let bootstrap = self.lookup_attr(external, "_bootstrap")?;
        let spec_type = self.lookup_attr(bootstrap, "ModuleSpec")?;
        let (origin, loader) = if let Some(file) = origin {
            let file = self.import_path_text(&file)?;
            let factory = self.lookup_attr(external, "SourceFileLoader")?;
            let loader = self.call(factory, vec![name, file], vec![])?;
            self.bind_loader_source(loader, name, file)?;
            (file, loader)
        } else {
            (NONE, NONE)
        };
        let spec = self.call(
            spec_type,
            vec![name, loader],
            vec![("origin".into(), origin)],
        )?;
        if origin != NONE {
            self.set_attr(spec, "has_location", TRUE)?;
        }
        if package {
            let portion = self.import_path_text(&stem)?;
            let locations = self.alloc(Value::List(vec![portion]));
            self.set_attr(spec, "submodule_search_locations", locations)?;
        }
        Ok(spec)
    }
}
