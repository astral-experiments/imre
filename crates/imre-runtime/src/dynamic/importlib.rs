//! Specs and executable loaders for sources registered with the native import system.
use std::path::{Path, PathBuf};

use crate::objects::exceptions::string_repr;

use super::{
    Context, stdlib,
    value::{self, Code, Error, FALSE, Id, NONE, Result, TRUE, Value},
};

impl Context {
    /// Select the same registered module for a lookup and for an executing import.
    pub(super) fn registered_source(
        &mut self,
        name: &str,
        child: &str,
        search: Id,
        top_level: bool,
        locations: &mut Vec<PathBuf>,
    ) -> Result<Option<(PathBuf, Code)>> {
        let mut selected = None;
        let mut namespace = None;
        // A bundled package contributes its own __path__. A first-party
        // regular package can never acquire one of its children implicitly.
        let search = self.items(search)?;
        let bundle = self.config["bundled_root"]
            .as_str()
            .map(std::path::Path::new);
        let roots = search
            .iter()
            .filter_map(|&id| self.str_value(id).ok().map(std::path::Path::new));
        for root in roots.chain(bundle.filter(|_| top_level)) {
            let root = if root.as_os_str().is_empty() {
                std::env::current_dir()
            } else if bundle.is_some_and(|dir| root.starts_with(dir)) {
                Ok(root.to_owned())
            } else {
                std::path::absolute(root)
            };
            let Ok(root) = root else { continue };
            let stem = root.join(child);
            let Some(choices) = self.sources.get(name) else {
                continue;
            };
            if let Some(module) = choices
                .iter()
                .find(|m| m.path == stem.join("__init__.py") || m.path == stem.with_extension("py"))
            {
                locations.clear();
                if module.path.ends_with("__init__.py") {
                    locations.push(stem);
                }
                selected = Some((module.path.clone(), module.code));
                break;
            }
            if let Some(module) = choices.iter().find(|m| m.path == stem) {
                locations.push(stem);
                namespace.get_or_insert((module.path.clone(), module.code));
            }
        }
        Ok(selected.or(namespace))
    }

    pub(super) fn module_code(&mut self, entry: Code, path: &Path) -> Id {
        let filename = self.text(path.to_string_lossy());
        let name_id = self.text("<module>");
        self.alloc(Value::Code(value::CodeObject {
            body: value::Body::Native(entry),
            parameters: NONE,
            nested: Vec::new(),
            freevars: Vec::new(),
            closure_supported: false,
            name: name_id,
            qualname: name_id,
            doc: NONE,
            filename,
            first_line: 1,
            module_definition: true,
            coroutine: false,
        }))
    }

    /// Return the compiled body belonging to this filesystem loader.
    pub(super) fn registered_code(&mut self, name: Id, path: Id) -> Result<Id> {
        let name = self.str_value(name)?;
        let path = Path::new(self.str_value(path)?);
        let source = self
            .sources
            .get(name)
            .and_then(|sources| sources.iter().find(|s| s.path == path))
            .ok_or_else(|| {
                Error::new(
                    "NotImplementedError",
                    format!("no registered source for {name:?} at {path:?}"),
                )
            })?;
        let (entry, path) = (source.code, source.path.clone());
        Ok(self.module_code(entry, &path))
    }

    fn file_spec(&mut self, name: Id, origin: Id, locations: Id) -> Result<Id> {
        let importlib = self.module("importlib")?;
        let make = self.lookup_attr(importlib, "_file_spec")?;
        self.call(make, vec![name, origin, locations], vec![])
    }

    pub(super) fn initialize_source(
        &mut self,
        module: Id,
        scope: Id,
        name: Id,
        origin: Id,
    ) -> Result<Id> {
        let locations = self.scope_name(scope, "__path__").unwrap_or(NONE);
        let spec = self.file_spec(name, origin, locations)?;
        let loader = self.lookup_attr(spec, "loader")?;
        self.bind(scope, "__spec__", spec);
        self.bind(scope, "__loader__", loader);
        let exec = self.lookup_attr(loader, "exec_module")?;
        self.set_attr(spec, "_initializing", TRUE)?;
        let result = self.call(exec, vec![module], vec![]);
        self.set_attr(spec, "_initializing", FALSE)?;
        result
    }

    pub(super) fn find_spec(&mut self, fullname: &str, original: &str) -> Result<Id> {
        if let Some(&module) = self.modules.get(fullname) {
            return match self.lookup_attr(module, "__spec__") {
                Ok(NONE) => Err(Error::new(
                    "ValueError",
                    format!("{original}.__spec__ is None"),
                )),
                Err(e)
                    if self.is_attribute_error(&e)
                        && (stdlib::is_module(fullname)
                            || matches!(
                                self.heap.get(module),
                                Value::Module { origin: None, .. }
                            )) =>
                {
                    Err(Error::new(
                        "NotImplementedError",
                        "find_spec requires a registered source file",
                    ))
                }
                Err(e) if self.is_attribute_error(&e) => {
                    let mut error =
                        Error::new("ValueError", format!("{original}.__spec__ is not set"));
                    error.tracing().context = Some(self.raised_instance(e));
                    error.tracing().cause = Some(NONE);
                    Err(error)
                }
                spec => spec,
            };
        }
        let (child, search, top_level) =
            if let Some((parent_name, child)) = fullname.rsplit_once('.') {
                let parent = self.module(parent_name)?;
                let path = match self.lookup_attr(parent, "__path__") {
                    Err(cause) if self.is_attribute_error(&cause) => {
                        let mut error = Error::new(
                            "ModuleNotFoundError",
                            format!(
                                "__path__ attribute not found on {} while trying to find {}",
                                string_repr(parent_name),
                                string_repr(fullname)
                            ),
                        )
                        .with_import(Some(fullname), None, None);
                        let cause = self.raised_instance(cause);
                        error.tracing().cause = Some(cause);
                        error.tracing().context = Some(cause);
                        return Err(error);
                    }
                    path => path?,
                };
                (child, path, false)
            } else {
                (fullname, self.sys_path, true)
            };
        if stdlib::is_module(fullname) {
            return Err(Error::new(
                "NotImplementedError",
                "find_spec requires a registered source file",
            ));
        }
        let mut locations = Vec::new();
        let Some((path, _)) =
            self.registered_source(fullname, child, search, top_level, &mut locations)?
        else {
            return Ok(NONE);
        };
        if locations.contains(&path) {
            return Err(Error::new(
                "NotImplementedError",
                "find_spec for namespace packages",
            ));
        }
        let name = self.text(fullname);
        let origin = self.text(path.to_string_lossy());
        let locations = if locations.is_empty() {
            NONE
        } else {
            let values = locations
                .iter()
                .map(|p| self.text(p.to_string_lossy()))
                .collect();
            self.alloc(Value::List(values))
        };
        self.file_spec(name, origin, locations)
    }
}
