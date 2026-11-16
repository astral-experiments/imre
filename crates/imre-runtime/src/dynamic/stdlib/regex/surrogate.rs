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
            // Three octal digits up to 0o377 denote a literal below U+0100, even outside a class.
            Some(b'0'..=b'3') if (0..2).all(|_| matches!(bytes.next(), Some(b'0'..=b'7'))) => {}
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

/// Write a nonempty interval of Unicode scalars into an open character class.
fn push_range(class: &mut String, first: u32, last: u32) {
    if first <= last {
        write!(class, r"\U{first:08x}").unwrap();
        if first < last {
            write!(class, r"-\U{last:08x}").unwrap();
        }
    }
}

/// Project a standalone literal class, including ranges and a simple repeat.
pub(super) fn class_patterns(source: &str) -> Option<(String, String)> {
    if !source.starts_with('[')
        || (source.is_ascii() && !source.contains(r"\u") && !source.contains(r"\U"))
    {
        return None;
    }
    let class = imre_regex::literal_class(source)?;
    if class.ranges.iter().all(|&(_, last)| last < 0x80) {
        return None;
    }
    let prefix = if class.negated { "[^" } else { "[" };
    let mut plain = String::from(prefix);
    let mut encoded = String::from(prefix);
    // D7FF and E000 are adjacent Unicode scalars. The backend can introduce both
    // as a spurious gap when negating separate ranges that contain these endpoints.
    let contains = |point| {
        class
            .ranges
            .iter()
            .any(|&(low, high)| low <= point && point <= high)
    };
    if class.negated && contains(0xd7ff) && contains(0xe000) {
        push_range(&mut plain, 0xd7ff, 0xe000);
        push_range(&mut encoded, 0xd7ff, 0xe000);
    }
    for (first, last) in class.ranges {
        // A scalar-only subject has no code points in the surrogate interval.
        push_range(&mut plain, first, last.min(0xd7ff));
        push_range(&mut plain, first.max(0xe000), last);

        // The encoded order differs from Python's. Partition the range before
        // translating it, so crossing a boundary cannot include unrelated points.
        push_range(&mut encoded, first, last.min(0xd7ff));
        let low = first.max(0xd800);
        let high = last.min(0xdfff);
        if low <= high {
            push_range(&mut encoded, encode(low) as u32, encode(high) as u32);
        }
        push_range(&mut encoded, first.max(0xe000), last.min(0xeffff));
        if first <= 0xf0800 && last >= 0xf0000 {
            // All genuine scalars in this window project to the same sentinel.
            if first > 0xf0000 || last < 0xf0800 {
                return None;
            }
            encoded.push('\u{f0800}');
        }
        push_range(&mut encoded, first.max(0xf0801), last);
    }
    if plain.len() == prefix.len() {
        plain = if class.negated { r"[\s\S]" } else { r"[^\s\S]" }.into();
    } else {
        plain.push(']');
    }
    encoded.push(']');
    plain.push_str(class.repeat);
    encoded.push_str(class.repeat);
    Some((plain, encoded))
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
    if points.first() == Some(&u32::from('[')) {
        // Spell literals from a Python code-point string for the same class parser.
        let mut source = String::new();
        let mut escaped = false;
        for &point in points {
            if let Some(ch) = char::from_u32(point) {
                source.push(ch);
                escaped = ch == '\\' && !escaped;
            } else if escaped {
                return Err(unsupported());
            } else {
                write!(source, r"\u{point:04x}").unwrap();
            }
        }
        if let Some(class) = class_patterns(&source) {
            return Ok(class);
        }
    }
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
        if !matches!(
            method,
            "search" | "match" | "fullmatch" | "findall" | "sub" | "split"
        ) || !source.is_some_and(|s| can_project(s, matcher))
        {
            return Err(Error::new(
                "NotImplementedError",
                "unsupported regular expression on a string containing surrogates",
            ));
        }
        Self::build(points, |p| char::from_u32(p).unwrap_or('\u{e000}'))
    }

    pub(super) fn for_literals(method: &str, points: &[u32]) -> Result<Self> {
        if !matches!(
            method,
            "match" | "search" | "fullmatch" | "findall" | "split" | "sub"
        ) {
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
