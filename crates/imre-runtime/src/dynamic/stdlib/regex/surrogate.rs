//! Search an immutable Python code-point string when the pattern cannot distinguish
//! a surrogate from a private-use scalar. Captures always refer to the original subject.
use super::*;

/// Regular ASCII atoms, ranges, dot, and Python's word/digit/space classes all give
/// the same answer for U+E000 and a surrogate. Exclude escapes that can name
/// either code point and backreferences, which can compare two subject characters.
fn can_project(source: &str, matcher: &Matcher) -> bool {
    if !source.is_ascii() || !matches!(matcher, Matcher::Standard(_)) || source.contains("(?P=") {
        return false;
    }
    let mut bytes = source.bytes();
    while let Some(ch) = bytes.next() {
        if ch != b'\\' {
            continue;
        }
        match bytes.next() {
            Some(b'x') => {
                if !(0..2).all(|_| bytes.next().is_some_and(|b| b.is_ascii_hexdigit())) {
                    return false;
                }
            }
            Some(ch) if !ch.is_ascii_alphanumeric() || b"AbBdDsSwWZzafnrtv".contains(&ch) => {}
            _ => return false,
        }
    }
    true
}

pub(super) struct Projection {
    pub(super) text: String,
    offsets: Vec<usize>,
}

impl Projection {
    pub(super) fn new(
        source: Option<&str>,
        matcher: &Matcher,
        method: &str,
        points: &[u32],
    ) -> Result<Self> {
        if !matches!(method, "search" | "match" | "fullmatch" | "sub")
            || !source.is_some_and(|s| can_project(s, matcher))
        {
            return Err(Error::new(
                "NotImplementedError",
                "unsupported regular expression on a string containing surrogates",
            ));
        }
        let mut text = String::new();
        let mut offsets = Vec::new();
        let chars = points
            .iter()
            .map(|&p| char::from_u32(p).unwrap_or('\u{e000}'));
        text.try_reserve_exact(chars.clone().map(char::len_utf8).sum())
            .map_err(|_| Error::new("MemoryError", ""))?;
        offsets
            .try_reserve_exact(points.len() + 1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        for ch in chars {
            offsets.push(text.len());
            text.push(ch);
        }
        offsets.push(text.len());
        Ok(Self { text, offsets })
    }

    pub(super) fn original(&self, span: Span) -> Span {
        Span {
            start: self
                .offsets
                .binary_search(&span.start)
                .expect("UTF-8 capture start"),
            end: self
                .offsets
                .binary_search(&span.end)
                .expect("UTF-8 capture end"),
        }
    }
}
