//! Connect the running interpreter's import registry to its native module initializers.
use std::path::{Path, PathBuf};

use crate::objects::exceptions::string_repr;

use super::{
    Context, SourceModule,
    importlib::SourceBody,
    stdlib,
    value::{Error, Id, NONE, Result, Value},
};

impl Context {
    /// The bootstrap modules are initialized before user code can replace any import registry.
    pub(super) fn install_import_registry(&mut self) -> Result<()> {
        let core = self.module("_frozen_importlib")?;
        self.bootstrap_module = core;
        let external = self.module("_frozen_importlib_external")?;
        let native = self.lookup_attr(core, "_NativeImporter")?;
        let bundled = self.lookup_attr(core, "_BundledImporter")?;
        let path = self.lookup_attr(external, "PathFinder")?;
        let file_hook = self.lookup_attr(external, "_registered_path_hook")?;
        let mut hooks = Vec::new();
        if cfg!(unix) {
            let zip = self.module("zipimport")?;
            hooks.push(self.lookup_attr(zip, "zipimporter")?);
        }
        hooks.push(file_hook);
        let sys = self.module("sys")?;
        let meta = self.alloc(Value::List(vec![native, path, bundled]));
        let hooks = self.alloc(Value::List(hooks));
        let cache = self.dict(Vec::new());
        self.set_attr(sys, "meta_path", meta)?;
        self.set_attr(sys, "path_hooks", hooks)?;
        self.set_attr(sys, "path_importer_cache", cache)?;
        self.import_registry_installed = true;
        Ok(())
    }

    pub(super) fn meta_call(&mut self, globals: Id, method: &str, args: Vec<Id>) -> Result<Id> {
        let globals = if globals == NONE {
            self.bootstrap_globals
        } else {
            globals
        };
        let key = self.text(method);
        let func = self.get_item(globals, key)?;
        self.call(func, args, vec![])
    }

    pub(super) fn parent_import_path(&mut self, name: &str, parent: Id) -> Result<Id> {
        self.lookup_attr(parent, "__path__").map_err(|error| {
            if !self.is_attribute_error(&error) {
                return error;
            }
            Error::new(
                "ModuleNotFoundError",
                format!(
                    "No module named {}; {} is not a package",
                    string_repr(name),
                    string_repr(name.rsplit_once('.').unwrap().0)
                ),
            )
            .with_import(Some(name), None, None)
        })
    }

    fn bundled_registration(&self, name: &str) -> Option<&SourceModule> {
        let root = Path::new(self.config["bundled_root"].as_str()?);
        self.sources
            .get(name)?
            .iter()
            .find(|s| s.path.starts_with(root))
    }

    fn bundled_source(&self, name: &str) -> Option<(PathBuf, SourceBody)> {
        let source = self.bundled_registration(name)?;
        Some((source.path.clone(), SourceBody::Compiled(source.code)))
    }

    /// Return the source of this loader's registered initializer when it has a Python body.
    pub(super) fn native_source(&mut self, name: &str, bundled: bool) -> Result<Id> {
        let frozen = self.is_frozen(name);
        let sealed = frozen || stdlib::is_module(name);
        let registration = self.bundled_registration(name);
        if sealed == bundled || (!sealed && registration.is_none()) {
            return Err(Self::missing_module(name));
        }
        let text = registration.and_then(|source| source.bundled_text);
        Ok(if frozen {
            NONE
        } else {
            text.map_or(NONE, |bytes| {
                self.text(std::str::from_utf8(bytes).expect("compiler-emitted Python is UTF-8"))
            })
        })
    }

    fn native_locations(&self, name: &str, source: Option<&Path>) -> Result<Vec<PathBuf>> {
        if name == "collections" {
            return Ok(vec![stdlib::package_path(self, name)?]);
        }
        if let Some(path) = source
            && path.ends_with("__init__.py")
        {
            return Ok(vec![path.parent().unwrap().to_owned()]);
        }
        if matches!(name, "email" | "importlib" | "json" | "re" | "urllib") {
            return Ok(vec![PathBuf::from(format!("<imre stdlib>/{name}"))]);
        }
        Ok(Vec::new())
    }

    pub(super) fn native_spec_info(&mut self, name: &str, path: Id, bundled: bool) -> Result<Id> {
        let sealed = stdlib::is_module(name) || self.is_frozen(name);
        if sealed == bundled {
            return Ok(NONE);
        }
        let source = self.bundled_source(name);
        if source.is_none() && !stdlib::is_module(name) {
            return Ok(NONE);
        }
        if bundled && path != NONE {
            let Some((source, _)) = &source else {
                return Ok(NONE);
            };
            let directory = if source.ends_with("__init__.py") {
                source.parent().and_then(Path::parent)
            } else {
                source.parent()
            };
            if !self
                .items(path)?
                .iter()
                .any(|id| self.str_value(*id).ok().map(Path::new) == directory)
            {
                return Ok(NONE);
            }
        }
        let source = source.as_ref().map(|(p, _)| p.as_path());
        let locations = self.native_locations(name, source)?;
        let paths = if locations.is_empty() {
            NONE
        } else {
            let items = locations
                .iter()
                .map(|p| self.text(p.to_string_lossy()))
                .collect();
            self.alloc(Value::List(items))
        };
        let origin = if self.is_frozen(name) {
            self.text("frozen")
        } else if let Some(path) = source {
            self.text(path.to_string_lossy())
        } else {
            self.text("built-in")
        };
        Ok(self.alloc(Value::Tuple(vec![origin, paths])))
    }

    pub(super) fn create_native_module(&mut self, name: &str) -> Result<Id> {
        let source = self.bundled_source(name);
        if source.is_none() && !stdlib::is_module(name) {
            return Err(Self::missing_module(name));
        }
        let locations = self.native_locations(name, source.as_ref().map(|(p, _)| p.as_path()))?;
        Ok(self
            .allocate_import_module(name, &locations, source.as_ref())?
            .0)
    }

    pub(super) fn exec_native_module(&mut self, module: Id) -> Result<Id> {
        let Value::Module {
            name,
            scope,
            origin,
            ..
        } = self.heap.get(module)
        else {
            return Err(Error::new("TypeError", "expected a module"));
        };
        let (name, scope, origin) = (name.clone(), *scope, *origin);
        let name_id = self
            .scope_name(scope, "__name__")
            .unwrap_or_else(|| self.text(&name));
        let source = self
            .bundled_source(&name)
            .map(|(path, code)| (path, code, origin));
        if source.is_none() && !stdlib::is_module(&name) {
            return Err(Self::missing_module(&name));
        }
        self.run_initializer(&name, module, scope, name_id, NONE, source)?;
        Ok(NONE)
    }
}
