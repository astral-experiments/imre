//! Specs and executable loaders for sources registered with the native import system.
use std::path::{Path, PathBuf};

use crate::objects::exceptions::string_repr;

use super::{
    Context, stdlib,
    value::{self, Code, Error, FALSE, Id, NONE, Result, TRUE, Value},
};

/// A compiled body, a newly discovered filesystem source, or its ZIP loader.
#[derive(Clone, Copy)]
pub(super) enum SourceBody {
    Compiled(Code),
    Filesystem,
    Zip(Id),
}

impl SourceBody {
    fn loader(self) -> Id {
        match self {
            Self::Compiled(_) | Self::Filesystem => NONE,
            Self::Zip(loader) => loader,
        }
    }
}

impl Context {
    /// Bootstrap modules remain available even while ordinary frozen imports are disabled.
    pub(super) fn is_frozen(&self, name: &str) -> bool {
        match name {
            "_frozen_importlib" | "_frozen_importlib_external" => true,
            "zipimport" => cfg!(unix),
            "stat" => {
                cfg!(any(target_os = "linux", target_os = "macos")) && self.frozen_override >= 0
            }
            "__hello__" => self.frozen_override >= 0,
            _ => false,
        }
    }

    pub(super) fn imp_call(
        &mut self,
        method: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("_imp.{method}() takes no keyword arguments"),
            ));
        }
        let [arg] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "_imp.{method}() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        };
        if method == "is_frozen" {
            // Frozen module names contain only scalar values.
            if matches!(
                self.heap.get(self.scalar_value(*arg)),
                Value::SurrogateStr(_)
            ) {
                return Ok(FALSE);
            }
            let name = self.str_value(*arg)?;
            return Ok(if self.is_frozen(name) { TRUE } else { FALSE });
        }
        let value = self
            .index_integer(*arg)?
            .to_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(|| {
                Error::new("OverflowError", "Python int too large to convert to C int")
            })?;
        match method {
            "_override_frozen_modules_for_tests" => {
                self.frozen_override = value;
                Ok(NONE)
            }
            // Imre programs currently run in a single main interpreter.
            "_override_multi_interp_extensions_check" => Err(Error::new(
                "RuntimeError",
                "_imp._override_multi_interp_extensions_check() cannot be used in the main interpreter",
            )),
            _ => unreachable!("unregistered _imp attribute"),
        }
    }

    /// Select a package or module in search order, including late files for resource lookups.
    pub(super) fn source_on_path(
        &mut self,
        name: &str,
        child: &str,
        search: Id,
        top_level: bool,
        locations: &mut Vec<PathBuf>,
    ) -> Result<Option<(PathBuf, SourceBody)>> {
        let mut namespace = None;
        // A bundled package contributes its own __path__. A first-party
        // regular package can never acquire one of its children implicitly.
        let search = self.items(search)?;
        let bundle = self.config["bundled_root"].as_str().map(PathBuf::from);
        let roots = search
            .into_iter()
            .filter_map(|id| self.str_value(id).ok().map(PathBuf::from))
            .collect::<Vec<_>>();
        for spelling in roots
            .into_iter()
            .chain(bundle.iter().filter(|_| top_level).cloned())
        {
            let root = if spelling.as_os_str().is_empty() {
                std::env::current_dir()
            } else if bundle.as_ref().is_some_and(|dir| spelling.starts_with(dir)) {
                Ok(spelling.clone())
            } else {
                std::path::absolute(&spelling)
            };
            let Ok(root) = root else { continue };
            let stem = root.join(child);
            let choices = self.sources.get(name).map_or(&[][..], Vec::as_slice);
            let disk = !bundle.as_ref().is_some_and(|dir| root.starts_with(dir))
                && Path::new(child).file_name() == Some(std::ffi::OsStr::new(child));
            for path in [stem.join("__init__.py"), stem.with_extension("py")] {
                let entry = choices.iter().find(|m| m.path == path).map(|m| m.code);
                if entry.is_some() || (disk && path.is_file()) {
                    locations.clear();
                    if path.ends_with("__init__.py") {
                        locations.push(stem);
                    }
                    return Ok(Some((
                        path,
                        entry.map_or(SourceBody::Filesystem, SourceBody::Compiled),
                    )));
                }
            }
            let entry = choices.iter().find(|m| m.path == stem).map(|m| m.code);
            if entry.is_some() || (disk && stem.is_dir()) {
                locations.push(stem.clone());
                namespace.get_or_insert((
                    stem,
                    entry.map_or(SourceBody::Filesystem, SourceBody::Compiled),
                ));
                continue;
            }
            // A path below an archive is virtual. Preserve its spelling because the
            // loader exposes it in __file__, __path__, and resource names.
            if cfg!(unix)
                && !spelling.as_os_str().is_empty()
                && spelling
                    .ancestors()
                    .find_map(|p| p.metadata().ok())
                    .is_some_and(|m| m.is_file())
                && let Some((origin, loader)) = self.zip_source(&spelling, name)?
            {
                if loader == NONE {
                    locations.push(origin.clone());
                    namespace.get_or_insert((origin, SourceBody::Zip(NONE)));
                } else {
                    locations.clear();
                    if origin.ends_with("__init__.py") {
                        locations.push(
                            origin
                                .parent()
                                .expect("ZIP package contains a member")
                                .to_owned(),
                        );
                    }
                    return Ok(Some((origin, SourceBody::Zip(loader))));
                }
            }
        }
        Ok(namespace)
    }

    /// Resolve one archive or virtual archive prefix through its cached path importer.
    fn zip_source(&mut self, root: &Path, name: &str) -> Result<Option<(PathBuf, Id)>> {
        let module = self.module("zipimport")?;
        let find = self.lookup_attr(module, "_find_at")?;
        let path = self.text(root.to_string_lossy());
        let name = self.text(name);
        let result = self.call(find, vec![path, name], vec![])?;
        if result == NONE {
            return Ok(None);
        }
        let parts = self.items(result)?;
        let [origin, loader] = parts.as_slice() else {
            return Err(Error::new("TypeError", "expected a ZIP origin and loader"));
        };
        Ok(Some((PathBuf::from(self.str_value(*origin)?), *loader)))
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
            flags: None,
        }))
    }

    /// Return the compiled body for this loader, or None to read the source through get_data.
    pub(super) fn registered_code(&mut self, name: Id, path: Id) -> Result<Id> {
        let name = self.str_value(name)?;
        let filename = self.str_value(path)?;
        if let Some(source) = self
            .sources
            .get(name)
            .and_then(|sources| sources.iter().find(|s| s.path == Path::new(filename)))
        {
            let (entry, path) = (source.code, source.path.clone());
            return Ok(self.module_code(entry, &path));
        }
        Ok(NONE)
    }

    fn file_spec(&mut self, name: Id, origin: Id, locations: Id, loader: Id) -> Result<Id> {
        let importlib = self.module("importlib")?;
        let make = self.lookup_attr(importlib, "_file_spec")?;
        self.call(make, vec![name, origin, locations, loader], vec![])
    }

    pub(super) fn initialize_source(
        &mut self,
        module: Id,
        scope: Id,
        name: Id,
        origin: Id,
        loader: Id,
    ) -> Result<Id> {
        let locations = self.scope_name(scope, "__path__").unwrap_or(NONE);
        let spec = self.file_spec(name, origin, locations, loader)?;
        let loader = self.lookup_attr(spec, "loader")?;
        self.bind(scope, "__spec__", spec);
        self.bind(scope, "__loader__", loader);
        let exec = self.lookup_attr(loader, "exec_module")?;
        self.set_attr(spec, "_initializing", TRUE)?;
        let result = self.call(exec, vec![module], vec![]);
        self.set_attr(spec, "_initializing", FALSE)?;
        result
    }

    pub(super) fn find_spec(
        &mut self,
        fullname: &str,
        original: &str,
        for_resource: bool,
    ) -> Result<Id> {
        let key = self.text(fullname);
        if let Some(module) = self.registered_module(key)? {
            if module == NONE {
                return Ok(NONE);
            }
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
                    if for_resource {
                        return Ok(NONE);
                    }
                    Err(Error::new(
                        "NotImplementedError",
                        "find_spec requires a registered source file",
                    ))
                }
                Err(e) if self.is_attribute_error(&e) => {
                    let mut error =
                        Error::new("ValueError", format!("{original}.__spec__ is not set"));
                    error.context = self.raised_instance(e);
                    error.tracing().cause = Some(NONE);
                    Err(error)
                }
                spec => spec,
            };
        }
        let (child, search, top_level) =
            if let Some((parent_name, child)) = fullname.rsplit_once('.') {
                let parent = self
                    .find_module(parent_name)?
                    .ok_or_else(|| Self::missing_module(parent_name))?;
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
                        error.context = cause;
                        return Err(error);
                    }
                    path => path?,
                };
                (child, path, false)
            } else {
                (fullname, self.sys_path, true)
            };
        if stdlib::is_module(fullname) || self.is_frozen(fullname) {
            if for_resource {
                return Ok(NONE);
            }
            return Err(Error::new(
                "NotImplementedError",
                "find_spec requires a registered source file",
            ));
        }
        let mut locations = Vec::new();
        let Some((path, body)) =
            self.source_on_path(fullname, child, search, top_level, &mut locations)?
        else {
            return Ok(NONE);
        };
        if locations.contains(&path) {
            if for_resource {
                return Ok(NONE);
            }
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
        self.file_spec(name, origin, locations, body.loader())
    }
}
