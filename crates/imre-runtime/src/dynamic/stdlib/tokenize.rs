//! Adapt Ruff's Python lexer to physical lines supplied by TokenizerIter.
use error::{Failure, Unknown};
use record::{Lines, Record};
use ruff_python_ast::token::{TokenFlags, TokenKind as K};
use ruff_python_parser::{Mode, lexer::lex};
use std::{borrow::Cow, ops::Range};

mod bridge;
mod error;
mod interpolation;
use interpolation::Interpolation;
mod record;
pub(super) use bridge::scan;

struct Batch<'a> {
    lines: Lines<'a>,
    records: Vec<Record<'a>>,
    indents: Vec<String>,
    extra: bool,
    more: bool,
    error: Option<Failure>,
}

impl Batch<'_> {
    fn emit(&mut self, kind: K, range: Range<usize>, trailing: bool) {
        if let Some(record) = self.lines.record(kind, range, self.extra, trailing) {
            self.records.push(record);
        }
    }

    // A change in indentation must have the same ordering with tab widths eight and one.
    fn indentation(&mut self, source: &str, start: usize, token: usize, base: i64) -> bool {
        fn columns(indent: &str) -> (usize, usize) {
            indent.bytes().fold((0, 0), |(col, alt), ch| match ch {
                b'\t' => ((col / 8 + 1) * 8, alt + 1),
                12 => (0, 0),
                _ => (col + 1, alt + 1),
            })
        }
        let prefix = &source[start..token];
        let physical_start = prefix.rfind('\n').map_or(start, |n| start + n + 1);
        // Across continuations the first nonzero prefix sets the level. INDENT
        // itself spans the leading whitespace on the final physical line.
        let indent = prefix
            .split_inclusive('\n')
            .map(|line| line.trim_end_matches(['\n', '\r', '\\']))
            .find(|part| columns(part).0 != 0)
            .unwrap_or("");
        let line_base = || {
            base + source[..physical_start]
                .bytes()
                .filter(|&c| c == b'\n')
                .count() as i64
        };
        let (col, alt) = columns(indent);
        let (top, top_alt) = columns(self.indents.last().map_or("", String::as_str));
        if col > top && alt > top_alt {
            if self.indents.len() >= crate::dynamic::source_code::MAX_INDENT_LEVELS {
                self.error = Some(Failure::too_deep(&source[physical_start..], line_base()));
                return false;
            }
            self.indents.push(indent.to_owned());
            self.emit(K::Indent, physical_start..token, false);
            return true;
        }
        let keep = self.indents.partition_point(|s| columns(s).0 <= col);
        let (found, found_alt) = columns(self.indents[..keep].last().map_or("", String::as_str));
        if found == col && found_alt == alt {
            for _ in keep..self.indents.len() {
                self.emit(K::Dedent, token..token, false);
            }
            self.indents.truncate(keep);
            return true;
        }
        self.error = Some(Failure::indent(
            &source[physical_start..],
            line_base(),
            col >= top || found == col,
        ));
        false
    }

    fn unfinished(&mut self, source: &str, base: i64, eof: bool) {
        if eof {
            self.error = Some(Failure::incomplete(source, base));
        } else {
            self.more = true;
        }
    }

    fn trailing(&mut self, end: usize) {
        for _ in 0..self.indents.len() {
            self.emit(K::Dedent, end..end, true);
        }
        self.indents.clear();
        self.emit(K::EndOfFile, end..end, true);
    }

    fn read(&mut self, source: &str, base: i64, eof: bool) {
        // Decoding already handled a bytes source's BOM. For public tokens a
        // U+FEFF still in the callback text is a potential NAME character,
        // even at the start of a batch where Ruff would skip it. A non-ASCII
        // identifier letter of the same UTF-8 width preserves lexer offsets;
        // token strings, lines and diagnostics still use the original source.
        let mut input = if self.extra && source.contains('\u{feff}') {
            Cow::Owned(source.replace('\u{feff}', "名"))
        } else {
            Cow::Borrowed(source)
        };
        if !source.ends_with('\n') {
            input.to_mut().push('\n');
        }
        let mut lexer = lex(&input, Mode::Module);
        let mut line_start = Some(0);
        let mut brackets = Vec::new();
        let mut strings: Vec<Interpolation> = Vec::new();
        let mut previous = K::EndOfFile;
        let nul_line = source
            .find('\0')
            .map(|pos| (source[..pos].rfind('\n').map_or(0, |n| n + 1), pos));
        loop {
            let mut kind = lexer.next_token();
            let span = lexer.current_range();
            let start = span.start().to_usize().min(source.len());
            let end = span.end().to_usize().min(source.len());
            if let Some((bol, pos)) = nul_line
                && (start >= bol || end > bol)
            {
                self.error = Some(Failure::nul(source, base, pos));
                break;
            }
            if kind == K::EndOfFile || (kind == K::Unknown && start == source.len()) {
                if !brackets.is_empty() || !strings.is_empty() {
                    self.unfinished(source, base, eof);
                } else if eof {
                    self.trailing(end);
                }
                break;
            }
            // Ignore the per-buffer indent stack: our stack persists across reads.
            if matches!(kind, K::Indent | K::Dedent) {
                continue;
            }
            if !matches!(kind, K::Newline | K::NonLogicalNewline | K::Comment)
                && !(kind == K::Unknown && source[start..end].trim_start().starts_with('\\'))
                && let Some(bol) = line_start.take()
                && !self.indentation(source, bol, start, base)
            {
                break;
            }
            let flags = lexer.current_flags();
            if let Some(string) = strings.last_mut() {
                if kind == K::Newline && span.is_empty() {
                    // Ruff recovered at the batch boundary inside a replacement field.
                    continue;
                }
                if kind == K::Unknown && &source[start..end] == "}" {
                    self.error = Some(Failure::at(
                        source,
                        base,
                        start,
                        format!("{}: single '}}' is not allowed", string.name()),
                    ));
                    break;
                }
                let stalled_middle = kind == string.middle
                    && matches!(
                        input.as_bytes().get(span.end().to_usize()),
                        None | Some(b'\n' | b'\r')
                    );
                if stalled_middle
                    || kind.is_any_newline() && flags.contains(TokenFlags::UNCLOSED_STRING)
                {
                    // An unfinished literal may already have returned complete escaped braces.
                    for range in
                        record::token_ranges(source, kind, start..end, flags.is_raw_string())
                            .filter(|r| r.end < end)
                    {
                        self.emit(kind, range, false);
                    }
                    if !string.triple && string.in_format(brackets.len()) {
                        let name = string.name();
                        let offset =
                            source[start..end].trim_end_matches(['\r', '\n']).len() + start;
                        self.error = Some(Failure::at(
                            source,
                            base,
                            offset,
                            format!(
                                "{name}: newlines are not allowed in format specifiers for single quoted {name}s"
                            ),
                        ));
                    } else {
                        match error::unknown(
                            source,
                            string.start..source.len(),
                            base,
                            eof,
                            self.extra,
                        ) {
                            Unknown::More => self.more = true,
                            Unknown::Error(failure) => self.error = Some(failure),
                            _ => unreachable!("interpolated string begins with its literal prefix"),
                        }
                    }
                    break;
                }
                if string.empty_middle(
                    kind,
                    brackets.len(),
                    previous,
                    source[start..].starts_with("{{"),
                ) {
                    self.emit(string.middle, start..start, false);
                }
            }
            if kind == K::Unknown || flags.contains(TokenFlags::UNCLOSED_STRING) {
                let offset = if matches!(kind, K::FStringMiddle | K::TStringMiddle) {
                    strings.last().map_or(start, |s| s.start)
                } else {
                    start
                };
                match error::unknown(source, offset..end, base, eof, self.extra) {
                    Unknown::More => {
                        self.more = true;
                        break;
                    }
                    Unknown::Error(failure) => {
                        self.error = Some(failure);
                        break;
                    }
                    Unknown::Operator => kind = K::Question,
                    Unknown::Number => kind = K::Int,
                    Unknown::Name => kind = K::Identifier,
                }
            }
            if matches!(kind, K::Int | K::Float | K::Complex)
                && let Some(err) = error::number_end(source, start..end, base, self.extra)
            {
                self.error = Some(err);
                break;
            }
            match kind {
                K::FStringStart | K::TStringStart => strings.push(Interpolation::new(
                    start,
                    kind,
                    brackets.len(),
                    flags.is_triple_quoted_interpolated_string(),
                )),
                K::FStringEnd | K::TStringEnd => {
                    strings.pop();
                }
                K::Lpar | K::Lsqb | K::Lbrace => {
                    if brackets.len() == 200 {
                        self.error = Some(Failure::at(
                            source,
                            base,
                            start,
                            "too many nested parentheses",
                        ));
                        break;
                    }
                    brackets.push(start);
                }
                K::Rpar | K::Rsqb | K::Rbrace => {
                    let closer = source.as_bytes()[start] as char;
                    let expected = match kind {
                        K::Rpar => '(',
                        K::Rsqb => '[',
                        _ => '{',
                    };
                    let open = brackets.pop();
                    if !self.extra && open.is_none_or(|i| source.as_bytes()[i] as char != expected)
                    {
                        let msg = match open {
                            Some(i) => {
                                let mut msg = format!(
                                    "closing parenthesis '{closer}' does not match opening parenthesis '{}'",
                                    source.as_bytes()[i] as char
                                );
                                if source[i..start].contains('\n') {
                                    let row = base
                                        + 1
                                        + source[..i].bytes().filter(|&c| c == b'\n').count()
                                            as i64;
                                    msg.push_str(&format!(" on line {row}"));
                                }
                                msg
                            }
                            None => format!("unmatched '{closer}'"),
                        };
                        self.error = Some(Failure::at(source, base, start, msg));
                        break;
                    }
                }
                K::Newline => line_start = Some(end),
                K::NonLogicalNewline if line_start.is_some() => line_start = Some(end),
                _ => {}
            }
            previous = kind;
            for range in record::token_ranges(source, kind, start..end, flags.is_raw_string()) {
                self.emit(kind, range, false);
            }
        }
    }
}
