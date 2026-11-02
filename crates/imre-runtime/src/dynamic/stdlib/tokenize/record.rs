//! Convert Ruff byte ranges into Python 3.14 tokenizer records.

use ruff_python_ast::token::TokenKind;
use std::ops::Range;

pub(super) struct Record<'a> {
    pub(super) ty: i64,
    pub(super) text: &'a str,
    pub(super) start: (i64, i64),
    pub(super) end: (i64, i64),
    pub(super) line: &'a str,
}

pub(super) struct Lines<'a> {
    source: &'a str,
    row_base: i64,
    starts: Vec<usize>,
    comment: Option<usize>,
    column_byte: usize,
    column: i64,
}

impl<'a> Lines<'a> {
    pub(super) fn new(source: &'a str, row_base: i64) -> Self {
        let mut starts = vec![0];
        starts.extend(
            memchr::memchr_iter(b'\n', source.as_bytes())
                .map(|i| i + 1)
                .take_while(|&i| i < source.len()),
        );
        Self {
            source,
            row_base,
            starts,
            comment: None,
            column_byte: 0,
            column: 0,
        }
    }

    fn row(&self, offset: usize) -> usize {
        self.starts.partition_point(|&start| start <= offset) - 1
    }

    fn line_end(&self, row: usize) -> usize {
        self.starts
            .get(row + 1)
            .copied()
            .unwrap_or(self.source.len())
    }

    fn position(&mut self, row: usize, byte: usize) -> (i64, i64) {
        let line_start = self.starts[row];
        if byte < self.column_byte || self.column_byte <= line_start {
            self.column_byte = line_start;
            self.column = 0;
        }
        self.column += self.source[self.column_byte..byte].chars().count() as i64;
        self.column_byte = byte;
        (self.row_base + row as i64 + 1, self.column)
    }

    pub(super) fn record(
        &mut self,
        kind: TokenKind,
        mut range: Range<usize>,
        extra: bool,
        trailing: bool,
    ) -> Option<Record<'a>> {
        use TokenKind as K;
        let previous_comment = self.comment.take();
        if !extra && kind == K::Comment {
            self.comment = Some(range.start);
            return None;
        }
        if !extra && kind == K::NonLogicalNewline {
            return None;
        }

        let ty = token_type(kind, extra)?;
        if trailing {
            let row = self.starts.len() - 1;
            let final_line = &self.source[self.starts[row]..];
            let (pos, line) = if extra {
                (
                    (
                        self.row_base + row as i64 + 1 + i64::from(!final_line.is_empty()),
                        0,
                    ),
                    "",
                )
            } else {
                if self.source.is_empty() {
                    return None;
                }
                // A skipped blank or comment-only line has no cached token text at EOF.
                let trimmed = final_line.trim_start();
                let line = if trimmed.is_empty() || trimmed.starts_with('#') {
                    ""
                } else {
                    final_line
                };
                ((self.row_base + row as i64 + 1, -1), line)
            };
            return Some(Record {
                ty,
                text: "",
                start: pos,
                end: pos,
                line,
            });
        }

        if kind == K::Dedent {
            range.start = range.end;
        } else if kind == K::Indent {
            // An explicit line continuation can span the indentation prefix.
            range.start = range.start.max(self.starts[self.row(range.end)]);
        }
        let mut implied_width = 0;
        if kind.is_any_newline() {
            if extra && range.end == self.source.len() && !self.source.ends_with('\n') {
                implied_width = 1;
            } else if kind == K::Newline && !extra {
                // The parser's NEWLINE spans the inline comment and CR, excluding the LF.
                if self.source[range.clone()].ends_with('\n') {
                    range.end -= 1;
                }
                range.start = previous_comment.unwrap_or(range.start);
            }
        }

        let first = self.row(range.start);
        let last = if kind.is_any_newline() {
            first
        } else {
            self.row(range.end)
        };
        let mut start = self.position(first, range.start);
        let mut end = self.position(last, range.end);
        let mut text = &self.source[range];
        if !extra && matches!(kind, K::Indent | K::Dedent) {
            start.1 = -1;
            end.1 = -1;
            text = "";
        } else {
            end.1 += implied_width;
            if kind == K::NonLogicalNewline && implied_width > 0 {
                text = "";
            }
        }
        Some(Record {
            ty,
            text,
            start,
            end,
            line: &self.source[self.starts[first]..self.line_end(last)],
        })
    }
}

/// CPython ends a middle token at each escaped brace and named Unicode escape.
/// Ruff provides one range for the whole literal portion of the interpolation.
pub(super) fn token_ranges(
    source: &str,
    kind: TokenKind,
    range: Range<usize>,
    raw: bool,
) -> impl Iterator<Item = Range<usize>> {
    let middle = matches!(kind, TokenKind::FStringMiddle | TokenKind::TStringMiddle);
    let bytes = source.as_bytes();
    let mut remaining = Some(range);
    std::iter::from_fn(move || {
        let range = remaining.take()?;
        if middle {
            let mut i = range.start;
            while i < range.end {
                let (end, next) = match bytes[i] {
                    b'{' | b'}' if i + 1 < range.end && bytes[i + 1] == bytes[i] => (i + 1, i + 2),
                    b'\\' if !raw && bytes[i..range.end].starts_with(br"\N{") => {
                        let Some(end) = memchr::memchr(b'}', &bytes[i + 3..range.end]) else {
                            break;
                        };
                        let end = i + 4 + end;
                        (end, end)
                    }
                    b'\\' if bytes.get(i + 1) == Some(&b'\\') => {
                        i += 2;
                        continue;
                    }
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                if next < range.end {
                    remaining = Some(next..range.end);
                }
                return Some(range.start..end);
            }
        }
        Some(range)
    })
}

// Numeric token constants come from CPython 3.14's Grammar/Tokens.
fn token_type(kind: TokenKind, extra: bool) -> Option<i64> {
    use TokenKind as K;
    let ty = match kind {
        K::EndOfFile => 0,
        K::Identifier => 1,
        k if k.is_keyword() => 1,
        K::Int | K::Float | K::Complex => 2,
        K::String => 3,
        K::Newline => 4,
        K::Indent => 5,
        K::Dedent => 6,
        K::Lpar => 7,
        K::Rpar => 8,
        K::Lsqb => 9,
        K::Rsqb => 10,
        K::Colon => 11,
        K::Comma => 12,
        K::Semi => 13,
        K::Plus => 14,
        K::Minus => 15,
        K::Star => 16,
        K::Slash => 17,
        K::Vbar => 18,
        K::Amper => 19,
        K::Less => 20,
        K::Greater => 21,
        K::Equal => 22,
        K::Dot => 23,
        K::Percent => 24,
        K::Lbrace => 25,
        K::Rbrace => 26,
        K::EqEqual => 27,
        K::NotEqual => 28,
        K::LessEqual => 29,
        K::GreaterEqual => 30,
        K::Tilde => 31,
        K::CircumFlex => 32,
        K::LeftShift => 33,
        K::RightShift => 34,
        K::DoubleStar => 35,
        K::PlusEqual => 36,
        K::MinusEqual => 37,
        K::StarEqual => 38,
        K::SlashEqual => 39,
        K::PercentEqual => 40,
        K::AmperEqual => 41,
        K::VbarEqual => 42,
        K::CircumflexEqual => 43,
        K::LeftShiftEqual => 44,
        K::RightShiftEqual => 45,
        K::DoubleStarEqual => 46,
        K::DoubleSlash => 47,
        K::DoubleSlashEqual => 48,
        K::At => 49,
        K::AtEqual => 50,
        K::Rarrow => 51,
        K::Ellipsis => 52,
        K::ColonEqual => 53,
        K::Exclamation => 54,
        K::Question => 55,
        K::FStringStart => 59,
        K::FStringMiddle => 60,
        K::FStringEnd => 61,
        K::TStringStart => 62,
        K::TStringMiddle => 63,
        K::TStringEnd => 64,
        K::Comment => 65,
        K::NonLogicalNewline => 66,
        _ => return None,
    };
    Some(if extra && (7..=54).contains(&ty) {
        55
    } else {
        ty
    })
}
