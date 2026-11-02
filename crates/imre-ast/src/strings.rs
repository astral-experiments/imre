//! Recover string spellings and code points that the decoded AST does not retain.
use ruff_python_ast::{
    self as ast, StringFlags,
    token::{Token, TokenKind, Tokens},
};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::collections::HashMap;

/// Raw and byte literals have no cooked Unicode escapes.
pub fn cooked_content(token: &Token) -> Option<TextRange> {
    if !matches!(
        token.kind(),
        TokenKind::String | TokenKind::FStringMiddle | TokenKind::TStringMiddle
    ) {
        return None;
    }
    let flags = token.unwrap_string_flags();
    if flags.is_raw_string() || flags.is_byte_string() {
        return None;
    }
    Some(if token.kind() == TokenKind::String {
        TextRange::new(
            token.start() + flags.opener_len(),
            token.end() - flags.closer_len(),
        )
    } else {
        token.range()
    })
}

/// Original code points rendered as U+FFFD in Ruff's decoded, non-raw string.
/// Non-Unicode escapes (including escaped backslashes) cannot produce a
/// surrogate or U+FFFD. Unknown escapes preserve the escaped character.
pub fn replacements(source: &str) -> impl Iterator<Item = u32> + '_ {
    let mut chars = source.chars();
    std::iter::from_fn(move || {
        loop {
            let mut point = u32::from(chars.next()?);
            if point == u32::from('\\') {
                point = match chars.next()? {
                    introducer @ ('u' | 'U') => {
                        let width = if introducer == 'u' { 4 } else { 8 };
                        let rest = chars.as_str();
                        let hex = rest.get(..width)?;
                        chars = rest[width..].chars();
                        u32::from_str_radix(hex, 16).ok()?
                    }
                    'N' => {
                        let (name, rest) = chars.as_str().strip_prefix('{')?.split_once('}')?;
                        chars = rest.chars();
                        u32::from(unicode_names2::character(name)?)
                    }
                    other => u32::from(other),
                };
            }
            if point == 0xfffd || (0xd800..=0xdfff).contains(&point) {
                return Some(point);
            }
        }
    })
}

/// CPython keeps the original expression spelling, including leading whitespace
/// and enclosing parentheses. Commented expressions require a separate tokenizer
/// transformation and are left unsupported.
pub fn template_expressions(
    source: &str,
    tokens: &Tokens,
    body: &[ast::Stmt],
) -> HashMap<TextSize, String> {
    use ast::visitor::{self, Visitor};
    struct Fields<'a> {
        source: &'a str,
        tokens: &'a Tokens,
        expressions: HashMap<TextSize, String>,
    }
    impl<'a> Visitor<'a> for Fields<'_> {
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                visitor::walk_expr(self, expr)
            });
        }

        fn visit_t_string(&mut self, template: &'a ast::TString) {
            for field in template.elements.interpolations() {
                // Ruff's expression range omits redundant outer parentheses. End
                // at the field delimiter to retain both those and explicit line joins.
                let Some(delimiter) = self
                    .tokens
                    .after(field.expression.end())
                    .iter()
                    .find(|t| !t.kind().is_trivia() && t.kind() != TokenKind::Rpar)
                else {
                    continue;
                };
                if !matches!(
                    delimiter.kind(),
                    TokenKind::Equal
                        | TokenKind::Exclamation
                        | TokenKind::Colon
                        | TokenKind::Rbrace
                ) {
                    continue;
                }
                let range = TextRange::new(field.start() + TextSize::new(1), delimiter.start());
                if self
                    .tokens
                    .in_range(range)
                    .iter()
                    .any(|t| t.kind() == TokenKind::Comment)
                {
                    continue;
                }
                let spelling = self.source[range].trim_end();
                let expression = if spelling.contains('\r') {
                    spelling.replace("\r\n", "\n").replace('\r', "\n")
                } else {
                    spelling.to_owned()
                };
                self.expressions.insert(field.start(), expression);
            }
            visitor::walk_t_string(self, template);
        }
    }
    let mut fields = Fields {
        source,
        tokens,
        expressions: HashMap::new(),
    };
    if tokens.iter().any(|t| t.kind() == TokenKind::TStringStart) {
        fields.visit_body(body);
    }
    fields.expressions
}

/// Limit the field metadata to forms supported by both native evaluators.
pub fn template_format_spec(
    field: &ast::InterpolatedElement,
) -> Result<Option<&ast::InterpolatedStringLiteralElement>, &'static str> {
    if field.debug_text.is_some() {
        return Err("debug t-string interpolation");
    }
    match field.format_spec.as_deref().map(|spec| &spec.elements[..]) {
        None | Some([]) => Ok(None),
        Some([ast::InterpolatedStringElement::Literal(literal)]) => Ok(Some(literal)),
        _ => Err("nested t-string format spec"),
    }
}
