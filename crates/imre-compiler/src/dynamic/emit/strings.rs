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
    // Equal constants are merged across nested code objects in one compilation.
    fn string_constant_slot(&mut self, constant: StringConstant) -> String {
        let slots = &mut self.frame.output.string_constant_slots;
        let next = slots.len();
        let slot = *slots.entry((self.source.unit, constant)).or_insert(next);
        self.integer(slot)
    }

    pub(super) fn literal_string_tuple(&mut self, items: &[ast::Expr]) -> Option<String> {
        if !self.source.has_templates || items.is_empty() {
            return None;
        }
        let segments = items
            .iter()
            .map(|item| {
                let ast::Expr::StringLiteral(text) = item else {
                    return None;
                };
                let mut segment = Vec::new();
                for part in text.value.iter() {
                    let points = self.source.replacements.get(&part.range());
                    push_codepoints(&mut segment, part, points.map(Vec::as_slice));
                }
                Some(segment)
            })
            .collect::<Option<_>>()?;
        Some(self.emit_string_tuple(segments))
    }

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
        let list = self.op("list", &[]);
        for segment in &segments {
            let value = self.linked_string(segment.clone());
            self.op("append", &[&list, &value]);
        }
        let slot = self.string_constant_slot(StringConstant::Tuple(segments));
        self.op("linked_string_tuple", &[&list, &slot])
    }

    pub(super) fn linked_string(&mut self, points: Vec<u8>) -> String {
        let (chunks, remainder) = points.as_chunks::<4>();
        debug_assert!(remainder.is_empty());
        let text = chunks
            .iter()
            .map(|bytes| char::from_u32(u32::from_le_bytes(*bytes)))
            .collect::<Option<String>>();
        let value = if let Some(text) = text {
            self.text(&text)
        } else {
            self.lit(7, &points)
        };
        let slot = self.string_constant_slot(StringConstant::Text(points));
        self.op("linked_string", &[&value, &slot])
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
