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

pub(super) fn interned_spelling(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

impl Function<'_, '_> {
    pub(super) fn template_strings(&mut self, template: &ast::TStringValue) -> String {
        let mut segments = Vec::new();
        let mut segment = Vec::new();
        for part in template.elements() {
            match part {
                ast::InterpolatedStringElement::Literal(lit) => {
                    let replacements = self.format_replacements(lit);
                    push_codepoints(&mut segment, &lit.value, replacements.as_deref());
                }
                ast::InterpolatedStringElement::Interpolation(_) => {
                    segments.push(std::mem::take(&mut segment));
                }
            }
        }
        segments.push(segment);
        self.emit_string_tuple(segments)
    }

    fn emit_string_tuple(&mut self, segments: Vec<Vec<u8>>) -> String {
        let slot = self.constant_slot(Constant::Tuple(
            segments.iter().cloned().map(Constant::Text).collect(),
        ));
        self.cached_tuple(&slot, |code| {
            let values = segments
                .into_iter()
                .map(|segment| code.linked_string(segment))
                .collect::<Vec<_>>();
            code.build_sequence(&values, true, Some(&slot))
        })
    }

    pub(super) fn source_text(&mut self, text: &str, points: Option<Vec<u8>>) -> String {
        let mut chars = text.chars();
        let short = chars
            .next()
            .is_none_or(|ch| ch <= '\u{ff}' && chars.next().is_none());
        if short || interned_spelling(text) {
            return self.text(text);
        }
        let points = points.unwrap_or_else(|| {
            let mut points = Vec::new();
            push_codepoints(&mut points, text, None);
            points
        });
        let data = self.output.data(text.as_bytes());
        self.cached_string(1, data, text.len(), points)
    }

    pub(super) fn linked_string(&mut self, points: Vec<u8>) -> String {
        let (chunks, remainder) = points.as_chunks::<4>();
        debug_assert!(remainder.is_empty());
        let text = chunks
            .iter()
            .map(|bytes| char::from_u32(u32::from_le_bytes(*bytes)))
            .collect::<Option<String>>();
        if let Some(text) = text {
            return self.source_text(&text, Some(points));
        }
        let data = self.output.data(&points);
        self.cached_string(7, data, points.len(), points)
    }

    fn cached_string(&mut self, kind: i32, data: String, len: usize, points: Vec<u8>) -> String {
        let slot = self.constant_slot(Constant::Text(points));
        self.take_matching_check();
        self.checked(&format!(
            "call i64 @imre_dyn_linked_string(ptr %ctx, ptr {slot}, i32 {kind}, ptr {data}, i64 {len})"
        ))
    }

    pub(super) fn format_literal(
        &mut self,
        literal: &ast::InterpolatedStringLiteralElement,
    ) -> String {
        if let Some(points) = self.format_replacements(literal) {
            let mut bytes = Vec::new();
            push_codepoints(&mut bytes, &literal.value, Some(&points));
            self.lit(7, &bytes)
        } else {
            self.text(&literal.value)
        }
    }

    fn format_replacements(
        &self,
        literal: &ast::InterpolatedStringLiteralElement,
    ) -> Option<Vec<u32>> {
        let range = literal.range();
        // Escaped braces may join multiple middle tokens into one literal
        // element. Decode its replacement positions in original source order.
        self.source
            .replacements
            .keys()
            .any(|&r| range.contains_range(r))
            .then(|| imre_ast::strings::replacements(&self.source.positions.text[range]).collect())
    }
}
