//! A splitlines snapshot keeps its immutable source and UTF-8 byte ranges.
//! Prefix filters, len and join can use the ranges directly. An arbitrary
//! element read or write still gets the usual independently owned list words.

use super::super::{LineOffsets, Object, Str, Words, exceptions, owned_line};
use super::{Deferred, View, suffix};
use std::{cell::OnceCell, ops::Range, rc::Rc};

pub(super) struct TextLines {
    source: Rc<Str>,
    rows: Vec<Range<usize>>,
    /// With only LF between retained ranges, joining with LF can copy once.
    continuous_lf: bool,
    materialized: OnceCell<Words>,
}

impl TextLines {
    fn new(source: Rc<Str>) -> Result<Self, exceptions::Error> {
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
        Ok(Self {
            source,
            rows,
            continuous_lf,
            materialized: OnceCell::new(),
        })
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
                    .map(|row| Rc::into_raw(owned_line(&self.source, row.clone())) as usize),
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
        let mut rows = Vec::new();
        for row in &self.rows {
            let text = &self.source[row.clone()];
            if (text.starts_with(first) || second.is_some_and(|s| text.starts_with(s)))
                && !exclude.is_some_and(|s| text.starts_with(s))
            {
                rows.push(row.end - suffix(text, strip).len()..row.end);
            }
        }
        if rows.is_empty() {
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

    pub(super) fn joined(&self, separator: &str) -> *const Str {
        let Some(first) = self.rows.first() else {
            return Rc::into_raw(Rc::new(Str::default()));
        };
        let last = self.rows.last().unwrap();
        let contiguous = self.rows.len() == 1 || (separator == "\n" && self.continuous_lf);
        if contiguous && first.start == 0 && last.end == self.source.len() {
            return Rc::into_raw(Rc::clone(&self.source));
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
        Rc::into_raw(Rc::new(result))
    }
}

impl Deferred {
    /// Tiny strings already fit the ordinary inline sequence; avoid a range table.
    pub(in super::super) fn split_lines(
        source: &Rc<Str>,
    ) -> Result<Option<*const Object>, exceptions::Error> {
        if source.len() < 1024 {
            return Ok(None);
        }
        {
            let lines = TextLines::new(Rc::clone(source))?;
            let words = if lines.len() <= 3 {
                lines.ordered();
                lines.materialized.into_inner().unwrap()
            } else {
                Words::deferred(Self(View::Lines(Rc::new(lines))))
            };
            Ok(Some(Rc::into_raw(words.into_object())))
        }
    }
}
