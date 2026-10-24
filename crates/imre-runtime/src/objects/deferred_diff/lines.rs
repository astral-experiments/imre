//! A splitlines snapshot keeps its immutable source and UTF-8 byte ranges.
//! Prefix filters, len and join can use the ranges directly. An arbitrary
//! element read or write still gets the usual independently owned list words.

use super::super::{Boundaries, LineOffsets, Object, Str, Words, exceptions, owned_line};
use super::{Deferred, View, suffix};
use std::{cell::OnceCell, ops::Range, rc::Rc};

pub(super) struct TextLines {
    source: Rc<Str>,
    rows: LineRanges,
    /// With only LF between retained ranges, joining with LF can copy once.
    continuous_lf: bool,
    materialized: OnceCell<Words>,
}

/// LF-only input needs one end offset per line. Filters and mixed line endings
/// keep both bounds; only strings over 4 GiB need machine-sized offsets.
enum LineRanges {
    Lf(Vec<u32>),
    Compact(Vec<Range<u32>>),
    Wide(Vec<Range<usize>>),
}

impl LineRanges {
    fn sparse(source_len: usize) -> Self {
        if u32::try_from(source_len).is_ok() {
            Self::Compact(Vec::new())
        } else {
            Self::Wide(Vec::new())
        }
    }

    fn push(&mut self, row: Range<usize>) {
        match self {
            Self::Lf(_) => unreachable!("LF offsets are collected directly"),
            Self::Compact(rows) => rows.push(row.start as u32..row.end as u32),
            Self::Wide(rows) => rows.push(row),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Lf(ends) => ends.len(),
            Self::Compact(rows) => rows.len(),
            Self::Wide(rows) => rows.len(),
        }
    }

    fn range(&self, index: usize) -> Range<usize> {
        match self {
            Self::Lf(ends) => {
                let start = if index == 0 {
                    0
                } else {
                    ends[index - 1] as usize + 1
                };
                start..ends[index] as usize
            }
            Self::Compact(rows) => rows[index].start as usize..rows[index].end as usize,
            Self::Wide(rows) => rows[index].clone(),
        }
    }

    fn iter(&self) -> impl ExactSizeIterator<Item = Range<usize>> {
        (0..self.len()).map(|i| self.range(i))
    }
}

impl TextLines {
    fn new(source: Rc<Str>) -> Self {
        let mut offsets = LineOffsets::new(&source);
        let mut continuous_lf = true;
        let rows = if u32::try_from(source.len()).is_ok()
            && matches!(offsets.boundaries, Boundaries::CrLf)
            && memchr::memchr(b'\r', source.as_bytes()).is_none()
        {
            let lf = memchr::memchr_iter(b'\n', source.as_bytes());
            let trailing_line = !source.is_empty() && !source.ends_with('\n');
            let mut ends = Vec::with_capacity(lf.clone().count() + usize::from(trailing_line));
            ends.extend(lf.map(|end| end as u32));
            if trailing_line {
                ends.push(source.len() as u32);
            }
            LineRanges::Lf(ends)
        } else {
            let mut rows = LineRanges::sparse(source.len());
            let mut end = 0;
            while let Some(row) = offsets.next(&source) {
                if rows.len() != 0 {
                    continuous_lf &= source.get(end..row.start) == Some("\n");
                }
                end = row.end;
                rows.push(row);
            }
            rows
        };
        Self {
            source,
            rows,
            continuous_lf,
            materialized: OnceCell::new(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(super) fn ordered(&self) -> &Words {
        self.materialized.get_or_init(|| {
            let mut result = Words::new(3);
            result.values.extend(
                self.rows
                    .iter()
                    .map(|row| Rc::into_raw(owned_line(&self.source, row)) as usize),
            );
            result.update_header();
            result
        })
    }

    pub(super) fn filter(
        &self,
        first: &str,
        second: Option<&str>,
        exclude: Option<&str>,
        strip: usize,
    ) -> Words {
        let mut rows = LineRanges::sparse(self.source.len());
        for row in self.rows.iter() {
            let text = &self.source[row.clone()];
            if (text.starts_with(first) || second.is_some_and(|s| text.starts_with(s)))
                && !exclude.is_some_and(|s| text.starts_with(s))
            {
                rows.push(row.end - suffix(text, strip).len()..row.end);
            }
        }
        if rows.len() == 0 {
            return Words::new(3);
        }
        let selection = Self {
            source: Rc::clone(&self.source),
            rows,
            continuous_lf: false,
            materialized: OnceCell::new(),
        };
        // A few short results should not keep a large discarded input alive.
        if selection.rows.len() < 8 && self.source.len() > 32 * 1024 {
            selection.ordered();
            selection.materialized.into_inner().unwrap()
        } else {
            Words::deferred(Deferred(View::Lines(Rc::new(selection))))
        }
    }

    /// Returns the source bytes that a join would reproduce without change.
    fn span(&self, separator: &str) -> Option<Range<usize>> {
        let len = self.len();
        if len != 0 && (len == 1 || (separator == "\n" && self.continuous_lf)) {
            Some(self.rows.range(0).start..self.rows.range(len - 1).end)
        } else {
            None
        }
    }

    pub(super) fn joined(&self, separator: &str) -> *const Str {
        if self.len() == 0 {
            return Rc::into_raw(Rc::new(Str::default()));
        }
        let span = self.span(separator);
        if span == Some(0..self.source.len()) {
            return Rc::into_raw(Rc::clone(&self.source));
        }
        let size = if let Some(span) = &span {
            Some(span.len())
        } else {
            separator
                .len()
                .checked_mul(self.rows.len() - 1)
                .and_then(|n| {
                    self.rows
                        .iter()
                        .try_fold(n, |n, row| n.checked_add(row.len()))
                })
        };
        let Some(mut result) = size.and_then(|n| Str::try_with_capacity(n).ok()) else {
            exceptions::sequence_allocation_failed();
            return std::ptr::null();
        };
        if let Some(span) = span {
            result.push_str(&self.source[span]);
        } else {
            let mut rows = self.rows.iter();
            result.push_str(&self.source[rows.next().unwrap()]);
            for row in rows {
                result.push_str(separator);
                result.push_str(&self.source[row]);
            }
        }
        Rc::into_raw(Rc::new(result))
    }
}

impl Deferred {
    /// Tiny strings already fit the ordinary inline sequence; avoid a range table.
    pub(in super::super) fn split_lines(source: &Rc<Str>) -> Option<*const Object> {
        (source.len() >= 1024).then(|| {
            let lines = TextLines::new(Rc::clone(source));
            let words = if lines.len() <= 3 {
                lines.ordered();
                lines.materialized.into_inner().unwrap()
            } else {
                Words::deferred(Self(View::Lines(Rc::new(lines))))
            };
            Rc::into_raw(words.into_object())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lf_ranges_keep_blank_lines_and_omit_only_the_final_terminator() {
        for (source, expected) in [
            ("\n\n🦀\n", vec![0..0, 1..1, 2..6]),
            ("\n\n🦀", vec![0..0, 1..1, 2..6]),
            ("🦀\n\n", vec![0..4, 5..5]),
        ] {
            let lines = TextLines::new(Rc::new(Str::new(source)));
            assert!(matches!(lines.rows, LineRanges::Lf(_)));
            assert_eq!(lines.rows.iter().collect::<Vec<_>>(), expected);
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn offsets_above_four_gib_are_never_truncated() {
        let start = u32::MAX as usize + 7;
        let mut rows = LineRanges::sparse(start + 8);
        rows.push(start..start + 8);
        assert_eq!(rows.range(0), start..start + 8);
    }
}
