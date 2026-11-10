//! Parsed physical module text and the source checks used before preparing registered code.
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use ruff_python_ast as ast;
use ruff_python_parser::Parsed;
use ruff_text_size::TextRange;

/// Text and syntax in the same coordinate system. Discovery normalizes physical newlines.
pub(crate) struct ParsedModule<'a> {
    pub(crate) text: Cow<'a, str>,
    pub(crate) parsed: Cow<'a, Parsed<ast::ModModule>>,
}

/// Only name lowering requires a mutable body. Keep the checked graph's tokens in place.
pub(crate) fn into_body(parsed: Cow<'_, Parsed<ast::ModModule>>) -> ast::Suite {
    match parsed {
        Cow::Owned(module) => module.into_syntax().body,
        Cow::Borrowed(module) => module.syntax().body.clone(),
    }
}

/// A physical file whose checked parser input still matches its executable source.
#[derive(Clone, Copy)]
pub(crate) struct CheckedFile(crate::modules::ModuleId);

impl CheckedFile {
    pub(crate) fn select(
        graph: &crate::modules::SourceGraph,
        id: crate::modules::ModuleId,
    ) -> Option<Self> {
        let module = &graph.modules[id];
        if module.path.extension()? != "py" || !module.parsed.has_no_syntax_errors() {
            return None;
        }
        // Inference may have replaced the parser input with an offset-preserving rewrite.
        let input = imre_ast::indentation::prepare(&module.source);
        graph
            .session
            .matches_parser_input(id, &input.text)
            .then_some(Self(id))
    }

    pub(crate) fn parsed(self, modules: &[crate::modules::SourceModule]) -> ParsedModule<'_> {
        let module = &modules[self.0];
        ParsedModule {
            text: Cow::Borrowed(&module.source),
            parsed: Cow::Borrowed(&module.parsed),
        }
    }

    pub(crate) fn entry(graph: &crate::modules::SourceGraph, path: &Path) -> Option<Self> {
        if graph.modules[graph.entry].path != std::path::absolute(path).ok()? {
            return None;
        }
        Self::select(graph, graph.entry)
    }
}

impl<'a> ParsedModule<'a> {
    /// Check the original file offsets. A discovered CRLF file can have normalized offsets.
    pub(crate) fn check_file(
        path: &Path,
        source: &'a str,
        discovered: Option<Self>,
        keep_docstrings: bool,
    ) -> Result<(), String> {
        let module = match discovered.filter(|entry| entry.text == source) {
            Some(entry) => entry,
            None => Self::parse(path, source.into())?,
        };
        validate(path, &module.text, &module.parsed, keep_docstrings)?;
        let mut body = into_body(module.parsed);
        lower(path, &module.text, &mut body)?;
        Ok(())
    }

    pub(crate) fn parse(path: &Path, text: Cow<'a, str>) -> Result<Self, String> {
        let parsed = imre_ast::indentation::prepare(&text)
            .parse_module()
            .map_err(|err| format!("{}: {err}", path.display()))?;
        Ok(Self {
            text,
            parsed: Cow::Owned(parsed),
        })
    }
}

/// Check syntax and prepare the literal escapes that this module can emit.
pub(crate) fn validate(
    path: &Path,
    source: &str,
    parsed: &Parsed<ast::ModModule>,
    keep_docstrings: bool,
) -> Result<HashMap<TextRange, Vec<u32>>, String> {
    if let Some((range, reason)) = imre_ast::comprehension::dict_unpacking_error(parsed) {
        let error = super::diagnostic_message(source, &range, &format!("SyntaxError: {reason}"));
        return Err(format!("{}:{error}", path.display()));
    }
    crate::analysis::check_scope_syntax(source, parsed.syntax())
        .map_err(|error| format!("{}:{error}", path.display()))?;
    if let Some(keyword) = imre_ast::repeated_keyword(parsed.syntax().into()) {
        let name = keyword.arg.as_ref().expect("repeated named argument");
        let reason = format!("SyntaxError: keyword argument repeated: {name}");
        let error = super::diagnostic_message(source, keyword, &reason);
        return Err(format!("{}:{error}", path.display()));
    }
    super::module_literal_replacements(source, parsed, keep_docstrings)
        .map_err(|error| format!("{}:{error}", path.display()))
}

pub(crate) fn lower(
    path: &Path,
    source: &str,
    body: &mut [ast::Stmt],
) -> Result<imre_ast::Names, String> {
    let private_names = imre_ast::lower(body);
    if source.contains("nonlocal") {
        super::function_scope::check_nonlocals(source, body)
            .map_err(|error| format!("{}:{error}", path.display()))?;
    }
    if let Some((range, reason)) = imre_ast::annotation_scope_error(body) {
        let error = super::diagnostic_message(source, &range, &format!("SyntaxError: {reason}"));
        return Err(format!("{}:{error}", path.display()));
    }
    if let Some(param) = &private_names.duplicate_type_parameter {
        let reason = format!("SyntaxError: duplicate type parameter '{}'", param.id);
        let error = super::diagnostic_message(source, param, &reason);
        return Err(format!("{}:{error}", path.display()));
    }
    Ok(private_names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::SourceGraph;

    #[test]
    fn discovery_borrows_only_the_original_entry_parse() {
        let path = Path::new("/input.py");
        let graph = SourceGraph::single("value = 1\r\n").unwrap();
        let entry = CheckedFile::entry(&graph, path)
            .unwrap()
            .parsed(&graph.modules);
        assert_eq!(entry.text, "value = 1\n");
        assert!(matches!(entry.parsed, Cow::Borrowed(_)));
        assert!(CheckedFile::entry(&graph, Path::new("/other.py")).is_none());

        // The checked namedtuple declaration replaces the separator escape with spaces.
        let graph = SourceGraph::single(
            "from collections import namedtuple\nPair = namedtuple('Pair', 'left\\x1cright')\n",
        )
        .unwrap();
        assert!(CheckedFile::entry(&graph, path).is_none());
    }
}
