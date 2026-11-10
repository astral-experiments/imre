//! Recover Python code points which Ruff cannot represent in a Rust string.
use super::*;
use ruff_text_size::TextRange;

pub(super) use crate::frontend::literal_replacements;
use crate::frontend::push_codepoints;

/// CPython cannot normalize a docstring containing a surrogate. Report the
/// offending literal before emitting code; -OO omits the docstring entirely.
pub(super) fn check_docs(
    source: &str,
    body: &[ast::Stmt],
    replacements: &HashMap<TextRange, Vec<u32>>,
) -> Result<(), String> {
    if let Some(range) = imre_ast::strings::find_docstring(body, true, |doc| {
        doc.value
            .iter()
            .map(Ranged::range)
            .find(|range| replacements.contains_key(range))
    }) {
        return Err(crate::frontend::diagnostic_message(
            source,
            &range,
            "surrogate string escapes are unsupported in docstrings",
        ));
    }
    Ok(())
}

impl Function<'_, '_> {
    pub(super) fn format_literal(
        &mut self,
        literal: &ast::InterpolatedStringLiteralElement,
    ) -> String {
        if let Some(points) = self.source.registered_source().format_replacements(literal) {
            let mut bytes = Vec::new();
            push_codepoints(&mut bytes, &literal.value, Some(&points));
            self.lit(7, &bytes)
        } else {
            self.text(&literal.value)
        }
    }
}
