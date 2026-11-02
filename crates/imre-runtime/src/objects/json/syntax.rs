//! Format a failed stdlib JSON parse at Python's character offset. Serde owns
//! the shallow successful path; the iterative decoder reuses these token rules.

use crate::StrOwner;
use std::sync::LazyLock;

use regex_automata::meta::Regex;

use crate::Error;

pub(super) type Failure = (usize, &'static str);

pub(super) struct Cursor<'a> {
    pub(super) source: &'a str,
    pub(super) pos: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.pos).copied()
    }

    pub(super) fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Result<(), Failure> {
        self.space();
        match self.peek() {
            Some(open @ (b'[' | b'{')) => self.container(open == b'{'),
            _ => self.scalar().map(|_| ()),
        }
    }

    pub(super) fn scalar(&mut self) -> Result<&'a str, Failure> {
        let start = self.pos;
        if self.peek() == Some(b'"') {
            self.string()?;
        } else {
            static ATOM: LazyLock<Regex> = LazyLock::new(|| {
                Regex::new(r"^(?:-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?|true|false|null|NaN|-?Infinity)").unwrap()
            });
            let token = ATOM
                .find(&self.source[self.pos..])
                .ok_or((self.pos, "Expecting value"))?;
            self.pos += token.end();
        }
        Ok(&self.source[start..self.pos])
    }

    fn string(&mut self) -> Result<(), Failure> {
        self.pos = string_end(self.source.as_bytes(), self.pos + 1, self.pos, true)?;
        Ok(())
    }

    fn container(&mut self, object: bool) -> Result<(), Failure> {
        self.pos += 1;
        self.space();
        let close = if object { b'}' } else { b']' };
        if self.peek() == Some(close) {
            self.pos += 1;
            return Ok(());
        }
        loop {
            if object {
                if self.peek() != Some(b'"') {
                    return Err((
                        self.pos,
                        "Expecting property name enclosed in double quotes",
                    ));
                }
                self.string()?;
                self.space();
                if self.peek() != Some(b':') {
                    return Err((self.pos, "Expecting ':' delimiter"));
                }
                self.pos += 1;
            }
            self.value()?;
            self.space();
            match self.peek() {
                Some(c) if c == close => {
                    self.pos += 1;
                    return Ok(());
                }
                Some(b',') => {
                    let comma = self.pos;
                    self.pos += 1;
                    self.space();
                    if self.peek() == Some(close) {
                        return Err((
                            comma,
                            if object {
                                "Illegal trailing comma before end of object"
                            } else {
                                "Illegal trailing comma before end of array"
                            },
                        ));
                    }
                }
                _ => return Err((self.pos, "Expecting ',' delimiter")),
            }
        }
    }
}

/// Token boundaries use bytes in a UTF-8 document and code points in a wide Python string.
/// Only ASCII syntax participates, so both use the same escape and error rules.
pub(crate) fn string_end<T: Copy + Into<u32>>(
    units: &[T],
    mut at: usize,
    start: usize,
    strict: bool,
) -> Result<usize, Failure> {
    let byte = |unit: &T| u8::try_from((*unit).into()).unwrap_or(0xff);
    while let Some(point) = units.get(at) {
        at += 1;
        match byte(point) {
            b'"' => return Ok(at),
            0..=31 if strict => return Err((at - 1, "Invalid control character at")),
            b'\\' => match units.get(at).map(byte) {
                Some(b'u') => {
                    let end = at + 5;
                    if end >= units.len()
                        || !units[at + 1..end]
                            .iter()
                            .all(|p| byte(p).is_ascii_hexdigit())
                    {
                        return Err((at, r"Invalid \uXXXX escape"));
                    }
                    at = end;
                }
                Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => at += 1,
                Some(_) => return Err((at - 1, r"Invalid \escape")),
                None => break,
            },
            _ => {}
        }
    }
    Err((start, "Unterminated string starting at"))
}

#[cold]
/// The caller has checked the complete document's container depth.
pub(super) fn decode_error(
    text: &str,
    fallback: serde_json::Error,
    owner: impl FnOnce() -> Option<StrOwner>,
) -> Error {
    let mut cursor = Cursor {
        source: text,
        pos: 0,
    };
    let issue = if let Err(failure) = cursor.value() {
        Some(failure)
    } else {
        cursor.space();
        (cursor.pos < text.len()).then_some((cursor.pos, "Extra data"))
    };
    let fallback_message;
    let (at, msg) = if let Some(issue) = issue {
        issue
    } else {
        // Serde can reject a syntactically valid string outside Imre's Unicode
        // domain. Its column is byte-based and one-indexed, unlike Python's pos.
        let line_start = text
            .split_inclusive('\n')
            .take(fallback.line().saturating_sub(1))
            .map(str::len)
            .sum::<usize>();
        let at = text.floor_char_boundary(
            (line_start + fallback.column().saturating_sub(1)).min(text.len()),
        );
        fallback_message = fallback.to_string();
        let msg = fallback_message
            .rsplit_once(" at line ")
            .map_or(fallback_message.as_str(), |(msg, _)| msg);
        (at, msg)
    };
    let Some(doc) = owner() else {
        return Error::allocation();
    };
    at_error(doc, at, msg)
}

#[cold]
pub(super) fn at_error(doc: StrOwner, at: usize, msg: &str) -> Error {
    let prefix = &doc[..at];
    let lineno = 1 + memchr::memchr_iter(b'\n', prefix.as_bytes()).count();
    let colno = 1 + prefix
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count();
    let pos = prefix.chars().count();
    Error::json_decode(msg, doc, pos, lineno, colno)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_diagnostic_owner_raises_memory_error() {
        let error = serde_json::from_str::<serde_json::Value>("!").unwrap_err();
        assert!(decode_error("!", error, || None).is_kind(crate::Kind::MemoryError));
    }
}
