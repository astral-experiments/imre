//! Adapt CPython's indentation columns to the pinned Ruff lexer while retaining source offsets.

use ruff_python_ast::token::TokenKind;
use ruff_python_parser::{Mode, ParseError, Parsed, lexer::lex};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug)]
pub struct Error {
    start: usize,
    pub line: usize,
    pub offset: usize,
    pub kind: &'static str,
    pub message: &'static str,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}: {}", self.line, self.kind, self.message)
    }
}

pub struct Source<'a> {
    pub text: Cow<'a, str>,
    issue: Option<Error>,
}

impl<'a> Source<'a> {
    pub fn original(source: &'a str) -> Self {
        Self {
            text: Cow::Borrowed(source),
            issue: None,
        }
    }

    /// Prefer a grammar error in an earlier statement to a later indentation error.
    pub fn first_issue(&self, error: Option<&ParseError>) -> Option<Error> {
        self.issue.filter(|issue| {
            error.is_none_or(|e| {
                // An unrepresentable view can still have an ordinary parser diagnostic.
                issue.kind != "NotImplementedError" && e.location.start().to_usize() >= issue.start
            })
        })
    }

    pub fn parse_module(&self) -> Result<Parsed<ruff_python_ast::ModModule>, String> {
        let parsed = ruff_python_parser::parse_module(&self.text);
        if let Some(issue) = self.first_issue(parsed.as_ref().err()) {
            return Err(issue.to_string());
        }
        parsed.map_err(|e| e.to_string())
    }
}

/// CPython takes indentation from the first nonzero prefix before a backslash join.
/// For that prefix only, it uses the expanded column for both comparisons.
pub fn columns(prefix: &str) -> (usize, usize) {
    let mut pair = (0, 0);
    for byte in prefix.bytes() {
        let (col, alt) = pair;
        pair = match byte {
            b' ' => (col + 1, alt + 1),
            b'\t' => ((col / 8 + 1) * 8, alt + 1),
            b'\x0c' => (0, 0),
            b'\\' if col != 0 => return (col, col),
            _ => pair,
        };
    }
    pair
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mismatch {
    Tabs,
    Dedent,
}

impl Mismatch {
    pub fn diagnostic(self) -> (&'static str, &'static str) {
        match self {
            Self::Tabs => (
                "TabError",
                "inconsistent use of tabs and spaces in indentation",
            ),
            Self::Dedent => (
                "IndentationError",
                "unindent does not match any outer indentation level",
            ),
        }
    }
}

/// Determine the new logical suite depth using the eight-column and one-column stacks.
/// The tokenizer retains its stack across reads, whereas a parser input owns one stack.
pub fn next_depth<T>(
    stack: &[T],
    (col, alt): (usize, usize),
    level: impl Fn(&T) -> (usize, usize),
) -> Result<usize, Mismatch> {
    let (top, top_alt) = stack.last().map_or((0, 0), &level);
    if col > top {
        return if alt > top_alt {
            Ok(stack.len() + 1)
        } else {
            Err(Mismatch::Tabs)
        };
    }
    let keep = stack.partition_point(|s| level(s).0 <= col);
    let found = stack[..keep].last().map_or((0, 0), level);
    if found.0 != col {
        Err(Mismatch::Dedent)
    } else if found.1 != alt {
        Err(Mismatch::Tabs)
    } else {
        Ok(keep)
    }
}

/// A bare backslash is a lexical error. Only a complete explicit join belongs to indentation.
fn indentation_prefix(text: &str) -> bool {
    text.split("\\\n")
        .all(|part| part.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\x0c')))
}

/// Yield only indentation before the first token of each logical statement, along
/// with the suite depth that Ruff assigned. Blank and comment-only lines reset
/// the start offset; continuation lines inside expressions stay within the statement.
fn logical_indents(
    source: &str,
) -> impl Iterator<Item = (std::ops::Range<usize>, Option<usize>)> + '_ {
    use TokenKind::{Comment, Dedent, EndOfFile, Indent, Newline, NonLogicalNewline, Unknown};
    let mut lexer = lex(source, Mode::Module);
    let mut start = Some(if source.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    });
    let mut depth = Some(0usize);
    std::iter::from_fn(move || {
        loop {
            let kind = lexer.next_token();
            let span = lexer.current_range();
            match kind {
                EndOfFile => return None,
                Newline => start = Some(span.end().to_usize()),
                NonLogicalNewline if start.is_some() => start = Some(span.end().to_usize()),
                Indent => depth = depth.map(|d| d + 1),
                Dedent => depth = depth.and_then(|d| d.checked_sub(1)),
                NonLogicalNewline | Comment => {}
                // Width-two indentation errors can precede the real token at the same offset.
                Unknown if indentation_prefix(&source[span]) => depth = None,
                // The lexer rejects a malformed leading join before it compares indentation.
                Unknown if start.is_some() && &source[span] == "\\" => return None,
                _ => {
                    if let Some(begin) = start.take() {
                        return Some((begin..span.start().to_usize(), depth));
                    }
                }
            }
        }
    })
}

/// Encode one column per active suite. Any surplus bytes reset the parser's
/// columns; explicit joins and all physical newlines retain their original positions.
fn write_depth(prefix: &str, bytes: &mut [u8], depth: usize) -> bool {
    let mut start = 0;
    for segment in prefix.split("\\\n") {
        if segment.len() >= depth {
            let end = start + segment.len();
            for byte in &mut *bytes {
                if matches!(*byte, b' ' | b'\t' | b'\x0c') {
                    *byte = b'\x0c';
                }
            }
            bytes[end - depth..end].fill(b' ');
            return true;
        }
        start += segment.len() + 2;
    }
    false
}

/// Keep the original bytes whenever Ruff gives the same suite depths. For a valid
/// disagreement, only the parser's copy of logical indentation changes. All AST
/// ranges still index the original source, including debug fields and string data.
pub fn prepare(source: &str) -> Source<'_> {
    if !source.contains('\t') {
        return Source::original(source);
    }
    let mut bytes = source.as_bytes().to_vec();
    let mut stack = Vec::new();
    let mut corrected = false;
    let mut tab_indent = false;
    let mut issue = None;
    for (range, ruff_depth) in logical_indents(source) {
        let prefix = &source[range.clone()];
        // Leave any unrelated lexer recovery (and the rest of its input) to Ruff.
        if !indentation_prefix(prefix) {
            break;
        }
        let pair = columns(prefix);
        tab_indent |= prefix.contains('\t');
        let failure = match next_depth(&stack, pair, |p| *p) {
            Ok(depth) => {
                stack.resize(depth, pair);
                corrected |= ruff_depth != Some(depth);
                if write_depth(prefix, &mut bytes[range.clone()], depth) {
                    continue;
                }
                (
                    "NotImplementedError",
                    "continued indentation is too short to represent this suite depth",
                )
            }
            Err(mismatch) => mismatch.diagnostic(),
        };
        issue = Some(Error {
            line: 1 + source[..range.end].bytes().filter(|&b| b == b'\n').count(),
            start: range.start,
            offset: range.end,
            kind: failure.0,
            message: failure.1,
        });
        break;
    }
    if !tab_indent {
        return Source::original(source);
    }
    let text = if corrected {
        Cow::Owned(String::from_utf8(bytes).expect("only ASCII whitespace was replaced"))
    } else {
        Cow::Borrowed(source)
    };
    Source { text, issue }
}

pub fn parser_source(source: &str) -> Result<Cow<'_, str>, Error> {
    let input = prepare(source);
    match input.issue {
        Some(issue) => Err(issue),
        None => Ok(input.text),
    }
}

/// Recreate the first indentation of a sliced generator suite after a synthetic header.
pub fn generator_prefix(source_before_body: &str) -> Cow<'_, str> {
    let mut previous = source_before_body.rsplit('\n');
    let last = previous.next().unwrap_or_default();
    if !last.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\x0c')) {
        return Cow::Borrowed(" ");
    }
    let mut expanded = None;
    for line in previous {
        let Some(prefix) = line.strip_suffix('\\') else {
            break;
        };
        if !prefix.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\x0c')) {
            break;
        }
        let (col, _) = columns(prefix);
        if col != 0 {
            expanded = Some(col);
        }
    }
    match expanded {
        Some(col) => Cow::Owned(" ".repeat(col)),
        None => Cow::Borrowed(last),
    }
}
