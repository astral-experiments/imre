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
    use ast::visitor::{Visitor, walk_stmt};
    struct Docs<'a> {
        replacements: &'a HashMap<TextRange, Vec<u32>>,
        invalid: Option<TextRange>,
    }
    impl Docs<'_> {
        fn check(&mut self, body: &[ast::Stmt]) {
            if let Some(doc) = crate::frontend::docstring_literal(body) {
                self.invalid = self.invalid.or_else(|| {
                    doc.value
                        .iter()
                        .map(Ranged::range)
                        .find(|r| self.replacements.contains_key(r))
                });
            }
        }
    }
    impl<'a> Visitor<'a> for Docs<'_> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::ClassDef(c) => self.check(&c.body),
                ast::Stmt::FunctionDef(f) => self.check(&f.body),
                _ => {}
            }
            if self.invalid.is_none() {
                walk_stmt(self, stmt);
            }
        }
        fn visit_expr(&mut self, _: &'a ast::Expr) {}
    }
    let mut docs = Docs {
        replacements,
        invalid: None,
    };
    docs.check(body);
    docs.visit_body(body);
    if let Some(range) = docs.invalid {
        return Err(crate::frontend::error(
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

    pub(super) fn format_literal(
        &mut self,
        literal: &ast::InterpolatedStringLiteralElement,
    ) -> String {
        let range = literal.range();
        // Escaped braces may join multiple middle tokens into one literal
        // element. Decode its replacement positions in original source order.
        if self
            .source
            .replacements
            .keys()
            .any(|&r| range.contains_range(r))
        {
            let replacements: Vec<_> =
                imre_ast::strings::replacements(&self.source.text[range]).collect();
            let mut bytes = Vec::new();
            Self::push_codepoints(&mut bytes, &literal.value, Some(&replacements));
            self.lit(7, &bytes)
        } else {
            self.text(&literal.value)
        }
    }

    pub(super) fn literal_text<'a>(
        &mut self,
        text: &str,
        parts: impl Iterator<Item = &'a ast::StringLiteral> + Clone,
    ) -> String {
        if !parts
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
        self.lit(7, &bytes)
    }
}
