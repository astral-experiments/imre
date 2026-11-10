//! Parsed physical module text and the source checks used before preparing registered code.
use std::borrow::Cow;
use std::path::Path;

use ruff_python_ast as ast;
use ruff_python_parser::Parsed;

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

impl<'a> ParsedModule<'a> {
    /// The physical entry snapshot has already passed file and indentation checks in
    /// Session::add. Inference can subsequently replace its syntax with a rewritten view.
    pub(crate) fn checked_entry(
        graph: &'a crate::modules::SourceGraph,
        path: &Path,
    ) -> Option<Self> {
        let module = &graph.modules[graph.entry];
        if module.path != std::path::absolute(path).ok()?
            || module.path.extension()? != "py"
            || !module.parsed.has_no_syntax_errors()
        {
            return None;
        }
        let input = imre_ast::indentation::prepare(&module.source);
        graph
            .session
            .matches_parser_input(graph.entry, &input.text)
            .then(|| Self {
                text: Cow::Borrowed(&module.source),
                parsed: Cow::Borrowed(&module.parsed),
            })
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

pub(crate) fn validate_syntax(
    path: &Path,
    source: &str,
    parsed: &Parsed<ast::ModModule>,
) -> Result<(), String> {
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
    Ok(())
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
        let entry = ParsedModule::checked_entry(&graph, path).unwrap();
        assert_eq!(entry.text, "value = 1\n");
        assert!(matches!(entry.parsed, Cow::Borrowed(_)));
        assert!(ParsedModule::checked_entry(&graph, Path::new("/other.py")).is_none());

        // The checked namedtuple declaration replaces the separator escape with spaces.
        let graph = SourceGraph::single(
            "from collections import namedtuple\nPair = namedtuple('Pair', 'left\\x1cright')\n",
        )
        .unwrap();
        assert!(ParsedModule::checked_entry(&graph, path).is_none());
    }
}
