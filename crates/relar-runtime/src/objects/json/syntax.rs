//! Format a failed stdlib JSON parse at Python's character offset. Serde owns
//! the successful path. This cursor only distinguishes the tokens Python expected.

use std::sync::LazyLock;

use regex_automata::meta::Regex;

use crate::{Error, Kind};

type Failure = (usize, &'static str);

struct Cursor<'a> {
    source: &'a str,
    pos: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.pos).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Result<(), Failure> {
        self.space();
        match self.peek() {
            Some(b'"') => self.string(),
            Some(open @ (b'[' | b'{')) => self.container(open == b'{'),
            _ => {
                static ATOM: LazyLock<Regex> = LazyLock::new(|| {
                    Regex::new(r"^(?:-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?|true|false|null|NaN|-?Infinity)").unwrap()
                });
                let token = ATOM
                    .find(&self.source[self.pos..])
                    .ok_or((self.pos, "Expecting value"))?;
                self.pos += token.end();
                Ok(())
            }
        }
    }

    fn string(&mut self) -> Result<(), Failure> {
        let start = self.pos;
        self.pos += 1;
        while let Some(byte) = self.peek() {
            self.pos += 1;
            match byte {
                b'"' => return Ok(()),
                0..=31 => return Err((self.pos - 1, "Invalid control character at")),
                b'\\' => match self.peek() {
                    Some(b'u') => {
                        let escape = self.pos;
                        self.pos += 1;
                        for _ in 0..4 {
                            if !self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                                return Err((escape, r"Invalid \uXXXX escape"));
                            }
                            self.pos += 1;
                        }
                    }
                    Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => self.pos += 1,
                    Some(_) => return Err((self.pos - 1, r"Invalid \escape")),
                    None => break,
                },
                _ => {}
            }
        }
        Err((start, "Unterminated string starting at"))
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

#[cold]
/// The caller has checked the complete document's container depth.
pub(super) fn decode_error(text: &str, fallback: serde_json::Error) -> Error {
    let mut cursor = Cursor {
        source: text,
        pos: 0,
    };
    let issue = if text.starts_with('\u{feff}') {
        Some((0, "Unexpected UTF-8 BOM (decode using utf-8-sig)"))
    } else if let Err(failure) = cursor.value() {
        Some(failure)
    } else {
        cursor.space();
        (cursor.pos < text.len()).then_some((cursor.pos, "Extra data"))
    };
    let message = if let Some((at, message)) = issue {
        let prefix = &text[..at];
        let line = 1 + memchr::memchr_iter(b'\n', prefix.as_bytes()).count();
        let column = 1 + prefix
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count();
        format!(
            "{message}: line {line} column {column} (char {})",
            prefix.chars().count()
        )
    } else {
        fallback.to_string()
    };
    Error::new(Kind::JsonDecodeError, message)
}
