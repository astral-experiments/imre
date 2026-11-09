//! Recover Python code points which Ruff cannot represent in a Rust string.
use super::*;
use ruff_text_size::TextRange;

/// Preserve Ruff's decoded strings except for the positions where Rust's
/// Unicode scalar type loses information. Raw literals and bytes are exact.
pub(super) fn literal_replacements(
    source: &str,
    tokens: &ast::token::Tokens,
) -> HashMap<TextRange, Vec<u32>> {
    let mut replacements = HashMap::new();
    for token in tokens {
        let Some(range) = imre_ast::strings::cooked_content(token) else {
            continue;
        };
        let points: Vec<_> = imre_ast::strings::replacements(&source[range]).collect();
        if points.iter().any(|&point| point != 0xfffd) {
            replacements.insert(token.range(), points);
        }
    }
    replacements
}

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
    fn push_codepoints(bytes: &mut Vec<u8>, text: &str, replacements: Option<&[u32]>) {
        let mut points = replacements.into_iter().flatten();
        for c in text.chars() {
            let point = if c == char::REPLACEMENT_CHARACTER && replacements.is_some() {
                *points
                    .next()
                    .expect("one original code point per replacement")
            } else {
                u32::from(c)
            };
            bytes.extend_from_slice(&point.to_le_bytes());
        }
        debug_assert!(points.next().is_none());
    }

    // Equal constants are merged across nested code objects in one compilation.
    fn string_constant_slot(&mut self, constant: StringConstant) -> String {
        let slots = &mut self.output.string_constant_slots;
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
                    Self::push_codepoints(&mut segment, part, points.map(Vec::as_slice));
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
                    Self::push_codepoints(&mut segment, &lit.value, replacements.as_deref());
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

    fn linked_string(&mut self, points: Vec<u8>) -> String {
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
            Self::push_codepoints(&mut bytes, &literal.value, Some(&points));
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
            .then(|| imre_ast::strings::replacements(&self.source.text[range]).collect())
    }

    pub(super) fn literal_text<'a>(
        &mut self,
        text: &str,
        parts: impl Iterator<Item = &'a ast::StringLiteral> + Clone,
    ) -> String {
        if !self.source.has_templates
            && !parts
                .clone()
                .any(|part| self.source.replacements.contains_key(&part.range()))
        {
            return self.text(text);
        }
        let mut bytes = Vec::new();
        for part in parts {
            let replacements = self.source.replacements.get(&part.range());
            Self::push_codepoints(&mut bytes, part, replacements.map(Vec::as_slice));
        }
        if self.source.has_templates {
            self.linked_string(bytes)
        } else {
            self.lit(7, &bytes)
        }
    }
}
