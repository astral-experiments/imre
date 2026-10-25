//! A splitlines snapshot keeps its immutable source and UTF-8 byte ranges.
//! Prefix filters, len and join can use the ranges directly. An arbitrary
//! element read or write still gets the usual independently owned list words.

use super::super::{LineOffsets, Object, Str, Words, exceptions, owned_line};
use super::{Deferred, View, suffix};
use crate::StrOwner;
use std::{ops::Range, rc::Rc};

pub(super) struct TextLines {
    source: StrOwner,
    rows: Vec<Range<usize>>,
    /// With only LF between retained ranges, joining with LF can copy once.
    continuous_lf: bool,
    materialized: Words,
}

impl TextLines {
    fn new(source: StrOwner) -> Result<Self, exceptions::Error> {
        let mut offsets = LineOffsets::new(&source);
        let mut rows = Vec::new();
        let mut continuous_lf = true;
        let mut end = 0;
        while let Some(row) = offsets.next(&source) {
            if !rows.is_empty() {
                continuous_lf &= source.get(end..row.start) == Some("\n");
            }
            end = row.end;
            rows.try_reserve(1)
                .map_err(|_| exceptions::Error::allocation())?;
            rows.push(row);
        }
        let materialized = Self::materialize(&source, rows.iter().cloned())?;
        Ok(Self {
            source,
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
        let mut rows = Vec::new();
        let mut materialized = Words::new(3);
        for (index, row) in self.rows.iter().enumerate() {
            let text = &self.source[row.clone()];
            if !(text.starts_with(first) || second.is_some_and(|s| text.starts_with(s)))
                || exclude.is_some_and(|s| text.starts_with(s))
            {
                continue;
            }
            rows.try_reserve(1)
                .map_err(|_| exceptions::Error::allocation())?;
            materialized
                .values
                .try_reserve(1)
                .map_err(|_| exceptions::Error::allocation())?;
            let selected = row.end - suffix(text, strip).len()..row.end;
            let owner = if selected == *row {
                // SAFETY: the original ordered string remains independently rooted.
                unsafe { StrOwner::acquire_word(self.materialized.values[index]) }
            } else {
                owned_line(&self.source, selected.clone())?
            };
            materialized.values.push(StrOwner::into_raw(owner) as usize);
            rows.push(selected);
        }
        materialized.update_header();
        if rows.is_empty() || (rows.len() < 8 && self.source.len() > 32 * 1024) {
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

    pub(super) fn joined(&self, separator: &str) -> *const Str {
        let Some(first) = self.rows.first() else {
            return StrOwner::into_raw(super::super::text::owned_string(""));
        };
        let last = self.rows.last().unwrap();
        let contiguous = self.rows.len() == 1 || (separator == "\n" && self.continuous_lf);
        if contiguous && first.start == 0 && last.end == self.source.len() {
            return StrOwner::into_raw(self.source.clone());
        }
        let size = if contiguous {
            Some(last.end - first.start)
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
        if contiguous {
            result.push_str(&self.source[first.start..last.end]);
        } else {
            result.push_str(&self.source[first.clone()]);
            for row in &self.rows[1..] {
                result.push_str(separator);
                result.push_str(&self.source[row.clone()]);
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
