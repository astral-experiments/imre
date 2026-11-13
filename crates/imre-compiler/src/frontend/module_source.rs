//! Parsed physical module text and the source checks used before preparing registered code.
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use ruff_python_ast as ast;
use ruff_python_parser::Parsed;
use ruff_text_size::{TextRange, TextSize};

/// Text and syntax in the same coordinate system. Discovery normalizes physical newlines.
pub(crate) struct ParsedModule<'text, 'syntax> {
    pub(crate) text: Cow<'text, str>,
    pub(crate) parsed: Cow<'syntax, Parsed<ast::ModModule>>,
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

    pub(crate) fn module(
        self,
        modules: &[crate::modules::SourceModule],
    ) -> &crate::modules::SourceModule {
        &modules[self.0]
    }

    pub(crate) fn parsed(self, modules: &[crate::modules::SourceModule]) -> ParsedModule<'_, '_> {
        let module = self.module(modules);
        ParsedModule {
            text: Cow::Borrowed(&module.source),
            parsed: Cow::Borrowed(&module.parsed),
        }
    }

    pub(crate) fn entry(graph: &crate::modules::SourceGraph, identity: &Path) -> Option<Self> {
        if graph.modules[graph.entry].path != identity {
            return None;
        }
        Self::select(graph, graph.entry)
    }
}

impl<'a> ParsedModule<'a, '_> {
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

/// The source bytes and line starts used to report locations in a compilation unit.
pub(crate) struct SourceFile<'a> {
    path: &'a Path,
    pub(crate) text: &'a str,
    newlines: Vec<usize>,
}

impl<'a> SourceFile<'a> {
    pub(crate) fn new(path: &'a Path, text: &'a str) -> Self {
        Self {
            path,
            text,
            newlines: text.match_indices('\n').map(|(offset, _)| offset).collect(),
        }
    }

    pub(crate) fn line(&self, offset: TextSize) -> usize {
        self.newlines.partition_point(|&n| n < usize::from(offset)) + 1
    }

    pub(crate) fn line_and_column(&self, offset: TextSize) -> (usize, usize) {
        let line = self.line(offset);
        let start = if line == 1 {
            0
        } else {
            self.newlines[line - 2] + 1
        };
        (line, usize::from(offset) - start)
    }

    /// Registered operands use one-based byte columns for both ends of a diagnostic.
    pub(crate) fn diagnostic_at(&self, range: TextRange, reason: &str) -> String {
        let (first_line, first_column) = self.line_and_column(range.start());
        let (last_line, last_column) = self.line_and_column(range.end());
        format!(
            "{}:{first_line}:{}: {reason} (through {last_line}:{})",
            self.path.display(),
            first_column + 1,
            last_column + 1
        )
    }
}

/// Report a failure before a code-generation frame needs a line index.
pub(crate) fn operand_diagnostic(
    path: &Path,
    source: &str,
    range: TextRange,
    reason: &str,
) -> String {
    SourceFile::new(path, source).diagnostic_at(range, reason)
}

fn validate_syntax(
    path: &Path,
    source: &str,
    parsed: &Parsed<ast::ModModule>,
    checked: Option<&crate::modules::SourceModule>,
) -> Result<(), String> {
    if let Some((range, reason)) = imre_ast::comprehension::dict_unpacking_error(parsed) {
        let error = super::diagnostic_message(source, &range, &format!("SyntaxError: {reason}"));
        return Err(format!("{}:{error}", path.display()));
    }
    if checked.is_none_or(|module| !std::ptr::eq(&*module.parsed, parsed)) {
        crate::analysis::check_scope_syntax(source, parsed.syntax())
            .map_err(|error| format!("{}:{error}", path.display()))?;
    }
    if let Some(keyword) = imre_ast::repeated_keyword(parsed.syntax().into()) {
        let name = keyword.arg.as_ref().expect("repeated named argument");
        let reason = format!("SyntaxError: keyword argument repeated: {name}");
        let error = super::diagnostic_message(source, keyword, &reason);
        return Err(format!("{}:{error}", path.display()));
    }
    Ok(())
}

/// During discovery only a docstring can require a surrogate diagnostic.
pub(crate) fn validate_file(
    path: &Path,
    source: &str,
    parsed: &Parsed<ast::ModModule>,
    keep_docstrings: bool,
) -> Result<(), String> {
    validate_syntax(path, source, parsed, None)?;
    super::validate_docstring_literals(source, parsed, keep_docstrings, None)
        .map_err(|error| format!("{}:{error}", path.display()))
}

/// Check syntax and prepare the literal escapes that this module can emit.
pub(crate) fn validate<'a>(
    path: &Path,
    source: &str,
    parsed: &Parsed<ast::ModModule>,
    checked: Option<&'a crate::modules::SourceModule>,
    keep_docstrings: bool,
) -> Result<Cow<'a, HashMap<TextRange, Vec<u32>>>, String> {
    validate_syntax(path, source, parsed, checked)?;
    let replacements = checked
        .and_then(|module| module.cached_registered_values())
        .map_or_else(
            || Cow::Owned(super::literal_replacements(source, parsed.tokens())),
            |values| Cow::Borrowed(&values.replacements),
        );
    super::validate_docstring_literals(source, parsed, keep_docstrings, Some(&replacements))
        .map_err(|error| format!("{}:{error}", path.display()))?;
    Ok(replacements)
}

pub(crate) fn validate_names(
    path: &Path,
    source: &str,
    body: &[ast::Stmt],
    private_names: &imre_ast::Names,
) -> Result<(), String> {
    if private_names.nonlocal_in_generic_class {
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::SourceGraph;

    #[test]
    fn checked_literals_borrow_the_existing_map_and_still_validate_docstrings() {
        let graph = SourceGraph::single("def f():\r\n    '\\ud800'\r\n").unwrap();
        let module = &graph.modules[graph.entry];
        let validate =
            |checked, keep| validate(&module.path, &module.source, &module.parsed, checked, keep);
        let independent = validate(None, false).unwrap();
        assert!(matches!(independent, Cow::Owned(_)));
        let map = &module.registered_values().replacements;
        assert_eq!(&*independent, map);
        assert!(matches!(
            validate(Some(module), false).unwrap(),
            Cow::Borrowed(borrowed) if std::ptr::eq(borrowed, map)
        ));
        let error = validate(None, true).unwrap_err();
        assert!(
            error.ends_with(":2:5: Imre: surrogate string escapes are unsupported in docstrings")
        );
        assert_eq!(validate(Some(module), true).unwrap_err(), error);
        let layout = ParsedModule::parse(&module.path, module.source.as_str().into()).unwrap();
        let borrowed = super::validate(
            &module.path,
            &layout.text,
            &layout.parsed,
            Some(module),
            false,
        )
        .unwrap();
        assert!(matches!(borrowed, Cow::Borrowed(borrowed) if std::ptr::eq(borrowed, map)));
    }

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
