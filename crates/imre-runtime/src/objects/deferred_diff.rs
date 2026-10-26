//! A diff list can share immutable snapshots across independent shallow copies.
//! Ordinary reads retain the exact ndiff order and guides. Prefix filters and a
//! stable suffix sort can instead select just the rows they observe.

use super::difflib::{Output, materialize_words};
use super::exceptions::Error;
use super::{Object, Str, Words, object};
use crate::{ObjectOwner, StrOwner, shared::{Shared, Weak}};
use std::cell::{OnceCell, Ref, RefCell};
use std::rc::Rc;

mod lines;
mod packed;

#[cfg(test)]
mod tests;

struct Snapshots {
    inputs: RefCell<Inputs>,
    nonempty: bool,
    full: OnceCell<Words>,
    signed: OnceCell<Words>,
    projected: OnceCell<Words>,
}

enum Inputs {
    Original(Words, Words),
    Packed(packed::Packed),
    Materialized,
}

fn cached<T>(cell: &OnceCell<T>, build: impl FnOnce() -> Result<T, Error>) -> Result<&T, Error> {
    if let Some(value) = cell.get() {
        return Ok(value);
    }
    let value = build()?;
    Ok(cell.get_or_init(|| value))
}

impl Snapshots {
    fn side(&self, prefix: &str) -> Option<Ref<'_, Words>> {
        Ref::filter_map(self.inputs.borrow(), |inputs| match inputs {
            Inputs::Original(left, right) => {
                Some(if prefix.starts_with('+') { right } else { left })
            }
            Inputs::Packed(_) | Inputs::Materialized => None,
        })
        .ok()
    }

    fn full(&self) -> Result<&Words, Error> {
        cached(&self.full, || {
            let mut inputs = self.inputs.borrow_mut();
            match &*inputs {
                Inputs::Original(left, right) => {
                    // SAFETY: both snapshots own exact immutable strings.
                    unsafe { materialize_words(left, right, Output::Full) }
                }
                Inputs::Packed(snapshot) => {
                    let (left, right) = snapshot.unpack()?;
                    // SAFETY: restoring the archive recovers the original sequences.
                    let rows = unsafe { materialize_words(&left, &right, Output::Full) }?;
                    *inputs = Inputs::Materialized;
                    Ok(rows)
                }
                Inputs::Materialized => unreachable!("the full result is already cached"),
            }
        })
    }

    fn signed(&self) -> Result<&Words, Error> {
        cached(&self.signed, || {
            filter(self.full()?, "+", Some("-"), None, 0, None)
        })
    }

    fn projected(&self) -> Result<&Words, Error> {
        if self.full.get().is_some() {
            return self.signed();
        }
        cached(&self.projected, || {
            let mut inputs = self.inputs.borrow_mut();
            let Inputs::Original(left, right) = &*inputs else {
                unreachable!("compaction follows the first projection")
            };
            // SAFETY: the snapshots own every string until the result is complete.
            let rows = unsafe { materialize_words(left, right, Output::ProjectedChanges) }?;
            let len = left.values.len().max(right.values.len());
            // Sparse projections can outlive millions of unchanged strings. Sorted
            // inputs keep equal rows adjacent, including repeated shared handles.
            if len >= 1024
                && rows.values.len() <= len / 4
                && left.strings_ascending()
                && right.strings_ascending()
            {
                *inputs = Inputs::Packed(packed::Packed::new(left, right)?);
            }
            Ok(rows)
        })
    }
}

#[derive(Clone)]
pub(super) struct Deferred(View);

#[derive(Clone)]
enum View {
    Lines(Rc<lines::TextLines>),
    Full(Shared<Snapshots>),
    Signed(Shared<Snapshots>),
    /// Owns the sorted rows. An independent result must not pin input strings
    /// that a caller has discarded just in case it later slices off the tags.
    Sorted {
        rows: Shared<Words>,
        origin: Weak<Snapshots>,
    },
}

impl Deferred {
    pub(super) fn ordered(&self) -> Result<&Words, Error> {
        match &self.0 {
            View::Lines(lines) => Ok(lines.ordered()),
            View::Full(source) => source.full(),
            View::Signed(source) => source.signed(),
            View::Sorted { rows, .. } => Ok(rows),
        }
    }

    pub(super) fn len(&self) -> Result<usize, Error> {
        match &self.0 {
            View::Lines(lines) => Ok(lines.len()),
            _ => Ok(self.ordered()?.values.len()),
        }
    }

    pub(super) fn join_ranges(&self, separator: &str) -> Option<*const Str> {
        if let View::Lines(lines) = &self.0 {
            Some(lines.joined(separator))
        } else {
            None
        }
    }

    fn nonempty(&self) -> Result<bool, Error> {
        Ok(match &self.0 {
            View::Lines(lines) => lines.len() != 0,
            View::Full(source) => source.nonempty,
            View::Signed(source) => !source.projected()?.values.is_empty(),
            View::Sorted { rows, .. } => !rows.values.is_empty(),
        })
    }

    fn signed(&self) -> Self {
        match &self.0 {
            View::Full(source) => Self(View::Signed(Shared::clone(source))),
            View::Signed(_) | View::Sorted { .. } => self.clone(),
            View::Lines(_) => unreachable!("text ranges filter without ndiff projection"),
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
) -> Result<Words, Error> {
    let mut result = Words::new(3);
    let mut originals = originals.map(Words::strings);
    for (word, text) in values.strings() {
        if !(text.starts_with(first) || second.is_some_and(|s| text.starts_with(s)))
            || exclude.is_some_and(|s| text.starts_with(s))
        {
            continue;
        }
        result
            .values
            .try_reserve(1)
            .map_err(|_| Error::memory_error())?;
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
            StrOwner::into_raw(StrOwner::try_new(
                Str::try_new(selected).map_err(|_| Error::memory_error())?,
            ).map_err(|_| Error::memory_error())?) as usize
        };
        result.values.push(element);
    }
    result.update_header();
    Ok(result)
}

fn own_list(words: Words) -> Result<*const Object, Error> {
    ObjectOwner::try_new(Object(super::StorageCell::new(words.into_storage())))
        .map(ObjectOwner::into_raw)
        .map_err(|_| Error::memory_error())
}

/// A merge can reuse equal strings from the first snapshot when copying the second.
fn snapshot<'a>(
    words: &Words,
    candidates: impl Iterator<Item = (usize, &'a Str)>,
) -> Result<Words, Error> {
    let mut result = Words::new(3);
    result
        .values
        .try_reserve(words.values.len())
        .map_err(|_| Error::memory_error())?;
    let mut candidates = candidates.peekable();
    for (word, text) in words.strings() {
        while candidates
            .next_if(|(_, candidate)| *candidate < text)
            .is_some()
        {}
        let word = candidates
            .next_if(|(_, candidate)| *candidate == text)
            .map_or(word, |(word, _)| word);
        // SAFETY: the source and candidate buffers root every borrowed string.
        result
            .values
            .push(unsafe { words.kind.acquire(word) }.into_word());
    }
    result.update_header();
    Ok(result)
}

/// # Safety
/// The caller roots two lists of nonnullable strings. Snapshots retain their
/// current elements, so subsequent mutations cannot change this diff.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_ndiff_deferred(
    left: *const Object,
    right: *const Object,
) -> *const Object {
    let result = (|| {
        // SAFETY: acquiring the elements cannot invoke callbacks or alias a mutable borrow.
        let left = unsafe {
            object(left)
                .0
                .with(|storage| snapshot(storage.sequence()?, std::iter::empty()))
        }?;
        // SAFETY: sharing equal strings acquires independent references on both sides.
        let right = unsafe {
            object(right)
                .0
                .with(|storage| snapshot(storage.sequence()?, left.strings()))
        }?;
        let source = Shared::try_new(Snapshots {
            nonempty: !left.values.is_empty() || !right.values.is_empty(),
            inputs: RefCell::new(Inputs::Original(left, right)),
            full: OnceCell::new(),
            signed: OnceCell::new(),
            projected: OnceCell::new(),
        }).map_err(|_| Error::memory_error())?;
        own_list(Words::deferred(Deferred(View::Full(source)))?)
    })();
    crate::finish(result, std::ptr::null())
}

/// # Safety
/// The caller roots a list of exact strings. The result transfers one distinct list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_list_copy(list: *const Object) -> *const Object {
    // SAFETY: the caller roots the list while an immutable plan owner is cloned.
    let plan = unsafe {
        object(list)
            .0
            .with(|storage| storage.sequence_storage().plan().cloned())
    };
    match plan {
        Some(plan) => crate::finish(Words::deferred(plan).and_then(own_list), std::ptr::null()),
        // SAFETY: omitted bounds copy the entire rooted sequence.
        None => unsafe { super::sequences::imre_seq_slice(list, 0, 0, 0) },
    }
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
pub unsafe extern "C" fn imre_str_list_filter(
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
                if let View::Lines(lines) = &plan.0 {
                    return lines.filter(first, second, exclude, strip as usize);
                }
                if strip == 0 && exclude.is_none() && both_signs(first, second) {
                    return Words::deferred(plan.signed());
                }
                if same_sign(first, second) {
                    let origin;
                    let (rows, originals) = match &plan.0 {
                        View::Lines(_) => unreachable!("text ranges handled above"),
                        View::Full(source) | View::Signed(source) => (
                            source.projected()?,
                            (strip == 2).then(|| source.side(first)).flatten(),
                        ),
                        View::Sorted { rows, origin: weak } => {
                            origin = (strip == 2).then(|| weak.upgrade()).flatten();
                            // Cutting off the tag recovers a source subsequence in
                            // this sorted order only when that input side is sorted.
                            let originals = origin
                                .as_ref()
                                .and_then(|s| s.side(first))
                                .filter(|s| s.strings_ascending());
                            (&**rows, originals)
                        }
                    };
                    return filter(
                        rows,
                        first,
                        second,
                        exclude,
                        strip as usize,
                        originals.as_deref(),
                    );
                }
            }
            filter(
                words.resolved()?,
                first,
                second,
                exclude,
                strip as usize,
                None,
            )
        })
    };
    super::finish(
        result.and_then(own_list),
        std::ptr::null(),
    )
}

/// Stable ordering by the suffix starting at a fixed nonnegative scalar index.
///
/// # Safety
/// The caller roots a list of exact strings, with no callback or concurrent mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_list_sorted_suffix(
    list: *const Object,
    skip: u32,
) -> *const Object {
    // SAFETY: every copied word independently owns its chosen string throughout sorting.
    let result = unsafe {
        object(list).0.with(|storage| {
            let words = storage.sequence_storage();
            let source = match (skip, words.plan()) {
                (2, Some(Deferred(View::Signed(source)))) => Some(source),
                (2, Some(plan @ Deferred(View::Sorted { .. }))) => {
                    return Words::deferred(plan.clone());
                }
                _ => None,
            };
            let input = source.map_or_else(|| words.resolved(), |source| source.projected())?;
            let mut sorted = snapshot(input, std::iter::empty())?;
            let key = |word: usize| suffix(&*(word as *const Str), skip as usize);
            // The standard stable slice sort allocates scratch and aborts when
            // that allocation fails. Keep a stack fallback, as direct string
            // sorting does, so the already-owned result remains sortable.
            let mut local = [std::mem::MaybeUninit::uninit(); 64];
            let mut heap = Vec::<usize>::new();
            let scratch = if sorted.values.len() <= 128
                || heap.try_reserve_exact(sorted.values.len()).is_err()
            {
                local.as_mut_slice()
            } else {
                heap.spare_capacity_mut()
            };
            glidesort::sort_with_buffer_by(&mut sorted.values, scratch, |&a, &b| {
                key(a).cmp(key(b))
            });
            Ok(if let Some(source) = source {
                Words::deferred(Deferred(View::Sorted {
                    rows: Shared::try_new(sorted).map_err(|_| Error::memory_error())?,
                    origin: Shared::downgrade(source),
                }))?
            } else {
                sorted
            })
        })
    };
    crate::finish(
        result.and_then(own_list),
        std::ptr::null(),
    )
}

/// # Safety
/// The caller roots an exact string list. Diff inputs yield at least one row
/// if either source is nonempty, without requiring any intraline matching.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_list_nonempty(list: *const Object) -> u32 {
    // SAFETY: shared input storage remains rooted for this entire check.
    unsafe {
        object(list).0.with(|storage| {
            let words = storage.sequence_storage();
            crate::finish(
                words
                    .plan()
                    .map_or_else(|| Ok(!words.values.is_empty()), Deferred::nonempty)
                    .map(u32::from),
                0,
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
pub unsafe extern "C" fn imre_seq_resolve_header(list: *const Object) -> u32 {
    // SAFETY: initialization uses only independently owned immutable snapshots.
    unsafe {
        object(list)
            .0
            .with(|storage| crate::finish(storage.sequence_storage().resolved_header(), 0))
    }
}
