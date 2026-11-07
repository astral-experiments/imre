use super::warnings_accel::WarningLocation;
use super::{Context, Id, NONE, Result};

enum Origin {
    FrozenImport(u32),
    Bootstrap {
        globals: Id,
        line: u32,
        frozen_external: bool,
    },
}

impl Origin {
    fn core(source_globals: Id, line: u32) -> Self {
        if source_globals == NONE {
            Self::FrozenImport(line)
        } else {
            Self::Bootstrap {
                globals: source_globals,
                line,
                frozen_external: false,
            }
        }
    }
}

impl Context {
    /// Report the warning in the bootstrap module performing this particular import.
    pub(in crate::dynamic) fn warn_child_binding(
        &mut self,
        source_globals: Id,
        message: &str,
    ) -> Result<()> {
        self.emit_warning(
            crate::objects::exceptions::Kind::ImportWarning,
            message,
            Origin::core(source_globals, 1353),
        )
    }

    pub(in crate::dynamic) fn warn_import(
        &mut self,
        globals: Id,
        message: &str,
        line: u32,
        external: Option<bool>,
    ) -> Result<()> {
        let origin = match external {
            Some(frozen_external) => Origin::Bootstrap {
                globals,
                line,
                frozen_external,
            },
            None => Origin::core(globals, line),
        };
        self.emit_warning(
            crate::objects::exceptions::Kind::ImportWarning,
            message,
            origin,
        )
    }

    pub(in crate::dynamic) fn warn_legacy_load(&mut self, source_globals: Id) -> Result<()> {
        self.emit_warning(
            crate::objects::exceptions::Kind::DeprecationWarning,
            "the load_module() method is deprecated and slated for removal in Python 3.15; use exec_module() instead",
            Origin::core(source_globals, 533),
        )
    }

    pub(in crate::dynamic) fn warn_bootstrap_loader(
        &mut self,
        globals: Id,
        line: u32,
        message: &str,
        frozen_external: bool,
    ) -> Result<()> {
        self.emit_warning(
            crate::objects::exceptions::Kind::DeprecationWarning,
            message,
            Origin::Bootstrap {
                globals,
                line,
                frozen_external,
            },
        )
    }

    pub(in crate::dynamic) fn encoding_warning(&mut self, level: i32) -> Result<()> {
        self.emit_builtin_warning(
            crate::objects::exceptions::Kind::EncodingWarning,
            "'encoding' argument not specified",
            level,
        )
    }

    /// Use the Python coroutine warning hook so filters, origin tracking and source capture apply.
    pub(in crate::dynamic) fn warn_unawaited_coroutine(&mut self, coroutine: Id) -> Result<()> {
        let import_error = self.builtin("ImportError");
        let hook = match self.module("warnings") {
            Ok(warnings) => self.optional_attr(warnings, "_warn_unawaited_coroutine"),
            Err(error) if self.raised_error_matches(&error, import_error) => Ok(None),
            Err(error) => Err(error),
        };
        let result = hook.and_then(|hook| {
            hook.map(|hook| self.call(hook, vec![coroutine], Vec::new()))
                .transpose()
        });
        match result {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(error) => {
                let category = self.builtin("RuntimeWarning");
                let warned = self.raised_error_matches(&error, category);
                super::super::unraisable::report(
                    self,
                    error,
                    true,
                    Some(coroutine),
                    "Exception ignored while finalizing coroutine",
                );
                if warned {
                    return Ok(());
                }
            }
        }
        let qualname = self.lookup_attr(coroutine, "__qualname__")?;
        let message = format!(
            "coroutine '{}' was never awaited",
            self.str_value(qualname)?
        );
        self.emit_builtin_warning_with_source(
            crate::objects::exceptions::Kind::RuntimeWarning,
            &message,
            1,
            coroutine,
        )
    }

    pub(in crate::dynamic) fn emit_builtin_warning(
        &mut self,
        kind: crate::objects::exceptions::Kind,
        message: &str,
        level: i32,
    ) -> Result<()> {
        self.emit_builtin_warning_with_source(kind, message, level, NONE)
    }

    /// Preserve the source object in a captured resource warning.
    pub(in crate::dynamic) fn emit_builtin_warning_with_source(
        &mut self,
        kind: crate::objects::exceptions::Kind,
        message: &str,
        level: i32,
        source: Id,
    ) -> Result<()> {
        let message = self.text(message);
        let category = self.builtin(kind.name());
        let level = self.int(i64::from(level));
        super::warnings_accel::warn::warn(self, &[message, category, level, source], &[])
            .map(|_| ())
    }

    pub(in crate::dynamic) fn emission_warning_registry(&mut self, globals: Id) -> Result<Id> {
        let key = self.heap.code_name("__warningregistry__");
        let storage = self
            .dict_storage(globals)
            .expect("module globals dictionary");
        let (_, index) = self.find_dictionary_key(storage, key, None)?;
        if let Some(index) = index {
            return Ok(self.stored_entries(storage)[index].1);
        }
        let registry = self.dict(Vec::new());
        self.set_dictionary_key(storage, key, registry, None, true)?;
        Ok(registry)
    }

    fn emit_warning(
        &mut self,
        kind: crate::objects::exceptions::Kind,
        message: &str,
        origin: Origin,
    ) -> Result<()> {
        let (filename, line, module, globals) = match origin {
            Origin::FrozenImport(line) => {
                // Some closed graphs run the native bootstrap without importing its module.
                if self.bootstrap_globals == NONE {
                    self.bootstrap_globals = self.dict(Vec::new());
                }
                (
                    "<frozen importlib._bootstrap>".into(),
                    line,
                    "importlib._bootstrap".to_owned(),
                    self.bootstrap_globals,
                )
            }
            Origin::Bootstrap {
                globals,
                line,
                frozen_external,
            } => {
                let module = self.dict_named_item(globals, "__name__");
                if module == Some(NONE) {
                    return Ok(());
                }
                let module = module
                    .and_then(|id| self.str_value(id).ok())
                    .unwrap_or("<string>")
                    .to_owned();
                let filename = if frozen_external {
                    "<frozen importlib._bootstrap_external>".into()
                } else {
                    // Sliced source keeps the code filename but uses the pinned warning call site.
                    self.heap
                        .current_location()
                        .map(|loc| loc.file)
                        .unwrap_or("<sys>".into())
                };
                (filename, line, module, globals)
            }
        };
        let registry = self.emission_warning_registry(globals)?;
        let message = self.text(message);
        let category = self.builtin(kind.name());
        let filename = self.text(filename.as_ref());
        let module = self.text(&module);
        super::warnings_accel::explicit::emit(
            self,
            message,
            category,
            WarningLocation {
                filename,
                lineno: line as i32,
                line: None,
            },
            Some(module),
            registry,
            NONE,
        )
        .map(|_| ())
    }
}
