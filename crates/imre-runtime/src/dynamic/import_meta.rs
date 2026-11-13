//! Connect the running interpreter's import registry to its native module initializers.
use std::path::{Path, PathBuf};

use crate::objects::{builtin_module_names, exceptions::string_repr};

use super::{
    Context, ModuleInitializer, SourceModule,
    importlib::SourceBody,
    stdlib,
    value::{Code, Error, Id, NONE, Result, Value},
};

impl Context {
    /// The bootstrap modules are initialized before user code can replace any import registry.
    pub(super) fn install_import_registry(&mut self) -> Result<()> {
        let core = self.module("_frozen_importlib")?;
        self.bootstrap_module = core;
        let external = self.module("_frozen_importlib_external")?;
        let builtin = self.lookup_attr(core, "BuiltinImporter")?;
        let frozen = self.lookup_attr(core, "FrozenImporter")?;
        let native = self.lookup_attr(core, "_NativeImporter")?;
        let bundled = self.lookup_attr(core, "_BundledImporter")?;
        let path = self.lookup_attr(external, "PathFinder")?;
        let registered_hook = self.lookup_attr(external, "_registered_path_hook")?;
        let file_hook = self.lookup_attr(external, "_file_path_hook")?;
        let mut hooks = Vec::new();
        if cfg!(unix) {
            let zip = self.module("zipimport")?;
            hooks.push(self.lookup_attr(zip, "zipimporter")?);
        }
        hooks.extend([registered_hook, file_hook]);
        let sys = self.module("sys")?;
        let meta = self.alloc(Value::List(vec![builtin, frozen, native, path, bundled]));
        let hooks = self.alloc(Value::List(hooks));
        let cache = self.dict(Vec::new());
        self.set_attr(sys, "meta_path", meta)?;
        self.set_attr(sys, "path_hooks", hooks)?;
        self.set_attr(sys, "path_importer_cache", cache)?;
        let init = self.lookup_attr(core, "_init_startup_specs")?;
        let names = stdlib::SYS_LAZY_FUNCTIONS
            .iter()
            .chain(stdlib::SYS_EAGER_FUNCTIONS)
            .chain(stdlib::SYS_BOUND_FUNCTIONS)
            .map(|&name| self.text(name))
            .collect();
        let names = self.alloc(Value::Tuple(names));
        self.call(init, vec![names], vec![])?;
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

    pub(super) fn parent_import_path(&mut self, name: &str, key: Id, parent: Id) -> Result<Id> {
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
            .with_import_name(key)
        })
    }

    pub(super) fn file_sources(&self, name: &str) -> impl Iterator<Item = &SourceModule> + Clone {
        self.sources
            .get(name)
            .into_iter()
            .flatten()
            .filter(|source| source.in_file_finder())
    }

    pub(super) fn builtin_initializer(&self, name: &str) -> Option<&SourceModule> {
        self.sources
            .get(name)?
            .iter()
            .find(|source| matches!(source.code, ModuleInitializer::Builtin(_)))
    }

    fn bundled_registration(&self, name: &str) -> Option<&SourceModule> {
        let root = Path::new(self.config["bundled_root"].as_str()?);
        self.file_sources(name)
            .find(|source| source.path.starts_with(root))
    }

    /// Mark the class defined by the bundled body, including when a file loader executes it.
    pub(super) fn register_decimal_class(&mut self, code: Id, entry: Code, scope: Id) {
        if let Some(source) = self.bundled_registration("decimal")
            && let ModuleInitializer::Native(registered) = source.code
            && std::ptr::fn_addr_eq(registered, entry)
            && matches!(self.heap.get(self.code_object(code).filename), Value::Str(path) if source.path == Path::new(path))
            && let Some(class) = self.scope_name(scope, "Decimal")
            && let Value::Class {
                scope: class_scope, ..
            } = *self.heap.get(class)
            && self.root_scope(class_scope) == scope
            && let Value::Scope(frame) = self.heap.get_mut(class_scope)
            && let Some(layout) = &mut frame.class_namespace
        {
            layout.bundled_decimal = true;
        }
    }

    pub(super) fn is_bundled_decimal(&self, class: Id) -> bool {
        matches!(self.heap.get(class), Value::Class { scope, .. }
            if self.scope_data(*scope).class_namespace.as_ref().is_some_and(|layout| layout.bundled_decimal))
    }

    pub(super) fn bundled_source(&self, name: &str) -> Option<(PathBuf, SourceBody)> {
        let source = self.bundled_registration(name)?;
        Some((source.path.clone(), SourceBody::Registered(source.code)))
    }

    fn frozen_registration<'a>(&self, name: &'a str) -> Option<(&SourceModule, &'a str)> {
        if !self.is_frozen(name) {
            return None;
        }
        let original = match name {
            "_frozen_importlib" => "importlib._bootstrap",
            "_frozen_importlib_external" => "importlib._bootstrap_external",
            _ => name,
        };
        Some((self.bundled_registration(name)?, original))
    }

    /// Supply the frozen finder's package flag and original module name without executing code.
    pub(super) fn frozen_info(&mut self, name: &str) -> Id {
        let Some((source, original)) = self.frozen_registration(name) else {
            return NONE;
        };
        let package = super::builtins::boolean(source.path.ends_with("__init__.py"));
        let original = self.text(original);
        self.alloc(Value::Tuple(vec![NONE, package, original]))
    }

    pub(super) fn frozen_code(&mut self, key: Id) -> Result<Id> {
        let name = self.str_value(key)?;
        let (source, original) = self.frozen_registration(name).ok_or_else(|| {
            Error::new(
                "ImportError",
                format!("No such frozen object named {}", string_repr(name)),
            )
            .with_import_name(key)
        })?;
        self.module_code(source.code, Path::new(&format!("<frozen {original}>")))
    }

    fn target_native_module(&self, name: &str) -> bool {
        stdlib::is_module(name) && !self.installed_module(name)
    }

    fn installed_module(&self, name: &str) -> bool {
        let members = self.config["stdlib_members"].as_str().unwrap_or_default();
        imre_ast::providers::module_member(name, members.lines()).is_some()
    }

    pub(super) fn sealed_native(&self, name: &str) -> bool {
        self.is_frozen(name)
            || (self.target_native_module(name)
                && !imre_ast::providers::is_native_package(name)
                && !imre_ast::providers::is_native_fallback(name))
    }

    /// Look up a module owned by this loader, including native modules without a Python body.
    fn native_registration(&self, key: Id, bundled: bool) -> Result<Option<&SourceModule>> {
        let name = self.str_value(key)?;
        let sealed = self.sealed_native(name);
        let registration = self.bundled_registration(name);
        if sealed == bundled
            || (!sealed && registration.is_none() && !self.target_native_module(name))
        {
            return Err(Self::missing_module(name, key));
        }
        Ok(registration)
    }

    /// Return the registered Python body for execution in the caller's module namespace.
    pub(super) fn native_code(&mut self, key: Id, bundled: bool) -> Result<Id> {
        let Some(source) = self.native_registration(key, bundled)? else {
            return Ok(NONE);
        };
        let (entry, path) = (source.code, source.path.clone());
        self.module_code(entry, &path)
    }

    /// Return the source of this loader's registered initializer when it has a Python body.
    pub(super) fn native_source(&mut self, key: Id, bundled: bool) -> Result<Id> {
        let name = self.str_value(key)?;
        let text = self
            .native_registration(key, bundled)?
            .or_else(|| self.builtin_initializer(name))
            .and_then(|source| source.bundled_text);
        Ok(if self.is_frozen(name) {
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
        if builtin_module_names::is_native_package(name) {
            return Ok(vec![PathBuf::from(format!("<imre stdlib>/{name}"))]);
        }
        Ok(Vec::new())
    }

    /// Populate the original sys dictionary once, when its inventory is first
    /// observed. Looking up optional providers can open a host library.
    pub(super) fn publish_builtin_module_names(&mut self, scope: Id) -> Result<()> {
        if self.pending_builtin_names_scope != Some(scope) {
            return Ok(());
        }
        if self.scope_name(scope, "builtin_module_names").is_none() {
            let names = builtin_module_names::names(|name| self.is_builtin_module(name))?;
            let items = names.into_iter().map(|name| self.text(name)).collect();
            let names = self.heap.packed_tuple(items, false);
            self.bind(scope, "builtin_module_names", names);
        }
        self.pending_builtin_names_scope = None;
        Ok(())
    }

    /// A native import can check the original inventory without discovering unrelated
    /// optional providers. An exposed or overridden binding uses Python membership.
    pub(super) fn builtin_names_contains(&mut self, sys: Id, key: Id) -> Result<bool> {
        if let Value::Module {
            scope,
            constructor: None,
            ..
        } = *self.heap.get(sys)
            && self.pending_builtin_names_scope == Some(scope)
            && self.scope_name(scope, "builtin_module_names").is_none()
            && let Value::Str(name) = self.heap.get(key)
        {
            return Ok(self.is_builtin_module(name));
        }
        let names = self.lookup_attr(sys, "builtin_module_names")?;
        self.contains(key, names)
    }

    /// The public built-in finder owns sealed native modules without Python source or a package path.
    pub(super) fn is_builtin_module(&self, name: &str) -> bool {
        builtin_module_names::owns(
            name,
            || self.is_frozen(name),
            || self.bundled_registration(name).is_some() || self.installed_module(name),
        )
    }

    pub(super) fn native_spec_info(&mut self, name: &str, path: Id, bundled: bool) -> Result<Id> {
        let sealed = self.sealed_native(name);
        if self.is_frozen(name) || sealed == bundled || (!bundled && self.is_builtin_module(name)) {
            return Ok(NONE);
        }
        let source = self.bundled_source(name);
        if source.is_none() && !self.target_native_module(name) {
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
        let origin = if let Some(path) = source {
            self.text(path.to_string_lossy())
        } else {
            self.text("built-in")
        };
        Ok(self.alloc(Value::Tuple(vec![origin, paths])))
    }

    /// Allocate a fresh built-in, respecting the two single-phase interpreter modules.
    pub(super) fn create_builtin_module(&mut self, name: &str) -> Result<Id> {
        if !self.is_builtin_module(name) {
            return Ok(NONE);
        }
        if matches!(name, "sys" | "builtins") {
            let name = self.text(name);
            return self.meta_call(NONE, "_create_core_module", vec![name]);
        }
        let module = self.create_native_module(name)?;
        let Value::Module {
            scope,
            module_state,
            ..
        } = self.heap.get_mut(module)
        else {
            unreachable!()
        };
        *module_state = Some(super::value::NativeModuleState::PendingBuiltin);
        let scope = *scope;
        // module_from_spec attaches the spec, loader and parent after creation.
        self.bind(scope, "__loader__", NONE);
        self.bind(scope, "__spec__", NONE);
        self.bind(scope, "__package__", NONE);
        Ok(module)
    }

    /// Run the initializer only for an object created by the built-in loader.
    pub(super) fn exec_builtin_module(&mut self, module: Id) -> Result<Id> {
        use super::value::NativeModuleState;
        let Value::Module { module_state, .. } = self.heap.get_mut(module) else {
            return Ok(NONE);
        };
        if !matches!(module_state, Some(NativeModuleState::PendingBuiltin)) {
            return Ok(NONE);
        }
        *module_state = None;
        if let Err(error) = self.exec_native_module(module) {
            if let Value::Module { module_state, .. } = self.heap.get_mut(module) {
                *module_state = Some(NativeModuleState::PendingBuiltin);
            }
            return Err(error);
        }
        Ok(NONE)
    }

    pub(super) fn create_native_module(&mut self, name: &str) -> Result<Id> {
        let source = self.bundled_source(name);
        if source.is_none() && !self.target_native_module(name) {
            return Err(Self::missing_module(name, self.text(name)));
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
        if source.is_none() && !self.target_native_module(&name) {
            return Err(Self::missing_module(&name, name_id));
        }
        self.run_initializer(&name, module, scope, name_id, NONE, source)?;
        Ok(NONE)
    }
}
