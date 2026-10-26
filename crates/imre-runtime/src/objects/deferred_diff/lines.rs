//! A splitlines snapshot keeps its immutable source and UTF-8 byte ranges.
//! Prefix filters, len and join can use the ranges directly. An arbitrary
//! element read or write still gets the usual independently owned list words.

use super::super::{Boundaries, LineOffsets, Object, Str, Words, exceptions, owned_line};
use super::{Deferred, View, suffix};
use crate::StrOwner;
use std::{cell::RefCell, ops::Range, rc::Rc};

pub(super) struct TextLines {
    source: Rc<LineSource>,
    rows: LineRanges,
    /// With only LF between retained ranges, joining with LF can copy once.
    continuous_lf: bool,
    materialized: Words,
}

/// Every range, including those in filtered lists, ends before the final terminator.
struct LineSource {
    text: RefCell<StrOwner>,
    end: usize,
}

impl LineSource {
    /// Shares exactly the bytes a full join returns. An alias of the original
    /// Python string must still see its terminator, so detach when one exists.
    fn full_join(&self) -> Option<StrOwner> {
        let mut text = self.text.borrow_mut();
        if self.end != text.len() {
            if let Some(unique) = StrOwner::get_mut(&mut text) {
                unique.truncate(self.end);
            } else {
                *text = StrOwner::try_new(Str::try_new(&text[..self.end]).ok()?).ok()?;
            }
        }
        Some((*text).clone())
    }
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

    fn try_reserve_one(&mut self) -> Result<(), exceptions::Error> {
        match self {
            Self::Lf(rows) => rows.try_reserve(1),
            Self::Compact(rows) => rows.try_reserve(1),
            Self::Wide(rows) => rows.try_reserve(1),
        }
        .map_err(|_| exceptions::Error::allocation())
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
    fn new(source: StrOwner) -> Result<Self, exceptions::Error> {
        let mut offsets = LineOffsets::new(&source);
        let mut continuous_lf = true;
        let rows = if u32::try_from(source.len()).is_ok()
            && matches!(offsets.boundaries, Boundaries::CrLf)
            && memchr::memchr(b'\r', source.as_bytes()).is_none()
        {
            let lf = memchr::memchr_iter(b'\n', source.as_bytes());
            let trailing_line = !source.is_empty() && !source.ends_with('\n');
            let mut ends = Vec::new();
            ends.try_reserve_exact(lf.clone().count() + usize::from(trailing_line))
                .map_err(|_| exceptions::Error::allocation())?;
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
                rows.try_reserve_one()?;
                rows.push(row);
            }
            rows
        };
        let end = rows
            .len()
            .checked_sub(1)
            .map_or(0, |last| rows.range(last).end);
        let materialized = Self::materialize(&source, rows.iter())?;
        Ok(Self {
            source: Rc::new(LineSource {
                text: RefCell::new(source),
                end,
            }),
            rows,
            continuous_lf,
            materialized,
        })
    }

    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }

    // Complete all visible owners before returning the Python list. Its later reads cannot fail.
    fn materialize(
        source: &StrOwner,
        rows: impl ExactSizeIterator<Item = Range<usize>>,
    ) -> Result<Words, exceptions::Error> {
        let mut result = Words::new(3);
        result
            .values
            .try_reserve(rows.len())
            .map_err(|_| exceptions::Error::allocation())?;
        for row in rows {
            result
                .values
                .push(StrOwner::into_raw(owned_line(source, row)?) as usize);
        }
        result.update_header();
        Ok(result)
    }

    pub(super) fn ordered(&self) -> &Words {
        &self.materialized
    }

    pub(super) fn filter(
        &self,
        first: &str,
        second: Option<&str>,
        exclude: Option<&str>,
        strip: usize,
    ) -> Result<Words, exceptions::Error> {
        let source = self.source.text.borrow();
        let mut rows = LineRanges::sparse(source.len());
        let mut materialized = Words::new(3);
        for (index, row) in self.rows.iter().enumerate() {
            let text = &source[row.clone()];
            if !(text.starts_with(first) || second.is_some_and(|s| text.starts_with(s)))
                || exclude.is_some_and(|s| text.starts_with(s))
            {
                continue;
            }
            rows.try_reserve_one()?;
            materialized
                .values
                .try_reserve(1)
                .map_err(|_| exceptions::Error::allocation())?;
            let selected = row.end - suffix(text, strip).len()..row.end;
            let owner = if selected == row {
                // SAFETY: the original ordered string remains independently rooted.
                unsafe { StrOwner::acquire_word(self.materialized.values[index]) }
            } else {
                owned_line(&source, selected.clone())?
            };
            materialized.values.push(StrOwner::into_raw(owner) as usize);
            rows.push(selected);
        }
        materialized.update_header();
        if rows.len() == 0 || (rows.len() < 8 && source.len() > 32 * 1024) {
            return Ok(materialized);
        }
        let selection = Self {
            source: self.source.clone(),
            rows,
            continuous_lf: false,
            materialized,
        };
        Ok(Words::deferred(Deferred(View::Lines(Rc::new(selection)))))
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
            return StrOwner::into_raw(super::super::text::owned_string(""));
        }
        let span = self.span(separator);
        if span == Some(0..self.source.end) {
            return self.source.full_join().map_or_else(
                || {
                    exceptions::sequence_allocation_failed();
                    std::ptr::null()
                },
                StrOwner::into_raw,
            );
        }
        let source = self.source.text.borrow();
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
            result.push_str(&source[span]);
        } else {
            let mut rows = self.rows.iter();
            result.push_str(&source[rows.next().unwrap()]);
            for row in rows {
                result.push_str(separator);
                result.push_str(&source[row]);
            }
        }
        StrOwner::try_new(result).map_or_else(
            |_| {
                exceptions::sequence_allocation_failed();
                std::ptr::null()
            },
            StrOwner::into_raw,
        )
    }
}

impl Deferred {
    /// Tiny strings already fit the ordinary inline sequence; avoid a range table.
    pub(in super::super) fn split_lines(
        source: &StrOwner,
    ) -> Result<Option<*const Object>, exceptions::Error> {
        // Dense short lines have little data to share. Avoid an additional range table
        // on those inputs; the ordinary eager path already checks each owner allocation.
        if source.len() < 1024
            || memchr::memchr_iter(b'\n', &source.as_bytes()[..1024])
                .take(65)
                .count()
                > 64
        {
            return Ok(None);
        }
        {
            let lines = TextLines::new(source.clone())?;
            let words = if lines.len() <= 3 {
                lines.materialized
            } else {
                Words::deferred(Self(View::Lines(Rc::new(lines))))
            };
            Ok(Some(crate::ObjectOwner::into_raw(words.into_object())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join(lines: &TextLines, separator: &str) -> StrOwner {
        // SAFETY: the small test inputs fit in memory; the join transfers one owner.
        unsafe { StrOwner::from_raw(lines.joined(separator)) }
    }

    #[test]
    fn full_join_reuses_the_owned_text_through_a_trailing_line_break() {
        for ending in ["\n", "\r\n", "\u{2029}"] {
            let text = StrOwner::new(Str::from(format!("one\n\n🦀{ending}")));
            let pointer = StrOwner::as_ptr(&text);
            let lines = TextLines::new(text)
                .unwrap_or_else(|_| panic!("small line fixture should fit in memory"));
            let joined = join(&lines, "\n");
            assert_eq!(StrOwner::as_ptr(&joined), pointer);
            assert_eq!(joined.as_str(), "one\n\n🦀");
            assert_eq!(join(&lines, "|").as_str(), "one||🦀");
        }
    }

    #[test]
    fn full_join_keeps_string_aliases_and_preexisting_filtered_ranges_valid() {
        let text = StrOwner::new(Str::from("+one\n-two\n+three\n"));
        let lines = TextLines::new(text.clone())
            .unwrap_or_else(|_| panic!("small line fixture should fit in memory"));
        let selected = lines
            .filter("+", None, None, 1)
            .unwrap_or_else(|_| panic!("small line fixture should fit in memory"));
        let selected = selected.plan().expect("deferred selection");
        let View::Lines(selected) = &selected.0 else {
            unreachable!()
        };
        let joined = join(&lines, "\n");
        assert_eq!(text.as_str(), "+one\n-two\n+three\n");
        assert_eq!(joined.as_str(), "+one\n-two\n+three");
        assert_eq!(join(selected, "|").as_str(), "one|three");
        assert!(!StrOwner::ptr_eq(&text, &joined));
        assert!(StrOwner::ptr_eq(&join(&lines, "\n"), &joined));
    }

    #[test]
    fn lf_ranges_keep_blank_lines_and_omit_only_the_final_terminator() {
        for (source, expected) in [
            ("\n\n🦀\n", vec![0..0, 1..1, 2..6]),
            ("\n\n🦀", vec![0..0, 1..1, 2..6]),
            ("🦀\n\n", vec![0..4, 5..5]),
        ] {
            let lines = TextLines::new(StrOwner::new(Str::new(source)))
                .unwrap_or_else(|_| panic!("small line fixture should fit in memory"));
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
