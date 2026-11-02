use super::{Context, Error, Id, NONE, Result, Value};

enum Origin {
    Caller(i32),
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
            NONE,
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
            NONE,
        )
    }

    pub(in crate::dynamic) fn warn_legacy_load(&mut self, source_globals: Id) -> Result<()> {
        self.emit_warning(
            crate::objects::exceptions::Kind::DeprecationWarning,
            "the load_module() method is deprecated and slated for removal in Python 3.15; use exec_module() instead",
            Origin::core(source_globals, 533),
            NONE,
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
            NONE,
        )
    }

    pub(in crate::dynamic) fn encoding_warning(&mut self, level: i32) -> Result<()> {
        self.emit_builtin_warning(
            crate::objects::exceptions::Kind::EncodingWarning,
            "'encoding' argument not specified",
            level,
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
        self.emit_warning(kind, message, Origin::Caller(level), source)
    }

    fn emit_warning(
        &mut self,
        kind: crate::objects::exceptions::Kind,
        message: &str,
        origin: Origin,
        source: Id,
    ) -> Result<()> {
        let category = kind.name();
        let (filename, line, module) = match origin {
            Origin::FrozenImport(line) => (
                "<frozen importlib._bootstrap>".into(),
                line,
                "importlib._bootstrap".to_owned(),
            ),
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
                (filename, line, module)
            }
            Origin::Caller(level) => {
                if let Some((loc, scope)) = self.heap.warning_frame(level) {
                    let globals = self.module_globals(scope)?;
                    let module = self.dict_named_item(globals, "__name__");
                    // CPython suppresses warnings from a module being torn down, before filtering.
                    if module == Some(NONE) {
                        return Ok(());
                    }
                    let module = module
                        .and_then(|id| self.str_value(id).ok())
                        .unwrap_or("<string>")
                        .to_owned();
                    (loc.file, loc.line, module)
                } else {
                    ("<sys>".into(), 0, "sys".into())
                }
            }
        };
        // The bundled Python warnings implementation owns filters installed through
        // its public API, including context-local filters from catch_warnings.
        let key = self.heap.code_name("_py_warnings");
        if let Some(warnings) = self.registered_module(key)?
            && let Value::Module { scope, .. } = *self.heap.get(warnings)
        {
            let globals = self.module_globals(scope)?;
            if self
                .dict_named_item(globals, "_wm")
                .is_some_and(|wm| wm != NONE)
                && let Some(warn) = self.dict_named_item(
                    globals,
                    match origin {
                        Origin::Caller(_) => "warn",
                        Origin::FrozenImport(_) | Origin::Bootstrap { .. } => "warn_explicit",
                    },
                )
            {
                let message = self.text(message);
                let native_import = if category == "ImportWarning" {
                    self.dict_named_item(globals, "_native_import_warning")
                } else {
                    None
                };
                let category = self.builtin(category);
                let args = match origin {
                    Origin::Caller(level) => {
                        let level = self.int(i64::from(level));
                        let mut args = vec![message, category, level];
                        if source != NONE {
                            args.push(source);
                        }
                        args
                    }
                    Origin::FrozenImport(_) | Origin::Bootstrap { .. } => {
                        let globals = if let Origin::Bootstrap { globals, .. } = origin {
                            globals
                        } else {
                            // Some closed graphs run the native bootstrap without importing its module.
                            if self.bootstrap_globals == NONE {
                                self.bootstrap_globals = self.dict(Vec::new());
                            }
                            self.bootstrap_globals
                        };
                        let registry = match self.dict_named_item(globals, "__warningregistry__") {
                            Some(registry) => registry,
                            None => {
                                let registry = self.dict(Vec::new());
                                let key = self.text("__warningregistry__");
                                self.put_item(globals, key, registry)?;
                                registry
                            }
                        };
                        vec![
                            message,
                            category,
                            self.text(filename.as_ref()),
                            self.int(i64::from(line)),
                            self.text(&module),
                            registry,
                            native_import.unwrap_or(globals),
                        ]
                    }
                };
                return self.call(warn, args, Vec::new()).map(|_| ());
            }
        }
        if !crate::objects::warnings::should_emit(
            crate::objects::warnings::category_number(category).expect("warning category"),
            kind,
            message,
            &module,
            line as isize,
        )? {
            return Ok(());
        }
        let warnings_key = self.heap.code_name("warnings");
        if let Some(warnings) = self.registered_module(warnings_key)? {
            match self.lookup_attr(warnings, "showwarning") {
                Ok(hook) => {
                    if !self.callable(hook) {
                        return Err(Error::new(
                            "TypeError",
                            "warnings.showwarning() must be set to a function or method",
                        ));
                    }
                    let category = self.builtin(category);
                    let text = self.text(message);
                    let warning = self.call(category, vec![text], Vec::new())?;
                    let file = self.text(filename.as_ref());
                    let lineno = self.int(i64::from(line));
                    let arguments = vec![warning, category, file, lineno, NONE, NONE];
                    return self.call(hook, arguments, Vec::new()).map(|_| ());
                }
                Err(error) if self.is_attribute_error(&error) => {}
                Err(error) => return Err(error),
            }
        }
        let sys = self.module("sys")?;
        let stderr = self.lookup_attr(sys, "stderr")?;
        if stderr == NONE {
            return Ok(());
        }
        let mut formatted = format!("{filename}:{line}: {category}: {message}\n");
        if let Some(index) = line.checked_sub(1)
            && !filename.starts_with('<')
            && let Ok(bytes) = std::fs::read(&*filename)
            && let Ok(source) = crate::objects::decode_compile_source(&bytes)
            && let Some(text) = source.lines().nth(index as usize)
        {
            formatted.push_str("  ");
            formatted.push_str(text.trim_matches(crate::objects::python_whitespace));
            formatted.push('\n');
        }
        let write = self.lookup_attr(stderr, "write")?;
        let text = self.text(formatted);
        match self.call(write, vec![text], Vec::new()) {
            Err(error)
                if super::builtins::exception_subclass(&error.kind, "OSError") == Some(true) =>
            {
                Ok(())
            }
            result => result.map(|_| ()),
        }
    }
}
