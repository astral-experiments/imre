use super::{Context, Error, NONE, Result};

impl Context {
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
        let category = kind.name();
        let (filename, line, module) = if let Some((loc, scope)) = self.heap.warning_frame(level) {
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
        };
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
