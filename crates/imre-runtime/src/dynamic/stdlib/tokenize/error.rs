//! Translate lexer failures into the located exceptions consumed by Lib/tokenize.py.
use std::ops::Range;

use super::super::{Context, Id, NONE, Result, Value};

pub(super) struct Failure {
    kind: &'static str,
    message: String,
    row: i64,
    column: i64,
    text: Option<String>,
    end_row: Option<i64>,
    end_column: Option<i64>,
}

impl Failure {
    /// CPython points to a character, using one-based Unicode columns.
    pub(super) fn at(source: &str, base: i64, offset: usize, message: impl Into<String>) -> Self {
        let before = &source[..offset];
        let row = base + 1 + before.bytes().filter(|&c| c == b'\n').count() as i64;
        let start = before.rfind('\n').map_or(0, |i| i + 1);
        let column = source[start..offset].chars().count() as i64 + 1;
        let text = source[start..].split('\n').next().unwrap_or_default();
        Self {
            kind: "SyntaxError",
            message: message.into(),
            row,
            column,
            text: Some(text.to_owned()),
            end_row: Some(row),
            end_column: Some(column),
        }
    }

    pub(super) fn nul(source: &str, base: i64, offset: usize) -> Self {
        let mut err = Self::at(
            source,
            base,
            offset,
            "source code cannot contain null bytes",
        );
        err.text
            .as_mut()
            .unwrap()
            .truncate(source[..offset].rsplit('\n').next().unwrap().len());
        err.column = 0;
        err.end_column = Some(0);
        err
    }

    pub(super) fn incomplete(source: &str, base: i64) -> Self {
        let last = source.strip_suffix('\n').unwrap_or(source);
        let mut err = Self::at(
            source,
            base,
            last.len(),
            "unexpected EOF in multi-line statement",
        );
        err.column = 0;
        err.text = None;
        err.end_column = None;
        err
    }

    fn continuation(source: &str, base: i64) -> Self {
        let mut err = Self::incomplete(source, base);
        // A backslash retains the suffix of the physical input buffer starting at
        // the first nonblank continued line. This offset is in UTF-8 bytes.
        let mut bytes = 0;
        for line in source.split_inclusive('\n').rev() {
            let Some(before) = line.trim_end_matches(['\r', '\n']).strip_suffix('\\') else {
                break;
            };
            bytes += line.len() + usize::from(!line.ends_with('\n'));
            if !before.trim().is_empty() {
                err.column = bytes as i64;
            }
        }
        err
    }

    pub(super) fn indent(
        source: &str,
        base: i64,
        mismatch: imre_ast::indentation::Mismatch,
    ) -> Self {
        let (kind, msg) = mismatch.diagnostic();
        Self::line_end(source, base, 0, kind, msg)
    }

    pub(super) fn too_deep(source: &str, base: i64) -> Self {
        Self::line_end(
            source,
            base,
            0,
            "IndentationError",
            "too many levels of indentation",
        )
    }

    fn line_end(source: &str, base: i64, offset: usize, kind: &'static str, msg: &str) -> Self {
        let mut err = Self::at(source, base, offset, msg);
        err.kind = kind;
        err.column = err.text.as_ref().unwrap().chars().count() as i64 + 1;
        err.end_row = None;
        err.end_column = None;
        err
    }

    pub(super) fn boxed(self, ctx: &mut Context) -> Result<Id> {
        let message = ctx.text(&self.message);
        let file = ctx.text("<string>");
        let row = ctx.int(self.row);
        let column = ctx.int(self.column);
        let end_row = self.end_row.map_or(NONE, |n| ctx.int(n));
        let end_column = self.end_column.map_or(NONE, |n| ctx.int(n));
        let text = self.text.as_ref().map_or(NONE, |s| ctx.text(s));
        let details = ctx.alloc(Value::Tuple(vec![
            file, row, column, text, end_row, end_column,
        ]));
        // PyErr_SyntaxLocation sets attributes separately from args at EOF.
        let args = [message, details];
        let fields = ctx.syntax_fields(&args);
        let mut err = ctx.exception(
            self.kind,
            if self.text.is_some() {
                &args
            } else {
                &args[..1]
            },
        );
        err.message = self.message + &ctx.syntax_suffix(file, row)?;
        ctx.retain_syntax_fields(&mut err, fields)?;
        Ok(ctx.raised_instance(err))
    }
}

pub(super) enum Unknown {
    More,
    Operator,
    Number,
    Name,
    Error(Failure),
}

fn radix(literal: &str) -> Option<&'static str> {
    match literal.as_bytes() {
        [b'0', b'x' | b'X', ..] => Some("hexadecimal"),
        [b'0', b'b' | b'B', ..] => Some("binary"),
        [b'0', b'o' | b'O', ..] => Some("octal"),
        _ => None,
    }
}

/// Ruff ends a number before an invalid digit, underscore or identifier. CPython's
/// tokenizer can reject that same input before returning the number to the caller.
pub(super) fn number_end(
    source: &str,
    range: Range<usize>,
    base: i64,
    extra: bool,
) -> Option<Failure> {
    let literal = &source[range.clone()];
    let tail = &source[range.end..];
    let first = *tail.as_bytes().first()?;
    if !first.is_ascii_alphanumeric() && first != b'_' {
        return None;
    }
    let kind = radix(literal).unwrap_or(if literal.ends_with(['j', 'J']) {
        "imaginary"
    } else {
        "decimal"
    });
    if matches!(kind, "binary" | "octal") {
        let digits = tail.strip_prefix('_').unwrap_or(tail);
        if let Some(digit) = digits.as_bytes().first().filter(|b| b.is_ascii_digit()) {
            let offset = range.end + usize::from(first == b'_');
            return Some(Failure::at(
                source,
                base,
                offset,
                format!("invalid digit '{}' in {kind} literal", *digit as char),
            ));
        }
    }
    let offset = if first == b'_' && kind != "imaginary" && !literal.ends_with('.') {
        range.end
    } else if kind == "decimal"
        && !literal.contains(['e', 'E'])
        && matches!(tail.as_bytes(), [b'e' | b'E', b'+' | b'-', ..])
    {
        range.end + 1
    } else if extra {
        return None;
    } else {
        // Names beginning if/in/is and a few complete keywords can follow a strict number
        // (CPython emits a SyntaxWarning for these).
        let word = tail
            .split(|c: char| c.is_ascii() && !c.is_ascii_alphanumeric() && c != '_')
            .next();
        if ["if", "in", "is"].iter().any(|w| tail.starts_with(w))
            || matches!(word, Some("and" | "else" | "for" | "not" | "or"))
        {
            return None;
        }
        // Numeric literals contain only ASCII, so the preceding byte is a character boundary.
        range.end - 1
    };
    Some(Failure::at(
        source,
        base,
        offset,
        format!("invalid {kind} literal"),
    ))
}

pub(super) fn unknown(
    source: &str,
    range: Range<usize>,
    base: i64,
    eof: bool,
    extra: bool,
) -> Unknown {
    let raw = &source[range.clone()];
    let at = |offset, message| Unknown::Error(Failure::at(source, base, offset, message));
    let content = raw.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    if content.starts_with(['\'', '"']) {
        let triple = content.starts_with("'''") || content.starts_with("\"\"\"");
        let backslashes = content
            .trim_end_matches(['\r', '\n'])
            .bytes()
            .rev()
            .take_while(|&c| c == b'\\')
            .count();
        if !eof && range.end == source.len() && (triple || backslashes % 2 != 0) {
            return Unknown::More;
        }
        let prefix = raw[..raw.len() - content.len()].to_ascii_lowercase();
        let kind = if prefix.contains('f') {
            "f-string"
        } else if prefix.contains('t') {
            "t-string"
        } else {
            "string"
        };
        let qualifier = if triple { "triple-quoted " } else { "" };
        let detected = base
            + 1
            + source[..range.end]
                .strip_suffix('\n')
                .unwrap_or(&source[..range.end])
                .bytes()
                .filter(|&c| c == b'\n')
                .count() as i64;
        let mut message =
            format!("unterminated {qualifier}{kind} literal (detected at line {detected})");
        let quote = content.as_bytes()[0];
        if !triple && kind == "string" && content.as_bytes().windows(2).any(|w| w == [b'\\', quote])
        {
            message.push_str("; perhaps you escaped the end quote?");
        }
        return at(range.start, message);
    }
    if raw.trim_start().starts_with('\\') {
        if range.end == source.len() {
            return if eof {
                Unknown::Error(Failure::continuation(source, base))
            } else {
                Unknown::More
            };
        }
        return Unknown::Error(Failure::line_end(
            source,
            base,
            range.start,
            "SyntaxError",
            "unexpected character after line continuation character",
        ));
    }
    if raw.starts_with('0') {
        if let Some(kind) = radix(raw) {
            let next = &source[range.end..];
            let offset = range.end - 1 + usize::from(next.starts_with('_'));
            if let Some(digit) = next
                .strip_prefix('_')
                .unwrap_or(next)
                .chars()
                .next()
                .filter(char::is_ascii_digit)
            {
                return at(
                    offset + 1,
                    format!("invalid digit '{digit}' in {kind} literal"),
                );
            }
            return at(offset, format!("invalid {kind} literal"));
        }
        if extra {
            return Unknown::Number;
        }
        let mut err = Failure::at(
            source,
            base,
            range.start,
            "leading zeros in decimal integer literals are not permitted; use an 0o prefix for octal integers",
        );
        let zeros = raw.find(|c: char| matches!(c, '1'..='9')).unwrap_or(0);
        err.end_column = Some(err.column + zeros as i64);
        return Unknown::Error(err);
    }
    if let Some(ch) = raw.chars().next() {
        if ch == '\0' {
            return Unknown::Error(Failure::nul(source, base, range.start));
        }
        if ch.is_ascii_graphic() && raw.len() == 1 {
            return Unknown::Operator;
        }
        if !ch.is_ascii() && extra {
            return Unknown::Name;
        }
        let diagnostic = if ch.is_control() || ch.is_whitespace() {
            format!("invalid non-printable character U+{:04X}", ch as u32)
        } else {
            format!("invalid character '{ch}' (U+{:04X})", ch as u32)
        };
        return at(range.start, diagnostic);
    }
    at(range.start, "invalid token".to_owned())
}
