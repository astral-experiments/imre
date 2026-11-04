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
        class.push(char::from_u32(first).unwrap());
        if first < last {
            class.push('-');
            class.push(char::from_u32(last).unwrap());
        }
    }
}

/// Project a standalone literal class, including ranges and a simple repeat.
fn literal_class(points: &[u32]) -> Option<(String, String)> {
    let (points, repeat) = match points.split_last()? {
        (0x2a, rest) => (rest, "*"),
        (0x2b, rest) => (rest, "+"),
        (0x3f, rest) => (rest, "?"),
        _ => (points, ""),
    };
    let members = points
        .strip_prefix(&[u32::from('[')])?
        .strip_suffix(&[u32::from(']')])?;
    let (negated, mut members) = match members.strip_prefix(&[u32::from('^')]) {
        Some(rest) => (true, rest),
        None => (false, members),
    };
    // The full parser handles escaped endpoints, literal hyphens and set operators.
    let endpoint = |p| {
        !matches!(
            char::from_u32(p),
            Some('[' | ']' | '-' | '\\' | '^' | '&' | '|' | '~')
        )
    };
    let prefix = if negated { "[^" } else { "[" };
    let mut plain = String::from(prefix);
    let mut encoded = String::from(prefix);
    while let Some((&first, rest)) = members.split_first() {
        let (last, tail) = match rest {
            [0x2d, last, tail @ ..] => (*last, tail),
            _ => (first, rest),
        };
        members = tail;
        if !endpoint(first) || !endpoint(last) || first > last {
            return None;
        }

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
        plain = if negated { r"[\s\S]" } else { r"[^\s\S]" }.into();
    } else {
        plain.push(']');
    }
    encoded.push(']');
    plain.push_str(repeat);
    encoded.push_str(repeat);
    Some((plain, encoded))
}

/// Compile raw surrogate atoms separately for scalar-only and code-point subjects.
/// No Unicode scalar can match a surrogate literal, including under IGNORECASE.
pub(super) fn literal_patterns(points: &[u32]) -> Result<(String, String)> {
    if let Some(class) = literal_class(points) {
        return Ok(class);
    }
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
