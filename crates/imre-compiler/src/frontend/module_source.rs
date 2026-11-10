//! Parsed physical module text and the source checks used before preparing registered code.
use std::borrow::Cow;
use std::path::Path;

use ruff_python_ast as ast;
use ruff_python_parser::Parsed;

/// Text and syntax in the same coordinate system. Discovery normalizes physical newlines.
pub(crate) struct ParsedModule<'a> {
    pub(crate) text: Cow<'a, str>,
    pub(crate) parsed: Parsed<ast::ModModule>,
}

impl<'a> ParsedModule<'a> {
    pub(crate) fn parse(path: &Path, text: Cow<'a, str>) -> Result<Self, String> {
        let parsed = imre_ast::indentation::prepare(&text)
            .parse_module()
            .map_err(|err| format!("{}: {err}", path.display()))?;
        Ok(Self { text, parsed })
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
