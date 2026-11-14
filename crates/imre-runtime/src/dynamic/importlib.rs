//! Specs and executable loaders for sources registered with the native import system.
use std::path::{Path, PathBuf};

use crate::objects::exceptions::string_repr;

use super::{
    Context, ModuleInitializer, stdlib,
    value::{self, Error, FALSE, Id, NONE, Result, TRUE, Value},
};

/// A registered body, a newly discovered filesystem source, or its ZIP loader.
#[derive(Clone, Copy)]
pub(super) enum SourceBody {
    Registered(ModuleInitializer),
    Filesystem,
    Namespace,
    Zip(Id),
}

impl SourceBody {
    fn loader(self) -> Id {
        match self {
            Self::Registered(_) | Self::Filesystem | Self::Namespace => NONE,
            Self::Zip(loader) => loader,
        }
    }
}

extern "C" fn empty_module(_: *mut Context, _: Id) -> Id {
    NONE
}

impl Context {
    /// Call the caller's import hook for side effects, then read the interpreter's module registry.
    pub(super) fn import_with_builtins(&mut self, module_name: &str) -> Result<Id> {
        let mark = self.heap.temporary_mark();
        let builtins_key = self.heap.code_name("__builtins__");
        let globals = if self.heap.python_depth() == 0 {
            let builtins = self.module("builtins")?;
            self.dict(vec![(builtins_key, builtins)])
        } else {
            self.module_globals(self.heap.current_scope())?
        };
        self.heap.keep(globals);
        let builtins = self.get_item(globals, builtins_key)?;
        self.heap.keep(builtins);
        let hook = if self.dict_storage(builtins).is_some() {
            let key = self.heap.code_name("__import__");
            self.get_item(builtins, key)
                .map_err(|_| self.lookup_error("KeyError", key))?
        } else {
            self.lookup_attr(builtins, "__import__")?
        };
        self.heap.keep(hook);
        let name = self.heap.code_name(module_name);
        let fromlist = self.alloc(Value::List(Vec::new()));
        self.heap.keep(fromlist);
        let level = self.int(0);
        self.heap.keep(level);
        let call_mark = self.heap.temporary_mark();
        let returned = self.call(hook, vec![name, globals, globals, fromlist, level], vec![])?;
        // The hook's result can finalize and replace the registry entry before it is read.
        self.heap.keep(returned);
        self.discard_truth(call_mark);
        let module = self.get_item(self.modules, name)?;
        let collect = self.heap.discard_truth_operands(mark, self.modules);
        self.heap.keep(module);
        if collect {
            self.collect_unreachable();
        }
        Ok(module)
    }

    /// Return whether a launcher supplied the original argument vector.
    pub(super) fn startup_sys(&mut self) -> Result<bool> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let arguments =
            if self.config["module_entry"].is_string() || self.config["executable"].is_string() {
                crate::run_start::wait_for_cleanup(true).map_err(crate::objects::errno_error)?
            } else {
                None
            };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let arguments: Option<Vec<u8>> = None;
        let launched = arguments.is_some();
        let sys = self.module("sys")?;
        if let Some(bytes) = arguments {
            let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
            let mut original = Vec::new();
            for arg in bytes
                .strip_suffix(&[0])
                .unwrap_or(&bytes)
                .split(|byte| *byte == 0)
            {
                original.push(self.text_codepoints(codec.decode_filesystem(arg)?));
            }
            let original = self.alloc(Value::List(original));
            self.set_attr(sys, "orig_argv", original)?;
        }
        Ok(launched)
    }

    /// Enter the selected module in the startup __main__ namespace via the selected runpy.
    pub(super) fn run_module_as_main(&mut self, name: String) -> Result<Id> {
        let mark = self.heap.temporary_mark();
        let requested = self.text(&name);
        stdlib::audit::event(self, "cpython.run_module", &[requested])?;
        let runpy = self.module("runpy")?;
        let run = self.lookup_attr(runpy, "_run_module_as_main")?;
        // sys and runpy now own their initialization results. Keep only the callable
        // for this invocation before running arbitrary parent-package and target code.
        self.heap.release_temporaries_since(mark);
        self.heap.keep(run);
        let name = self.text(name);
        let alter_argv = if self.config["path_entry"].as_bool() == Some(true) {
            FALSE
        } else {
            TRUE
        };
        self.call(run, vec![name, alter_argv], Vec::new())
    }

    /// Infer the package from import globals, following the CPython fallback order.
    fn import_package(&mut self, globals: Option<Id>) -> Result<String> {
        use crate::objects::exceptions::Kind::{DeprecationWarning, ImportWarning};
        let missing_name = |ctx: &mut Self| {
            let key = ctx.text("'__name__' not in globals");
            ctx.lookup_error("KeyError", key)
        };
        let globals = globals.ok_or_else(|| missing_name(self))?;
        let globals = self
            .dict_storage(globals)
            .ok_or_else(|| Error::new("TypeError", "globals must be a dict"))?;
        let package = self
            .dict_named_item(globals, "__package__")
            .filter(|id| *id != NONE);
        let spec = self
            .dict_named_item(globals, "__spec__")
            .filter(|id| *id != NONE);
        Ok(if let Some(package) = package {
            let name = self
                .str_value(package)
                .map_err(|_| Error::new("TypeError", "package must be a string"))?
                .to_owned();
            if let Some(spec) = spec {
                let parent = self.lookup_attr(spec, "parent")?;
                let equal = self.binary("==", package, parent)?;
                if !self.truth(equal)? {
                    self.emit_builtin_warning(
                        DeprecationWarning,
                        "__package__ != __spec__.parent",
                        1,
                    )?;
                }
            }
            name
        } else if let Some(spec) = spec {
            let parent = self.lookup_attr(spec, "parent")?;
            self.str_value(parent)
                .map_err(|_| Error::new("TypeError", "__spec__.parent must be a string"))?
                .to_owned()
        } else {
            self.emit_builtin_warning(ImportWarning, "can't resolve package from __spec__ or __package__, falling back on __name__ and __path__", 1)?;
            let name = self
                .dict_named_item(globals, "__name__")
                .ok_or_else(|| missing_name(self))?;
            let name = self
                .str_value(name)
                .map_err(|_| Error::new("TypeError", "__name__ must be a string"))?;
            if self.dict_named_item(globals, "__path__").is_some() {
                name.to_owned()
            } else {
                name.rsplit_once('.')
                    .map_or("", |(parent, _)| parent)
                    .to_owned()
            }
        })
    }

    /// Resolve the package supplied by the caller of a relative builtin import.
    pub(super) fn relative_import(
        &mut self,
        name: &str,
        globals: Option<Id>,
        fromlist: Option<Id>,
        level: i32,
    ) -> Result<Id> {
        let parent = self.import_package(globals)?;
        if parent.is_empty() {
            return Err(Error::new(
                "ImportError",
                "attempted relative import with no known parent package",
            ));
        }
        let mut base = parent.as_str();
        for _ in 1..level {
            base = base
                .rsplit_once('.')
                .ok_or_else(|| {
                    Error::new(
                        "ImportError",
                        "attempted relative import beyond top-level package",
                    )
                })?
                .0;
        }
        let absolute = if name.is_empty() {
            base.to_owned()
        } else {
            format!("{base}.{name}")
        };
        let module = self.module(&absolute)?;
        self.heap.reserve_temporaries(1)?;
        self.heap.keep(module);
        if let Some(list) = fromlist
            && self.truth(list)?
        {
            self.prepare_fromlist(module, list)?;
        } else if let Some((first, _)) = name.split_once('.') {
            let head = format!("{base}.{first}");
            let key = self.text(&head);
            return self.registered_module(key)?.ok_or_else(|| {
                let key = self.text(format!("'{}' not in sys.modules as expected", head));
                self.lookup_error("KeyError", key)
            });
        }
        Ok(module)
    }

    /// A nonempty fromlist probes the live package path once before binding any names.
    pub(super) fn prepare_fromlist(&mut self, module: Id, list: Id) -> Result<Id> {
        match self.lookup_attr(module, "__path__") {
            Ok(_) if self.import_registry_installed => {
                let import = self.builtin("__import__");
                return self.meta_call(NONE, "_handle_fromlist", vec![module, list, import]);
            }
            Ok(_) => self.load_fromlist(module, list, false)?,
            Err(err) if self.is_attribute_error(&err) => {}
            Err(err) => return Err(err),
        }
        Ok(module)
    }

    /// Import missing children of a package requested by a nonempty fromlist.
    fn load_fromlist(&mut self, module: Id, list: Id, recursive: bool) -> Result<()> {
        let iterator = self.iterator(list)?;
        self.heap.reserve_temporaries(1)?;
        self.heap.keep(iterator);
        while let Some(item) = self.next_item(iterator)? {
            let name = match self.str_value(item) {
                Ok(name) => name.to_owned(),
                Err(_) => {
                    let owner = if recursive {
                        let id = self.lookup_attr(module, "__name__")?;
                        format!("{}.__all__", self.str_value(id)?)
                    } else {
                        "``from list''".to_owned()
                    };
                    return Err(Error::new(
                        "TypeError",
                        format!("Item in {owner} must be str, not {}", self.type_name(item)),
                    ));
                }
            };
            if name == "*" {
                if !recursive {
                    match self.lookup_attr(module, "__all__") {
                        Ok(_) => {
                            let names = self.lookup_attr(module, "__all__")?;
                            self.load_fromlist(module, names, true)?;
                        }
                        Err(err) if self.is_attribute_error(&err) => {}
                        Err(err) => return Err(err),
                    }
                }
                continue;
            }
            match self.lookup_named_attr(module, &name, Some(item)) {
                Ok(_) => continue,
                Err(err) if self.is_attribute_error(&err) => {}
                Err(err) => return Err(err),
            }
            let id = self.lookup_attr(module, "__name__")?;
            let child = format!("{}.{name}", self.str_value(id)?);
            // Missing children are ignored, but a missing dependency inside a child propagates.
            if let Err(error) = self.find_module(&child) {
                if !self.is_builtin_error(&error, "ModuleNotFoundError") {
                    return Err(error);
                }
                let exception = self.raised_instance(error);
                let ignore = self.with_import_exception(exception, |ctx| {
                    let name = ctx.lookup_attr(exception, "name")?;
                    let key = ctx.text(&child);
                    let equal = ctx.binary("==", name, key)?;
                    Ok(ctx.truth(equal)? && ctx.public_registered_module(key)? != Some(NONE))
                })?;
                if !ignore {
                    self.op("reraise", exception, NONE, NONE)?;
                }
            }
        }
        Ok(())
    }

    /// Bootstrap modules remain available even while ordinary frozen imports are disabled.
    pub(super) fn is_frozen(&self, name: &str) -> bool {
        imre_ast::providers::is_frozen(name, || match self.frozen_override {
            0 => self.config["frozen_modules"].as_bool().unwrap_or(true),
            override_value => override_value > 0,
        })
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
        if method == "extension_suffixes" {
            if !args.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "_imp.extension_suffixes() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            let suffixes = imre_ast::NATIVE_ARCHIVE_EXTENSIONS
                .iter()
                .map(|ext| self.text(format!(".{ext}")))
                .collect();
            return Ok(self.alloc(Value::List(suffixes)));
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
        let mut roots = Vec::new();
        for entry in search {
            if self.string_storage(entry).is_some() {
                roots.push(self.import_path(entry)?);
            }
        }
        for spelling in roots
            .into_iter()
            .chain(bundle.iter().filter(|_| top_level).cloned())
        {
            let spelling = self.registry_root(&spelling).to_owned();
            let root = if spelling.as_os_str().is_empty() {
                std::env::current_dir()
            } else if bundle.as_ref().is_some_and(|dir| spelling.starts_with(dir)) {
                Ok(spelling.clone())
            } else {
                std::path::absolute(&spelling)
            };
            let Ok(root) = root else { continue };
            let stem = root.join(child);
            let disk = !bundle.as_ref().is_some_and(|dir| root.starts_with(dir))
                && Path::new(child).file_name() == Some(std::ffi::OsStr::new(child));
            for path in [stem.join("__init__.py"), stem.with_extension("py")] {
                let entry = self.registered_file(name, &path).map(|source| source.code);
                if entry.is_some() || (disk && path.is_file()) {
                    locations.clear();
                    if path.ends_with("__init__.py") {
                        locations.push(stem);
                    }
                    return Ok(Some((
                        path,
                        entry.map_or(SourceBody::Filesystem, SourceBody::Registered),
                    )));
                }
            }
            let entry = self
                .file_sources(name)
                .find(|m| m.path == stem)
                .map(|m| m.code);
            if entry.is_some() || (disk && stem.is_dir()) {
                locations.push(stem.clone());
                namespace.get_or_insert((stem, SourceBody::Namespace));
                continue;
            }
            // A path below an archive is virtual. Preserve its spelling because the
            // loader exposes it in __file__, __path__, and resource names.
            if cfg!(unix)
                && disk
                && !spelling.as_os_str().is_empty()
                && spelling
                    .ancestors()
                    .find_map(|p| p.metadata().ok())
                    .is_some_and(|m| m.is_file())
                && let Some((origin, loader)) = self.zip_source(&spelling, name)?
            {
                if loader == NONE {
                    locations.push(origin.clone());
                    namespace.get_or_insert((origin, SourceBody::Namespace));
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
        let path = self.import_path_text(root)?;
        let name = self.text(name);
        let result = self.call(find, vec![path, name], vec![])?;
        if result == NONE {
            return Ok(None);
        }
        let parts = self.items(result)?;
        let [origin, loader] = parts.as_slice() else {
            return Err(Error::new("TypeError", "expected a ZIP origin and loader"));
        };
        Ok(Some((self.import_path(*origin)?, *loader)))
    }

    pub(super) fn module_code(&mut self, entry: ModuleInitializer, path: &Path) -> Result<Id> {
        let filename = self.import_path_text(path)?;
        self.module_code_at(entry, path, filename)
    }

    pub(super) fn module_code_at(
        &mut self,
        entry: ModuleInitializer,
        path: &Path,
        filename: Id,
    ) -> Result<Id> {
        let flags = match entry {
            ModuleInitializer::Zip(_, _, flags) => Some(flags),
            ModuleInitializer::File { flags, .. } => flags,
            _ => None,
        };
        let entry = match entry {
            ModuleInitializer::Native(entry)
            | ModuleInitializer::File { entry, .. }
            | ModuleInitializer::Zip(entry, _, _)
            | ModuleInitializer::Builtin(entry) => entry,
            ModuleInitializer::Invalid {
                message,
                position,
                in_args,
                parser_line,
                ..
            } => {
                if let Some(line) = parser_line {
                    let mut line = line.to_owned();
                    if !line.ends_with('\n') {
                        line.push('\n');
                    }
                    return Err(self.parser_syntax_error(
                        filename,
                        &path.to_string_lossy(),
                        &format!(": {message}"),
                        true,
                        position.expect("parser error position").map(i64::from),
                        line,
                    )?);
                }
                return Err(self.syntax_error_at(
                    message,
                    filename,
                    position.map_or([-1, -1, 0, 0], |span| span.map(i64::from)),
                    in_args,
                )?);
            }
        };
        Ok(self.native_module_code(entry, filename, flags))
    }

    /// Each execution of compiled source gets its own module code and nested definition cache.
    pub(super) fn native_module_code(
        &mut self,
        entry: value::Code,
        filename: Id,
        flags: Option<u32>,
    ) -> Id {
        let name_id = self.text("<module>");
        self.realize_code(
            value::Body::Native(entry, NONE),
            value::CodeDefinition {
                module_definition: true,
                flags: flags.unwrap_or(0),
                flags_complete: flags.is_some(),
                ..Default::default()
            },
            name_id,
            NONE,
            (name_id, NONE),
            (filename, 1),
        )
    }

    /// Resolve bytes read from a physical origin to its compiled module code, preserving
    /// the filename used by nested definitions and leaving namespace creation to exec().
    pub(super) fn file_code_from_bytes(
        &mut self,
        filename: Id,
        data: Id,
        inherited: u32,
        loader: bool,
    ) -> Result<Option<Id>> {
        use sha2::{Digest, Sha256};
        let data = self.scalar_value(data);
        if !matches!(self.heap.get(data), Value::Bytes(_) | Value::ByteArray(_)) {
            return Ok(None);
        }
        let path = match self.import_path(filename) {
            Ok(path) => path,
            // compile accepts diagnostic filenames that have no filesystem representation.
            Err(error) if !loader && error.kind == "UnicodeEncodeError" => return Ok(None),
            Err(error) => return Err(error),
        };
        // An empty source read by the loader has no operations tied to a build-time origin.
        if loader && self.bytes_like(data)?.is_empty() {
            return Ok(Some(self.native_module_code(
                empty_module,
                filename,
                Some(inherited),
            )));
        }
        let mut entries = self.sources.values().flatten().filter_map(|source| {
            if source.path.as_os_str() != path.as_os_str()
                && !matches!(source.code, ModuleInitializer::File { declared_open_path: Some(alias), .. } if alias.as_os_str() == path.as_os_str())
            {
                return None;
            }
            match source.code {
                code @ ModuleInitializer::File {
                    entry,
                    digest: Some(digest),
                    flags,
                    ..
                } => Some((code, Some(entry), digest, flags)),
                code @ ModuleInitializer::Zip(entry, digest, flags) => {
                    Some((code, Some(entry), digest, Some(flags)))
                }
                code @ ModuleInitializer::Invalid {
                    archive_digest: Some(digest),
                    ..
                } => Some((code, None, digest, None)),
                _ => None,
            }
        });
        let Some((code, entry, digest, flags)) = entries.next() else {
            return Ok(None);
        };
        // A source-declared future already applies to its initializer. Additional
        // inherited flags need different code. Only the loader owns legacy records
        // without flags and registrations that carry a syntax diagnostic.
        if inherited & !flags.unwrap_or(0) != 0 || (flags.is_none() && !loader) {
            return Ok(None);
        }
        if entries.any(|(_, other, hash, other_flags)| {
            hash != digest
                || other_flags != flags
                || match (entry, other) {
                    (Some(a), Some(b)) => !std::ptr::fn_addr_eq(a, b),
                    (None, None) => false,
                    _ => true,
                }
        }) {
            return Ok(None);
        }
        if Sha256::digest(self.bytes_like(data)?).as_slice() != digest {
            return Ok(None);
        }
        self.module_code_at(code, &path, filename).map(Some)
    }

    /// Resolve the file that runpy tried to open to the declaring loader’s native code.
    pub(super) fn declared_file_code(&mut self, path: Id, filename: Id) -> Result<Option<Id>> {
        let path = self.import_path(path)?;
        let mut selected = None;
        for (name, sources) in &self.sources {
            for source in sources {
                let ModuleInitializer::File {
                    entry,
                    digest,
                    flags,
                    declared_open_path: Some(open_path),
                } = source.code
                else {
                    continue;
                };
                if (open_path.as_os_str() != path.as_os_str()
                    && source.path.as_os_str() != path.as_os_str())
                    || self.sealed_native(name)
                {
                    continue;
                }
                if let Some((_, previous, hash, future)) = selected
                    && (!std::ptr::fn_addr_eq(previous, entry) || hash != digest || future != flags)
                {
                    return Ok(None);
                }
                selected = Some((source.code, entry, digest, flags));
            }
        }
        selected
            .map(|(code, ..)| self.module_code_at(code, &path, filename))
            .transpose()
    }

    fn registered_file(&self, name: &str, filename: &Path) -> Option<&super::SourceModule> {
        let source = self
            .file_sources(name)
            .find(|source| source.path == filename)?;
        // Standalone programs carry first-party source for relocation. While the actual
        // file is present, let SourceFileLoader read it, including after a failed import.
        // Bundled stdlib origins are virtual, and compiled initializers keep their code.
        if matches!(source.code, ModuleInitializer::Invalid { .. })
            && filename.is_absolute()
            && filename.is_file()
        {
            return None;
        }
        Some(source)
    }

    /// Bind the issued loader to its initializer, including modules compiled without source text.
    pub(super) fn bind_loader_source(&mut self, loader: Id, name: Id, origin: Id) -> Result<()> {
        let filename = self.import_path(origin)?;
        if let Some(source) = self.registered_file(self.str_value(name)?, &filename) {
            let source = super::SourceModule {
                path: filename,
                single_inputs: None,
                ..*source
            };
            self.heap
                .linked_sources
                .insert(loader, super::LinkedSource::Loader(source));
        }
        Ok(())
    }

    /// Attach finite inputs to the globals in which the selected initializer actually runs.
    pub(super) fn bind_doctest_module(&mut self, scope: Id, name: Id) -> Result<Id> {
        let code = self.code_object(self.heap.current_code());
        let value::Body::Native(active, _) = code.body else {
            return Ok(NONE);
        };
        let path = self.import_path(code.filename)?;
        let Some(sources) = self.sources.get(self.str_value(name)?) else {
            return Ok(NONE);
        };
        let selected = sources.iter().find(|source| {
            let (entry, alternate) = match source.code {
                ModuleInitializer::File {
                    entry,
                    declared_open_path,
                    ..
                } => (entry, declared_open_path),
                ModuleInitializer::Native(entry)
                | ModuleInitializer::Builtin(entry)
                | ModuleInitializer::Zip(entry, ..) => (entry, None),
                ModuleInitializer::Invalid { .. } => return false,
            };
            std::ptr::fn_addr_eq(active, entry)
                && source.single_inputs.is_some()
                && (source.path.as_os_str() == path.as_os_str()
                    || alternate.map(Path::as_os_str) == Some(path.as_os_str()))
        });
        let Some(source) = selected else {
            return Ok(NONE);
        };
        let source = super::SourceModule {
            path,
            single_inputs: source.single_inputs.clone(),
            ..*source
        };
        let globals = self.module_globals_storage(scope)?;
        self.heap
            .linked_sources
            .insert(globals, super::LinkedSource::Doctest(source));
        Ok(NONE)
    }

    pub(super) fn bind_doctest_source(&mut self, test: Id, module: Id) {
        if let Value::Module { scope, .. } = *self.heap.get(module)
            && let Some(globals) = self.scope_data(scope).namespace
            && let Some(super::LinkedSource::Doctest(source)) =
                self.heap.linked_sources.get(&globals)
        {
            let source = source.clone();
            self.heap
                .linked_sources
                .insert(test, super::LinkedSource::Doctest(source));
        }
    }

    fn loader_source(&mut self, loader: Id, filename: Id) -> Option<&super::SourceModule> {
        let path = self.import_path(filename).ok()?;
        let super::LinkedSource::Loader(source) = self.heap.linked_sources.get(&loader)? else {
            return None;
        };
        (source.path.as_os_str() == path.as_os_str()).then_some(source)
    }

    pub(super) fn registered_source_data(
        &mut self,
        loader: Id,
        filename: Id,
    ) -> Option<&'static [u8]> {
        self.loader_source(loader, filename)?.bundled_text
    }

    /// Return an issued loader's body or the matching initializer for a plain file loader.
    pub(super) fn registered_code(
        &mut self,
        loader: Id,
        path: Id,
        registration: Option<(Id, Id, Id)>,
    ) -> Result<Id> {
        if let Some((_, read, convert)) = registration {
            if !matches!(self.heap.get(path), Value::Str(_) | Value::SurrogateStr(_)) {
                return Ok(NONE);
            }
            let Value::Instance { class, .. } = *self.heap.get(loader) else {
                return Ok(NONE);
            };
            // Reading a user descriptor to test eligibility would invoke a loader hook twice.
            if self.class_attr(class, "__getattribute__").is_some_and(|hook| {
                !matches!(self.heap.get(hook), Value::Native(symbol) if symbol == "object.__getattribute__")
            }) || [("get_data", read), ("source_to_code", convert)].into_iter().any(|(attr, expected)| {
                self.class_attr(class, attr) != Some(expected) || self.raw_instance_field(loader, attr).is_some()
            }) {
                return Ok(NONE);
            }
        }
        if let Some(source) = self.loader_source(loader, path) {
            let (entry, origin) = (source.code, source.path.clone());
            return self.module_code(entry, &origin);
        }
        // A Python-constructed file loader has no issued-loader binding.
        if let Some((name, _, _)) = registration {
            let path = self.import_path(path)?;
            // Nested code retains the exact filename supplied at build time.
            if let Ok(name) = self.str_value(name)
                && let Some(source) = self.registered_file(name, &path)
                && source.path.as_os_str() == path.as_os_str()
                && let entry @ ModuleInitializer::File { .. } = source.code
            {
                return self.module_code(entry, &path);
            }
        }
        Ok(NONE)
    }

    fn file_spec(&mut self, name: Id, origin: Id, locations: Id, body: SourceBody) -> Result<Id> {
        let make = if self.bootstrap_file_spec != NONE
            && matches!(
                self.str_value(name)?,
                "importlib" | "importlib._bootstrap" | "importlib._bootstrap_external"
            ) {
            self.bootstrap_file_spec
        } else {
            let importlib = self.module("importlib")?;
            self.lookup_attr(importlib, "_file_spec")?
        };
        let spec = self.call(make, vec![name, origin, locations, body.loader()], vec![])?;
        if matches!(body, SourceBody::Registered(_))
            && let Some(loader) = self.raw_instance_field(spec, "loader")
        {
            self.bind_loader_source(loader, name, origin)?;
        }
        Ok(spec)
    }

    /// Execute the exact registered FILE as __main__ with its package's import context.
    pub(super) fn prepare_packaged_file(
        &mut self,
        name: Id,
        qualified: &str,
        path: &Path,
    ) -> Result<(Id, Id)> {
        let mark = self.heap.temporary_mark();
        if let Some((parent, _)) = qualified.rsplit_once('.') {
            self.module(parent)?;
        }
        let qualified = self.text(qualified);
        let spec = self.registered_source_spec(qualified, Some(path), None)?;
        let origin = self.lookup_attr(spec, "origin")?;
        let loader = self.lookup_attr(spec, "loader")?;
        let cached = self.lookup_attr(spec, "cached")?;
        let package = self.lookup_attr(spec, "parent")?;
        // Resolving the parent can replace __main__. Run the registered body in the
        // resulting namespace, including any builtins supplied by the hook.
        let modules = self.public_modules()?;
        let module = self.get_item(modules, name)?;
        self.heap.keep(module);
        let globals = self.lookup_attr(module, "__dict__")?;
        if self.dict_storage(globals).is_none() {
            let ty = self.type_name(globals);
            return Err(Error::new(
                "TypeError",
                format!("exec() globals must be a dict, not {ty}"),
            ));
        }
        let builtins = self.namespace_builtins(globals, None)?;
        let scope = self.function_environment("__main__", globals, builtins);
        self.bind(scope, "__name__", name);
        self.bind(scope, "__doc__", NONE);
        self.bind(scope, "__file__", origin);
        self.bind(scope, "__cached__", cached);
        self.bind(scope, "__loader__", loader);
        self.bind(scope, "__package__", package);
        self.bind(scope, "__spec__", spec);
        if let Value::Module { origin: file, .. } = self.heap.get_mut(module) {
            *file = Some(origin);
        }
        self.heap.release_temporaries_since(mark);
        self.heap.keep(module);
        self.heap.keep(scope);
        Ok((module, scope))
    }

    pub(super) fn location_list(&mut self, locations: &[PathBuf]) -> Result<Id> {
        let values = locations
            .iter()
            .map(|p| self.import_path_text(p))
            .collect::<Result<_>>()?;
        Ok(self.alloc(Value::List(values)))
    }

    /// Recheck the search roots supplied by _NamespacePath, ignoring already loaded modules.
    pub(super) fn namespace_locations(&mut self, name: &str, search: Id) -> Result<Id> {
        let child = name.rsplit('.').next().unwrap_or(name);
        let mut locations = Vec::new();
        let source = self.source_on_path(name, child, search, child == name, &mut locations)?;
        if matches!(source, Some((_, SourceBody::Namespace))) {
            self.location_list(&locations)
        } else {
            Ok(NONE)
        }
    }

    pub(super) fn namespace_spec(
        &mut self,
        name: Id,
        locations: Id,
        module: Id,
        source_globals: Id,
    ) -> Result<Id> {
        let importlib = self.module("importlib")?;
        let make = self.lookup_attr(importlib, "_namespace_spec")?;
        self.call(make, vec![name, locations, module, source_globals], vec![])
    }

    pub(super) fn initialize_source(
        &mut self,
        module: Id,
        scope: Id,
        name: Id,
        origin: Id,
        body: SourceBody,
    ) -> Result<Id> {
        let locations = self.scope_name(scope, "__path__").unwrap_or(NONE);
        let spec = self.file_spec(name, origin, locations, body)?;
        let loader = self.lookup_attr(spec, "loader")?;
        self.bind(scope, "__spec__", spec);
        self.bind(scope, "__loader__", loader);
        let exec = self.lookup_attr(loader, "exec_module")?;
        self.set_attr(spec, "_initializing", TRUE)?;
        let result = self.call(exec, vec![module], vec![]);
        self.set_attr(spec, "_initializing", FALSE)?;
        result
    }

    pub(super) fn cached_spec(
        &mut self,
        module: Id,
        key: Id,
        original: Id,
        for_resource: bool,
    ) -> Result<Id> {
        if module == NONE {
            return Ok(NONE);
        }
        let invalid_spec = |ctx: &mut Self, detail| -> Result<Error> {
            // The private resource caller already validated its package name.
            let name = if for_resource {
                original
            } else {
                super::builtins::format_value(ctx, original, None)?
            };
            Ok(Error::new(
                "ValueError",
                format!("{}.__spec__ {detail}", ctx.str_value(name)?),
            ))
        };
        match self.lookup_attr(module, "__spec__") {
            Ok(NONE) => Err(invalid_spec(self, "is None")?),
            Err(e)
                if self.is_attribute_error(&e)
                    && matches!(
                        self.heap.get(module),
                        Value::Module { constructor: None, origin, .. }
                            if origin.is_none() || self.str_value(key).is_ok_and(stdlib::is_module)
                    ) =>
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
                let caught = self.raised_instance(e);
                self.with_import_exception(caught, |ctx| {
                    let mut error = invalid_spec(ctx, "is not set")?;
                    error.tracing().cause = Some(NONE);
                    Err(error)
                })
            }
            spec => spec,
        }
    }

    pub(super) fn resource_spec(&mut self, key: Id) -> Result<Id> {
        let modules = self.public_modules()?;
        if self.contains(key, modules)? {
            let modules = self.public_modules()?;
            let module = self.get_item(modules, key)?;
            return self.cached_spec(module, key, key, true);
        }
        let fullname = self.str_value(key)?.to_owned();
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
                                string_repr(&fullname)
                            ),
                        )
                        .with_import_name(key);
                        let cause = self.raised_instance(cause);
                        error.tracing().cause = Some(cause);
                        error.context = cause;
                        return Err(error);
                    }
                    path => path?,
                };
                (child, path, false)
            } else {
                (fullname.as_str(), NONE, true)
            };
        if self.import_registry_installed {
            return self.meta_call(NONE, "_find_spec", vec![key, search]);
        }
        if stdlib::is_module(&fullname) || self.is_frozen(&fullname) {
            return Ok(NONE);
        }
        let search = if top_level {
            self.startup_search_path()?
        } else {
            search
        };
        let mut locations = Vec::new();
        let Some((path, body)) =
            self.source_on_path(&fullname, child, search, top_level, &mut locations)?
        else {
            return Ok(NONE);
        };
        if matches!(body, SourceBody::Namespace) {
            let paths = self.location_list(&locations)?;
            return self.namespace_spec(key, paths, NONE, NONE);
        }
        let origin = self.import_path_text(&path)?;
        let locations = if locations.is_empty() {
            NONE
        } else {
            self.location_list(&locations)?
        };
        self.file_spec(key, origin, locations, body)
    }
}
