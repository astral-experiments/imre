//! Search an immutable Python code-point string when the pattern cannot distinguish
//! a surrogate from a private-use scalar. Captures always refer to the original subject.
use super::*;

/// Regular ASCII atoms, ranges, dot, and Python's word/digit/space classes all give
/// the same answer for U+E000 and a surrogate. Exclude escapes that can name
/// either code point and backreferences, which can compare two subject characters.
fn can_project(source: &str, matcher: &Matcher) -> bool {
    matches!(matcher, Matcher::Standard(_)) && safe_alphabet(source)
}

fn safe_alphabet(source: &str) -> bool {
    if !source.is_ascii() || source.contains("(?P=") {
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

// Reserve distinct private-use scalars for surrogate literals. Real private-use
// characters have identical membership in every class this pattern can express,
// and none may alias a surrogate.
fn encode(point: u32) -> char {
    let scalar = match point {
        0xd800..=0xdfff => point - 0xd800 + 0xf0000,
        0xf0000..=0xf07ff => 0xf0800,
        _ => point,
    };
    char::from_u32(scalar).unwrap()
}

/// Compile raw surrogate atoms separately for scalar-only and code-point subjects.
/// No Unicode scalar can match a surrogate literal, including under IGNORECASE.
pub(super) fn literal_patterns(points: &[u32]) -> Result<(String, String)> {
    let unsupported = || {
        Error::new(
            "NotImplementedError",
            "unsupported surrogate regular expression",
        )
    };
    let mut plain = String::new();
    let mut encoded = String::new();
    let mut in_set = false;
    let mut escaped = false;
    for &point in points {
        if (0xd800..=0xdfff).contains(&point) {
            if escaped || in_set {
                return Err(unsupported());
            }
            plain.push_str(r"[^\s\S]");
            encoded.push(encode(point));
        } else {
            let ch = char::from_u32(point)
                .filter(char::is_ascii)
                .ok_or_else(unsupported)?;
            plain.push(ch);
            encoded.push(ch);
            if !escaped && (ch == '[' || ch == ']') {
                in_set = ch == '[';
            }
            escaped = !escaped && ch == '\\';
        }
    }
    if !safe_alphabet(&plain) || plain.contains("[]") || plain.contains("[^]") {
        return Err(unsupported());
    }
    Ok((plain, encoded))
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
        Self::build(points, |p| char::from_u32(p).unwrap_or('\u{e000}'))
    }

    pub(super) fn for_literals(method: &str, points: &[u32]) -> Result<Self> {
        if !matches!(method, "match" | "search" | "fullmatch") {
            return Err(Error::new(
                "NotImplementedError",
                "unsupported method on a surrogate regular expression",
            ));
        }
        Self::build(points, encode)
    }

    fn build(points: &[u32], encode: fn(u32) -> char) -> Result<Self> {
        let mut text = String::new();
        let mut offsets = Vec::new();
        let chars = points.iter().copied().map(encode);
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
