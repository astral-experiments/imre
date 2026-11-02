use super::{Context, NONE, Result};

impl Context {
    pub(in crate::dynamic) fn encoding_warning(&mut self, level: i32) -> Result<()> {
        let (filename, line, module) = if let Some((loc, scope)) = self.heap.warning_frame(level) {
            let globals = self.module_globals(scope)?;
            let module = self
                .dict_named_item(globals, "__name__")
                .and_then(|id| self.str_value(id).ok())
                .unwrap_or("<string>")
                .to_owned();
            (loc.file, loc.line, module)
        } else {
            ("<sys>".into(), 0, "sys".into())
        };
        let message = "'encoding' argument not specified";
        if !crate::objects::warnings::should_emit(
            11,
            crate::objects::exceptions::Kind::EncodingWarning,
            message,
            &module,
            line as isize,
        )? {
            return Ok(());
        }
        let sys = self.module("sys")?;
        let stderr = self.lookup_attr(sys, "stderr")?;
        if stderr == NONE {
            return Ok(());
        }
        let mut formatted = format!("{filename}:{line}: EncodingWarning: {message}\n");
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
