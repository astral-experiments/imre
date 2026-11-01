//! Source locations for decoded string values outside Rust's Unicode scalar domain.
use ruff_python_ast::{
    StringFlags,
    token::{Token, TokenKind},
};
use ruff_text_size::{Ranged, TextRange};

/// Raw and byte literals have no cooked Unicode escapes.
pub fn cooked_content(token: &Token) -> Option<TextRange> {
    if !matches!(token.kind(), TokenKind::String | TokenKind::FStringMiddle) {
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
