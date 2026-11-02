//! Expose the compiled sources belonging to one entry of Python's import path.
use std::ffi::OsStr;
use std::path::Path;

use super::{Context, Id, NONE, Result, TRUE, Value};

impl Context {
    /// Pin a compiled search directory when the path hook creates its finder.
    pub(super) fn registered_path(&mut self, path: Id) -> Result<Id> {
        let Ok(path) = self.str_value(path) else {
            return Ok(NONE);
        };
        let path = Path::new(path);
        let bundle = self.config["bundled_root"].as_str().map(Path::new);
        let root = if bundle.is_some_and(|dir| path.starts_with(dir)) {
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
        let registered = self.sources.iter().any(|(name, entries)| {
            let leaf = OsStr::new(name.rsplit('.').next().unwrap());
            entries.iter().any(|entry| {
                let parent = entry.path.parent();
                parent == Some(&root)
                    || (entry.path.ends_with("__init__.py")
                        && parent.and_then(Path::parent) == Some(&root))
                    || (entry.path == root && entry.path.file_name() == Some(leaf))
            })
        });
        Ok(if registered {
            self.text(root.to_string_lossy())
        } else {
            NONE
        })
    }

    /// Return a ModuleSpec for a compiled package, module or namespace portion on this entry.
    pub(super) fn registered_path_spec(&mut self, name: Id, path: Id) -> Result<Id> {
        let fullname = self.str_value(name)?;
        let Some(entries) = self.sources.get(fullname) else {
            return Ok(NONE);
        };
        let leaf = fullname.rsplit('.').next().unwrap();
        if Path::new(leaf).file_name() != Some(OsStr::new(leaf)) {
            return Ok(NONE);
        }
        let stem = Path::new(self.str_value(path)?).join(leaf);
        let init = stem.join("__init__.py");
        let file = stem.with_extension("py");
        let contains = |path: &Path| entries.iter().any(|entry| entry.path == path);
        let (origin, package) = if contains(&init) {
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
            let file = self.text(file.to_string_lossy());
            let factory = self.lookup_attr(external, "SourceFileLoader")?;
            (file, self.call(factory, vec![name, file], vec![])?)
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
            let portion = self.text(stem.to_string_lossy());
            let locations = self.alloc(Value::List(vec![portion]));
            self.set_attr(spec, "submodule_search_locations", locations)?;
        }
        Ok(spec)
    }
}
