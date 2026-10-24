//! A diff list can share immutable snapshots across independent shallow copies.
//! Ordinary reads retain the exact ndiff order and guides. Prefix filters and a
//! stable suffix sort can instead select just the rows they observe.

use super::difflib::{Output, materialize_words};
use super::{Object, Str, Words, object};
use std::cell::OnceCell;
use std::rc::Rc;

#[cfg(test)]
mod tests;

struct Snapshots {
    left: Words,
    right: Words,
    full: OnceCell<Words>,
    signed: OnceCell<Words>,
    projected: OnceCell<Words>,
}

impl Snapshots {
    fn full(&self) -> &Words {
        self.full.get_or_init(|| {
            // SAFETY: both snapshots own exact immutable string handles.
            unsafe { materialize_words(&self.left, &self.right, Output::Full) }
        })
    }

    fn signed(&self) -> &Words {
        self.signed
            .get_or_init(|| filter(self.full(), "+", Some("-"), None, 0, None))
    }

    fn projected(&self) -> &Words {
        if self.full.get().is_some() {
            return self.signed();
        }
        self.projected.get_or_init(|| {
            // SAFETY: the snapshots own every string until the result is complete.
            unsafe { materialize_words(&self.left, &self.right, Output::ProjectedChanges) }
        })
    }
}

#[derive(Clone)]
pub(super) struct Deferred {
    source: Rc<Snapshots>,
    signed_only: bool,
}

impl Deferred {
    pub(super) fn ordered(&self) -> &Words {
        if self.signed_only {
            self.source.signed()
        } else {
            self.source.full()
        }
    }

    fn nonempty(&self) -> bool {
        if self.signed_only {
            !self.source.projected().values.is_empty()
        } else {
            !self.source.left.values.is_empty() || !self.source.right.values.is_empty()
        }
    }
}

/// Slices at a Unicode scalar index. The diff's two ASCII tag bytes do not
/// require scanning the diagnostic that follows them.
fn suffix(text: &str, skip: usize) -> &str {
    if text.as_bytes().get(..skip).is_some_and(<[u8]>::is_ascii) {
        &text[skip..]
    } else {
        text.char_indices()
            .nth(skip)
            .map_or("", |(offset, _)| &text[offset..])
    }
}

fn filter(
    values: &Words,
    first: &str,
    second: Option<&str>,
    exclude: Option<&str>,
    strip: usize,
    originals: Option<&Words>,
) -> Words {
    let mut result = Words::new(3);
    let mut originals = originals.map(Words::strings);
    for (word, text) in values.strings() {
        if !(text.starts_with(first) || second.is_some_and(|s| text.starts_with(s)))
            || exclude.is_some_and(|s| text.starts_with(s))
        {
            continue;
        }
        let selected = suffix(text, strip);
        let retained = if let Some(originals) = &mut originals {
            // Removing an ndiff tag recovers a complete original string. Every
            // single-sign subsequence retains source order, even with duplicates.
            Some(
                originals
                    .find(|(_, text)| text.as_str() == selected)
                    .expect("ndiff side subsequence")
                    .0,
            )
        } else {
            (selected.len() == text.len()).then_some(word)
        };
        let element = if let Some(word) = retained {
            // SAFETY: retain the source string before adding it to the independent list.
            unsafe { values.kind.acquire(word) }.into_word()
        } else {
            Rc::into_raw(Rc::new(Str::new(selected))) as usize
        };
        result.values.push(element);
    }
    result.update_header();
    result
}

/// Share equal immutable payloads across snapshots without changing either
/// sequence. A merge finds every common sorted run; on unsorted input it can
/// miss sharing opportunities, but it still substitutes only equal strings.
fn snapshot_right(left: &Words, right: &Words) -> Words {
    let mut result = Words::new(3);
    result.values.reserve(right.values.len());
    let mut candidates = left.strings().peekable();
    for (word, text) in right.strings() {
        while candidates
            .next_if(|(_, candidate)| *candidate < text)
            .is_some()
        {}
        let word = candidates
            .next_if(|(_, candidate)| *candidate == text)
            .map_or(word, |(word, _)| word);
        // SAFETY: both snapshots own every borrowed word throughout the merge.
        result
            .values
            .push(unsafe { right.kind.acquire(word) }.into_word());
    }
    result.update_header();
    result
}

/// # Safety
/// The caller roots two lists of nonnullable strings. Snapshots retain their
/// current elements, so subsequent mutations cannot change this diff.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_ndiff_deferred(
    left: *const Object,
    right: *const Object,
) -> *const Object {
    // SAFETY: acquiring the elements cannot invoke callbacks or alias a mutable borrow.
    let left = unsafe {
        object(left)
            .0
            .with(|storage| storage.sequence().copy_strings())
    };
    // SAFETY: sharing equal strings acquires independent references on both sides.
    let right = unsafe {
        object(right)
            .0
            .with(|storage| snapshot_right(&left, storage.sequence()))
    };
    let source = Rc::new(Snapshots {
        left,
        right,
        full: OnceCell::new(),
        signed: OnceCell::new(),
        projected: OnceCell::new(),
    });
    Rc::into_raw(
        Words::deferred(Deferred {
            source,
            signed_only: false,
        })
        .into_object(),
    )
}

/// # Safety
/// The caller roots a list of exact strings. The result transfers one distinct list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_list_copy(list: *const Object) -> *const Object {
    // SAFETY: reading a plan or retaining string elements cannot invoke user code.
    let words = unsafe {
        object(list).0.with(|storage| {
            let words = storage.sequence_storage();
            words.plan().map_or_else(
                || words.copy_strings(),
                |plan| Words::deferred(plan.clone()),
            )
        })
    };
    Rc::into_raw(words.into_object())
}

fn same_sign(first: &str, second: Option<&str>) -> bool {
    let sign = first.as_bytes().first();
    matches!(sign, Some(b'+') | Some(b'-')) && second.is_none_or(|s| s.as_bytes().first() == sign)
}

fn both_signs(first: &str, second: Option<&str>) -> bool {
    matches!(
        (first, second),
        ("+" | "+ ", Some("-" | "- ")) | ("-" | "- ", Some("+" | "+ "))
    )
}

/// Filters a list of strings by one or two prefixes, an optional excluded
/// prefix, and a nonnegative slice start. Keeps per-side source order.
///
/// # Safety
/// The list and first prefix are rooted. Optional prefixes are null or rooted strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_list_filter(
    list: *const Object,
    first: *const Str,
    second: *const Str,
    exclude: *const Str,
    strip: u32,
) -> *const Object {
    // SAFETY: all live input strings are immutable and no user code runs while borrowed.
    let result = unsafe {
        let (first, second, exclude) = (
            (&*first).as_str(),
            second.as_ref().map(Str::as_str),
            exclude.as_ref().map(Str::as_str),
        );
        object(list).0.with(|storage| {
            let words = storage.sequence_storage();
            if let Some(plan) = words.plan() {
                if strip == 0 && exclude.is_none() && both_signs(first, second) {
                    return Words::deferred(Deferred {
                        source: Rc::clone(&plan.source),
                        signed_only: true,
                    });
                }
                if same_sign(first, second) {
                    let originals = (strip == 2).then_some(if first.starts_with('+') {
                        &plan.source.right
                    } else {
                        &plan.source.left
                    });
                    return filter(
                        plan.source.projected(),
                        first,
                        second,
                        exclude,
                        strip as usize,
                        originals,
                    );
                }
            }
            filter(
                words.resolved(),
                first,
                second,
                exclude,
                strip as usize,
                None,
            )
        })
    };
    Rc::into_raw(result.into_object())
}

/// Stable ordering by the suffix starting at a fixed nonnegative scalar index.
///
/// # Safety
/// The caller roots a list of exact strings, with no callback or concurrent mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_list_sorted_suffix(
    list: *const Object,
    skip: u32,
) -> *const Object {
    // SAFETY: every copied word independently owns its chosen string throughout sorting.
    let result = unsafe {
        object(list).0.with(|storage| {
            let words = storage.sequence_storage();
            let input = match words.plan() {
                Some(plan) if plan.signed_only && skip == 2 => plan.source.projected(),
                _ => words.resolved(),
            };
            let mut sorted = input.copy_strings();
            let key = |word: usize| suffix(&*(word as *const Str), skip as usize);
            sorted.values.sort_by(|&a, &b| key(a).cmp(key(b)));
            sorted
        })
    };
    Rc::into_raw(result.into_object())
}

/// # Safety
/// The caller roots an exact string list. Diff inputs yield at least one row
/// if either source is nonempty, without requiring any intraline matching.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_list_nonempty(list: *const Object) -> u32 {
    // SAFETY: shared input storage remains rooted for this entire check.
    unsafe {
        object(list).0.with(|storage| {
            let words = storage.sequence_storage();
            u32::from(
                words
                    .plan()
                    .map_or_else(|| !words.values.is_empty(), Deferred::nonempty),
            )
        })
    }
}

/// Resolves the sentinel-length entry of the compiler's sequence layout.
///
/// # Safety
/// The caller roots an exact sequence. The returned inline bit is clear for a
/// deferred list because its backing words live in its separately owned plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_resolve_header(list: *const Object) -> u32 {
    // SAFETY: initialization uses only independently owned immutable snapshots.
    unsafe {
        object(list)
            .0
            .with(|storage| storage.sequence_storage().resolved_header())
    }
}
