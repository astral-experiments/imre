use super::{Error, Object, OwnedWord, Storage, StorageCell, Str, StrOwner, owned_split_field};
use std::ops::Range;

/// The first unconsumed field and final byte offset of an ascending run.
struct FieldRun {
    front: Range<usize>,
    stop: usize,
}

enum FieldOrder {
    Runs(Vec<FieldRun>),
    Offsets(std::vec::IntoIter<Range<usize>>),
}

pub(super) struct SortedSplitFields {
    source: StrOwner,
    separator: u8,
    order: FieldOrder,
}

fn field_ranges(text: &[u8], separator: u8) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut start = 0;
    memchr::memchr_iter(separator, text)
        .chain(std::iter::once(text.len()))
        .map(move |end| {
            let field = start..end;
            start = end + 1;
            field
        })
}

impl FieldOrder {
    /// Merge a small number of ascending runs as their fields are consumed.
    /// For fragmented inputs, sort offsets once and break ties by input position.
    fn new(text: &[u8], separator: u8) -> Result<Self, Error> {
        const MAX_RUNS: usize = 8;
        let mut ranges = field_ranges(text, separator);
        let mut first = ranges.next().expect("a split always has a field");
        let mut previous = first.clone();
        let mut runs = Vec::new();
        for field in ranges {
            if text[previous.clone()] > text[field.clone()] {
                if runs.len() + 1 == MAX_RUNS {
                    let mut offsets = Vec::new();
                    for range in field_ranges(text, separator) {
                        offsets.try_reserve(1).map_err(|_| Error::memory_error())?;
                        offsets.push(range);
                    }
                    offsets.sort_unstable_by(|a, b| {
                        text[a.clone()]
                            .cmp(&text[b.clone()])
                            .then_with(|| a.start.cmp(&b.start))
                    });
                    return Ok(Self::Offsets(offsets.into_iter()));
                }
                runs.try_reserve(1).map_err(|_| Error::memory_error())?;
                runs.push(FieldRun {
                    front: first,
                    stop: previous.end,
                });
                first = field.clone();
            }
            previous = field;
        }
        runs.try_reserve(1).map_err(|_| Error::memory_error())?;
        runs.push(FieldRun {
            front: first,
            stop: previous.end,
        });
        Ok(Self::Runs(runs))
    }

    fn next(&mut self, text: &[u8], separator: u8) -> Option<Range<usize>> {
        let runs = match self {
            Self::Runs(runs) => runs,
            Self::Offsets(offsets) => return offsets.next(),
        };
        let at = match runs.len() {
            0 => return None,
            1 => 0,
            len => (0..len)
                .min_by(|&a, &b| {
                    let a = &runs[a].front;
                    let b = &runs[b].front;
                    text[a.clone()]
                        .cmp(&text[b.clone()])
                        .then_with(|| a.start.cmp(&b.start))
                })
                .unwrap(),
        };
        let run = &mut runs[at];
        let field = run.front.clone();
        if field.end == run.stop {
            runs.swap_remove(at);
        } else {
            let start = field.end + 1;
            let end = memchr::memchr(separator, &text[start..run.stop])
                .map_or(run.stop, |offset| start + offset);
            run.front = start..end;
        }
        Some(field)
    }
}

impl SortedSplitFields {
    pub(super) fn next(&mut self) -> Result<Option<(OwnedWord, Option<OwnedWord>)>, Error> {
        let Some(range) = self.order.next(self.source.as_bytes(), self.separator) else {
            return Ok(None);
        };
        let word = owned_split_field(&self.source, range)?;
        Ok(Some((word, None)))
    }
}

/// Iterates the stable lexical order of fields in a rooted immutable split source.
///
/// # Safety
/// The caller roots the string for this call and supplies an ASCII separator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_sorted_split_byte_iter(
    value: *const Str,
    separator: u8,
) -> *const Object {
    // SAFETY: the string remains rooted until the cursor acquires its reference.
    let source = unsafe { StrOwner::acquire_word(value as usize) };
    crate::finish(
        FieldOrder::new(source.as_bytes(), separator).map(|order| {
            let cursor = SortedSplitFields {
                source,
                separator,
                order,
            };
            crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
                Storage::SortedSplitFields(Box::new(cursor)),
            ))))
        }),
        std::ptr::null(),
    )
}

#[cfg(test)]
mod tests {
    use super::{FieldOrder, field_ranges};

    #[test]
    fn offsets_match_a_stable_sort_for_ordered_and_fragmented_fields() {
        for text in [
            "",
            "|",
            "a||b|",
            "β|a|β|🐍|zz|a",
            "z|a|y|b|x|c|w|d|v|e|u|f|t|g|s|h|r|i|a||a",
        ] {
            let bytes = text.as_bytes();
            let mut expected: Vec<_> = field_ranges(bytes, b'|').collect();
            expected.sort_by(|a, b| bytes[a.clone()].cmp(&bytes[b.clone()]));
            let mut order = FieldOrder::new(bytes, b'|')
                .unwrap_or_else(|_| panic!("the test offsets fit in memory"));
            let actual: Vec<_> = std::iter::from_fn(|| order.next(bytes, b'|')).collect();
            assert_eq!(actual, expected);
        }
    }
}
