//! Sealed scalar sorting and UTF-8 builders. User callbacks finish before entry.

use std::cmp::Ordering;
use std::rc::Rc;

use smallvec::SmallVec;

use super::{Bytes, Error, Kind, List, Object, Storage, StorageCell, Str, Words, fail, object};

/// Sorts original indices instead of moving key owners. Reversing comparison,
/// rather than the finished sequence, preserves the order of equal keys.
fn ordered_indices(
    length: usize,
    reverse: bool,
    mut compare: impl FnMut(usize, usize) -> Ordering,
) -> SmallVec<[usize; 4]> {
    let mut order: SmallVec<_> = (0..length).collect();
    order.sort_by(|&left, &right| {
        let order = compare(left, right);
        if reverse { order.reverse() } else { order }
    });
    order
}

/// Walks each cycle with owner-preserving swaps, marking visited indices in the permutation.
fn permute<T>(values: &mut [T], order: &mut [usize]) {
    if values.len() != order.len() {
        fail("SystemError", "sort key count differs from value count");
    }
    for start in 0..values.len() {
        let mut current = start;
        while order[current] != current {
            let next = std::mem::replace(&mut order[current], current);
            if next == start {
                break;
            }
            values.swap(current, next);
            current = next;
        }
    }
}

/// Long path-like strings often share the same leading bytes. For a large
/// stable sort, find that prefix once before comparing the remaining bytes.
///
pub(super) enum StringSort {
    Stable { reverse: bool },
    Distinct,
}

/// # Safety
/// Each word is an immutable string rooted for the entire sort.
pub(super) unsafe fn sort_strings(values: &mut [usize], mode: StringSort) {
    // SAFETY: the list keeps these owners live, including while they move.
    let bytes = |word: usize| unsafe { (&*(word as *const Str)).as_bytes() };
    let skip = shared_prefix(values.iter().map(|&word| bytes(word)));
    let compare = |&left: &usize, &right: &usize| bytes(left)[skip..].cmp(&bytes(right)[skip..]);
    match mode {
        StringSort::Stable { reverse: false } => values.sort_by(compare),
        StringSort::Stable { reverse: true } => values.sort_by(|left, right| compare(right, left)),
        StringSort::Distinct => sort_distinct_words(values, compare),
    }
}

/// Merge a handful of ascending runs with a fallible scratch buffer. Random
/// input and a failed reservation both use the allocation-free general sort.
fn sort_distinct_words(
    values: &mut [usize],
    compare: impl Fn(&usize, &usize) -> std::cmp::Ordering,
) {
    let mut breaks = smallvec::SmallVec::<[usize; 8]>::new();
    for index in 1..values.len() {
        if compare(&values[index - 1], &values[index]).is_gt() {
            if breaks.len() == breaks.capacity() {
                values.sort_unstable_by(compare);
                return;
            }
            breaks.push(index);
        }
    }
    let mut scratch = Vec::new();
    let ends = breaks
        .iter()
        .copied()
        .skip(1)
        .chain(std::iter::once(values.len()));
    for (&split, end) in breaks.iter().zip(ends) {
        if end - split <= 8 {
            for index in split..end {
                let word = values[index];
                let dest = values[..index].partition_point(|other| compare(other, &word).is_lt());
                values[dest..=index].rotate_right(1);
            }
            continue;
        }
        scratch.clear();
        if scratch.try_reserve_exact(split).is_err() {
            values.sort_unstable_by(compare);
            return;
        }
        scratch.extend_from_slice(&values[..split]);
        let (mut left, mut right) = (0, split);
        for dest in 0..end {
            if left == split {
                // The unconsumed right suffix already starts at its final position.
                break;
            }
            if right < end && compare(&values[right], &scratch[left]).is_lt() {
                values[dest] = values[right];
                right += 1;
            } else {
                values[dest] = scratch[left];
                left += 1;
            }
        }
    }
}

/// Returns a byte prefix shared by every key only when its removal pays for a
/// large sort. UTF-8 byte order after an identical prefix is unchanged.
pub(super) fn shared_prefix<'a>(mut keys: impl ExactSizeIterator<Item = &'a [u8]>) -> usize {
    if keys.len() < 64 {
        return 0;
    }
    let mut prefix = keys.next().unwrap();
    for candidate in keys {
        if !candidate.starts_with(prefix) {
            let end = prefix
                .iter()
                .zip(candidate)
                .take_while(|(a, b)| a == b)
                .count();
            if end < 16 {
                return 0;
            }
            prefix = &prefix[..end];
        }
    }
    if prefix.len() < 16 { 0 } else { prefix.len() }
}

/// # Safety
/// Both words are rooted canonical integers for this comparison.
unsafe fn compare_int(left: usize, right: usize) -> Ordering {
    if left & right & 1 != 0 {
        (left as isize).cmp(&(right as isize))
    } else {
        // SAFETY: the key list roots both arbitrary-precision integers.
        unsafe { crate::relar_int_compare(left, right) }.cmp(&0)
    }
}

/// # Safety
/// `keys` is a rooted list with the exact nonnullable scalar element kind.
/// No mutation or callback overlaps the short immutable storage access.
unsafe fn key_order(keys: *const (), kind: u32, reverse: bool) -> SmallVec<[usize; 4]> {
    if kind == 2 {
        // SAFETY: integer lists have their separate exact runtime allocation.
        return unsafe {
            (&*keys.cast::<List>()).0.with(|values| {
                ordered_indices(values.len(), reverse, |left, right| {
                    compare_int(values[left].0, values[right].0)
                })
            })
        };
    }
    // SAFETY: only the owned permutation escapes, never a reference to key storage.
    unsafe {
        object(keys.cast()).0.with(|storage| {
            let values = storage.sequence();
            values.check_kind(kind);
            ordered_indices(values.values.len(), reverse, |left, right| {
                let (left, right) = (values.values[left], values.values[right]);
                match kind {
                    1 => left.cmp(&right),
                    10 => super::string_enums::compare(left, right),
                    3 => (&*(left as *const Str)).cmp(&*(right as *const Str)),
                    6 => super::values::tuple_order(left as *const usize, right as *const usize),
                    8 => (&*(left as *const Bytes)).cmp(&*(right as *const Bytes)),
                    _ => fail("SystemError", "unsupported sort key representation"),
                }
            })
        })
    }
}

/// Transfers a list's elements to a new owner, leaving its existing aliases
/// observing an empty list while Python key callbacks execute.
///
/// # Safety
/// The caller roots the exact list and supplies its element representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_sort_detach(list: *const (), kind: u32) -> *const () {
    if kind == 2 {
        // SAFETY: no callback runs during this exclusive move of the buffer.
        let saved = unsafe {
            (&*list.cast::<List>())
                .0
                .with_mut(|values| std::mem::replace(values, crate::sorting_empty_ints()))
        };
        crate::new_list(saved).cast()
    } else {
        // SAFETY: the exact descriptor and cached length travel with the owners.
        let saved = unsafe {
            object(list.cast()).0.with_mut(|storage| {
                let words = storage.sequence_mut();
                words.check_kind(kind);
                std::mem::replace(words, Words::sorting_empty(kind))
            })
        };
        // Preserve the header verbatim, including a surrounding sort's marker.
        Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Sequence(saved))))).cast()
    }
}

/// Extending from an iterator without a length hint modifies the temporary
/// list even when it yields nothing. An ordinary destination needs no mutation.
///
/// # Safety
/// The argument is a rooted list with the supplied element representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_extend_unhinted(list: *const (), kind: u32) {
    if kind == 2 {
        // SAFETY: only an unallocated empty marker is replaced.
        unsafe {
            (&*list.cast::<List>()).0.with_mut(|values| {
                if crate::is_sorting_empty_ints(values) {
                    *values = Vec::new();
                }
            })
        };
    } else {
        // SAFETY: the real length stays unchanged at zero.
        unsafe {
            object(list.cast()).0.with_mut(|storage| {
                let words = storage.sequence_mut();
                if words.is_sorting_empty() {
                    words.update_header();
                }
            })
        };
    }
}

/// Restores the saved elements and discards any additions made by key callbacks.
/// A mutation raises ValueError after a completed sort; a failed key keeps its
/// own exception and the original element order.
///
/// # Safety
/// Both same-kind lists are independently rooted and distinct. The private
/// saved list came from the matching detach and cannot be observed elsewhere.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_sort_restore(
    list: *const (),
    kind: u32,
    saved: *const (),
    completed: u32,
) {
    let modified = if kind == 2 {
        // SAFETY: the caller proves distinct handles and no reentrant access.
        unsafe {
            (&*list.cast::<List>()).0.with_mut(|current| {
                let modified = !crate::is_sorting_empty_ints(current);
                (&*saved.cast::<List>())
                    .0
                    .with_mut(|saved| std::mem::swap(current, saved));
                modified
            })
        }
    } else {
        // SAFETY: the swap transfers all owners and headers without releasing
        // any value during access. Normal scope cleanup later drops the additions.
        unsafe {
            object(list.cast()).0.with_mut(|storage| {
                let current = storage.sequence_mut();
                let modified = !current.is_sorting_empty();
                object(saved.cast())
                    .0
                    .with_mut(|storage| std::mem::swap(current, storage.sequence_mut()));
                modified
            })
        }
    };
    if modified && completed != 0 {
        Error::new(Kind::ValueError, "list modified during sort").raise();
    }
}

/// Reorders materialized values by already evaluated scalar keys.
/// Key callbacks and their errors are handled by ordinary generated code before
/// this operation. No comparison calls user code or changes reference counts.
///
/// # Safety
/// Both pointers are rooted exact lists of equal length with the supplied kinds;
/// Kind 2 denotes List, all others a Sequence Object. Keys are ordered scalars or
/// immutable tuples whose fields have a compiler-proved total order.
/// Values own each element independently and cannot be accessed reentrantly.
/// The same list may supply both pointers: key reading ends before any mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_sort_by_keys(
    values: *const (),
    element_kind: u32,
    keys: *const (),
    key_kind: u32,
    reverse: u32,
) {
    if values == keys && key_kind == 3 {
        // Without a key function, the string list itself owns every comparison
        // operand. A stable sort can move those owners directly.
        // SAFETY: the exclusive list access roots both strings across comparison.
        unsafe {
            object(values.cast()).0.with_mut(|storage| {
                let values = storage.sequence_mut();
                values.check_kind(key_kind);
                sort_strings(
                    &mut values.values,
                    StringSort::Stable {
                        reverse: reverse != 0,
                    },
                );
            });
        }
        return;
    }
    // SAFETY: both exact list owners remain rooted throughout this operation.
    let mut order = unsafe { key_order(keys, key_kind, reverse != 0) };
    if element_kind == 2 {
        // SAFETY: all immutable key access has ended; swaps transfer existing owners.
        unsafe {
            (&*values.cast::<List>())
                .0
                .with_mut(|values| permute(values, &mut order))
        };
    } else {
        // SAFETY: no storage reference escapes and swaps never drop an owner.
        unsafe {
            object(values.cast()).0.with_mut(|storage| {
                let values = storage.sequence_mut();
                values.check_kind(element_kind);
                permute(&mut values.values, &mut order);
            })
        };
    }
}

fn string_capacity(length: Option<usize>) -> usize {
    length
        .filter(|&length| length <= isize::MAX as usize)
        .unwrap_or_else(|| fail("MemoryError", "string is too large"))
}

/// Joins an exact string sequence, retaining a single item without copying it.
///
/// # Safety
/// The separator and exact list[str] are rooted. No callback or mutation occurs
/// during the call; only an independent string owner escapes the storage borrow.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_join(
    separator: *const Str,
    values: *const Object,
) -> *const Str {
    // SAFETY: immutable string contents remain rooted by the caller and list.
    unsafe {
        let separator = &*separator;
        object(values).0.with(|storage| {
            let values = storage.sequence();
            values.check_kind(3);
            if let [word] = values.values.as_slice() {
                let value = *word as *const Str;
                Rc::increment_strong_count(value);
                return value;
            }
            let separators = string_capacity(
                separator
                    .len()
                    .checked_mul(values.values.len().saturating_sub(1)),
            );
            let length = values.values.iter().try_fold(separators, |length, &word| {
                length.checked_add((&*(word as *const Str)).len())
            });
            let mut result = Str::with_capacity(string_capacity(length));
            for (index, &word) in values.values.iter().enumerate() {
                if index != 0 {
                    result.push_str(separator);
                }
                result.push_str(&*(word as *const Str));
            }
            Rc::into_raw(Rc::new(result))
        })
    }
}

/// Substitutes plain named fields in encounter order. A missing field can raise
/// before a malformed brace later in the same template.
fn format_named<'a>(template: &str, field: impl Fn(&str) -> Option<&'a str>) -> crate::Result<Str> {
    // A long template can consist entirely of fields with empty replacements.
    let mut result = Str::default();
    let mut rest = template;
    while let Some(at) = memchr::memchr2(b'{', b'}', rest.as_bytes()) {
        result.push_str(&rest[..at]);
        let brace = rest.as_bytes()[at];
        rest = &rest[at + 1..];
        if rest.as_bytes().first() == Some(&brace) {
            result.push(char::from(brace));
            rest = &rest[1..];
            continue;
        }
        if brace == b'}' || rest.is_empty() {
            return Err(Error::new(
                Kind::ValueError,
                if brace == b'}' {
                    "Single '}' encountered in format string"
                } else {
                    "Single '{' encountered in format string"
                },
            ));
        }
        let Some(end) = rest.find(['{', '}', '!', ':']) else {
            return Err(Error::new(
                Kind::ValueError,
                "expected '}' before end of string",
            ));
        };
        let (field_name, mut tail) = rest.split_at(end);
        if tail.starts_with('{') {
            return Err(Error::new(Kind::ValueError, "unexpected '{' in field name"));
        }
        let name = field_name.split(['.', '[']).next().unwrap();
        let mut supported = name.len() == field_name.len();
        let unmatched = || Error::new(Kind::ValueError, "unmatched '{' in format spec");
        if let Some(conversion) = tail.strip_prefix('!') {
            let mut chars = conversion.chars();
            let flag = chars.next().ok_or_else(|| {
                Error::new(
                    Kind::ValueError,
                    "end of string while looking for conversion specifier",
                )
            })?;
            supported &= flag == 's';
            tail = chars.as_str();
            if !tail.is_empty() && !tail.starts_with([':', '}']) {
                return Err(Error::new(
                    Kind::ValueError,
                    "expected ':' after conversion specifier",
                ));
            }
        }
        if let Some(spec) = tail.strip_prefix(':') {
            let end = spec.find('}').ok_or_else(unmatched)?;
            supported &= end == 0;
            tail = &spec[end..];
        }
        rest = tail.strip_prefix('}').ok_or_else(unmatched)?;
        if name
            .chars()
            .all(|ch| crate::integer_text::decimal_digit(ch).is_some())
        {
            let index = name
                .chars()
                .try_fold(0isize, |index, ch| {
                    index
                        .checked_mul(10)?
                        .checked_add(crate::integer_text::decimal_digit(ch)? as isize)
                })
                .ok_or_else(|| {
                    Error::new(Kind::ValueError, "Too many decimal digits in format string")
                })?;
            return Err(Error::new(
                Kind::IndexError,
                compact_str::format_compact!(
                    "Replacement index {index} out of range for positional args tuple"
                ),
            ));
        }
        let text = field(name).ok_or_else(|| Error::string_key(Str::new(name)))?;
        if !supported {
            return Err(Error::new(
                Kind::ValueError,
                "Relar str.format supports plain named fields and !s",
            ));
        }
        result.push_str(text);
    }
    result.push_str(rest);
    Ok(result)
}

/// # Safety
/// The template and each of the `count` pairs of immutable name and value
/// handles are rooted until return. The result has its own string owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_format(
    template: *const Str,
    fields: *const [*const Str; 2],
    count: usize,
) -> *const Str {
    // SAFETY: the compiler supplies one stack slot per rooted argument pair.
    let (template, fields) = unsafe { (&*template, std::slice::from_raw_parts(fields, count)) };
    let formatted = format_named(template, |name| {
        fields.iter().find_map(|&[key, value]| {
            // SAFETY: the operands stay immutable and rooted until formatting ends.
            unsafe { ((&*key).as_str() == name).then(|| (&*value).as_str()) }
        })
    });
    super::finish(
        formatted.map(|text| Rc::into_raw(Rc::new(text))),
        std::ptr::null(),
    )
}

/// # Safety
/// All three handles are rooted immutable UTF-8 strings. Empty matches occur at
/// Unicode scalar boundaries, including both ends, exactly as Python str.replace.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_replace(
    value: *const Str,
    old: *const Str,
    new: *const Str,
) -> *const Str {
    // SAFETY: inputs remain immutable and rooted through both sizing and copying.
    let (value, old, new) = unsafe { (&*value, &*old, &*new) };
    let count = if old == new {
        0
    } else {
        value.matches(old.as_str()).count()
    };
    if count == 0 {
        // SAFETY: the caller roots the input; the unchanged result takes its own owner.
        return Rc::into_raw(unsafe {
            super::acquire_rc::<Str>(std::ptr::from_ref(value) as usize)
        });
    }
    let length = value
        .len()
        .checked_sub(old.len().checked_mul(count).unwrap())
        .and_then(|length| {
            new.len()
                .checked_mul(count)
                .and_then(|extra| length.checked_add(extra))
        });
    let mut result = Str::with_capacity(string_capacity(length));
    let mut copied = 0;
    for (index, _) in value.match_indices(old.as_str()) {
        result.push_str(&value[copied..index]);
        result.push_str(new);
        copied = index + old.len();
    }
    result.push_str(&value[copied..]);
    Rc::into_raw(Rc::new(result))
}

/// Formats a binary64 value with correctly rounded fixed decimal precision.
/// Percent scaling is binary64 multiplication, before rounding. Rust's fixed
/// formatter uses ties-to-even; Python's lowercase nonfinite spellings are explicit.
#[unsafe(no_mangle)]
pub extern "C" fn relar_float_format(value: f64, precision: u32, flags: u32) -> *const Str {
    if precision > 1000 {
        fail("SystemError", "unchecked float format precision");
    }
    // The low bit selects percentages. A zero high byte keeps the default sign.
    let percent = flags & 1 != 0;
    let positive_sign = (flags >> 8) as u8;
    let value = if percent { value * 100.0 } else { value };
    let mut result = if value.is_nan() {
        Str::from("nan")
    } else if value.is_infinite() {
        Str::from(if value.is_sign_negative() {
            "-inf"
        } else {
            "inf"
        })
    } else {
        let precision = precision as usize;
        compact_str::format_compact!("{value:.precision$}")
    };
    if positive_sign != 0 && !result.starts_with('-') {
        result.insert(0, char::from(positive_sign));
    }
    if percent {
        result.push('%');
    }
    Rc::into_raw(Rc::new(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_word_sort_handles_run_boundaries() {
        for runs in 1..=10 {
            for length in [0, 1, 7, 8, 31, 129] {
                let mut words: Vec<_> = (0..runs)
                    .flat_map(|start| (start..length).step_by(runs))
                    .collect();
                super::sort_distinct_words(&mut words, usize::cmp);
                assert!(words.into_iter().eq(0..length));
            }
        }
    }

    #[test]
    fn empty_named_fields_do_not_reserve_the_template_length() {
        let template = "{x}".repeat(20_000) + "{y}";
        let result = format_named(&template, |name| match name {
            "x" => Some(""),
            "y" => Some("雪"),
            _ => None,
        })
        .unwrap_or_else(|_| panic!("valid named fields"));
        assert_eq!(result.as_str(), "雪");
        assert!(!result.is_heap_allocated());
    }

    #[test]
    fn stable_permutations_preserve_ties_in_both_directions() {
        let keys = [2, 1, 2, 1, 0];
        for (reverse, expected) in [(false, [4, 1, 3, 0, 2]), (true, [0, 2, 1, 3, 4])] {
            let mut order = ordered_indices(keys.len(), reverse, |a, b| keys[a].cmp(&keys[b]));
            let mut values = [0, 1, 2, 3, 4];
            permute(&mut values, &mut order);
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn short_sorts_keep_the_permutation_inline() {
        for length in 0..=4 {
            let mut order = ordered_indices(length, true, |left, right| left.cmp(&right));
            assert!(!order.spilled());
            let mut values: Vec<_> = (0..length).collect();
            permute(&mut values, &mut order);
            assert!(values.into_iter().eq((0..length).rev()));
        }
    }

    #[test]
    fn direct_string_sorts_preserve_equal_owners_in_both_directions() {
        let texts = ["é", "", "é", "z", ""].map(|s| Rc::new(Str::from(s)));
        for (reverse, order) in [(0, [1, 4, 3, 0, 2]), (1, [0, 2, 3, 1, 4])] {
            // SAFETY: the list and its string owners stay rooted throughout the sort.
            unsafe {
                let list = Rc::from_raw(super::super::relar_seq_new(3));
                let pointer = Rc::as_ptr(&list);
                for text in &texts {
                    super::super::relar_seq_append(pointer, Rc::as_ptr(text) as usize, 3);
                }
                relar_sort_by_keys(pointer.cast(), 3, pointer.cast(), 3, reverse);
                list.0.with(|storage| {
                    assert_eq!(
                        storage.sequence().values.as_slice(),
                        order.map(|i| Rc::as_ptr(&texts[i]) as usize),
                    );
                });
            }
            assert!(texts.iter().all(|text| Rc::strong_count(text) == 1));
        }
    }

    #[test]
    fn sorting_shared_prefixes_uses_unicode_byte_order_and_stable_ties() {
        for prefix in ["", "a-long-directory-name/"] {
            let keys: Vec<_> = (0..96)
                .map(|i| Rc::new(Str::from(format!("{prefix}{}", ["é", "è", "ê"][i % 3]))))
                .collect();
            for reverse in [false, true] {
                let mut expected: Vec<_> = keys.iter().collect();
                expected.sort_by(|a, b| if reverse { b.cmp(a) } else { a.cmp(b) });
                let mut words: Vec<_> = keys.iter().map(|s| Rc::as_ptr(s) as usize).collect();
                // SAFETY: keys owns each immutable string until the comparison finishes.
                unsafe { sort_strings(&mut words, StringSort::Stable { reverse }) };
                assert!(
                    expected
                        .into_iter()
                        .map(|s| Rc::as_ptr(s) as usize)
                        .eq(words)
                );
            }
        }
    }

    #[test]
    fn failed_key_restores_owners_and_drops_temporary_additions() {
        let original = Rc::new(Str::from("original"));
        let addition = Rc::new(Str::from("addition"));
        // SAFETY: both strings and both distinct list handles stay rooted. The
        // saved list is released exactly once after restoration.
        unsafe {
            let list = Rc::from_raw(super::super::relar_seq_new(3));
            let alias = Rc::clone(&list);
            let target = Rc::as_ptr(&list);
            super::super::relar_seq_append(target, Rc::as_ptr(&original) as usize, 3);
            let saved = Rc::from_raw(relar_sort_detach(target.cast(), 3).cast::<Object>());
            assert!(alias.0.with(|storage| storage.sequence().values.is_empty()));
            assert_eq!(Rc::strong_count(&original), 2);
            super::super::relar_seq_append(target, Rc::as_ptr(&addition) as usize, 3);
            relar_sort_restore(target.cast(), 3, Rc::as_ptr(&saved).cast(), 0);
            assert_eq!(
                alias.0.with(|storage| storage.sequence().values[0]),
                Rc::as_ptr(&original) as usize,
            );
            drop(saved);
            assert_eq!(Rc::strong_count(&addition), 1);
            assert_eq!(Rc::strong_count(&original), 2);
        }
        assert_eq!(Rc::strong_count(&original), 1);
    }

    #[test]
    fn speculative_range_capacity_does_not_mark_integer_sort_as_modified() {
        // SAFETY: the buffers are distinct; the range words encode 0, 8, and 1.
        unsafe {
            let target = crate::new_list(vec![crate::Int(5)]);
            let saved = relar_sort_detach(target.cast(), 2);
            crate::relar_list_reserve_range(target, 1, 17, 3, 1);
            assert!(
                (*target)
                    .0
                    .with(|values| crate::is_sorting_empty_ints(values))
            );
            relar_sort_restore(target.cast(), 2, saved, 1);
            crate::relar_list_release(saved.cast());
            assert_eq!((*target).0.with(|values| values[0].0), 5);
            crate::relar_list_release(target);
        }
    }

    #[test]
    fn sorting_keeps_generic_element_owners_and_supports_aliased_integer_keys() {
        let texts = ["third", "first", "second"].map(|text| Rc::new(Str::from(text)));
        // SAFETY: every raw argument is rooted here; returned list handles each
        // transfer one reference, released after inspecting their owned elements.
        unsafe {
            let values = super::super::relar_seq_new(3);
            let keys = crate::relar_list_new();
            for (text, key) in texts.iter().zip([3, 1, 2]) {
                super::super::relar_seq_append(values, Rc::as_ptr(text) as usize, 3);
                crate::relar_list_append(keys, crate::Int::small(key).unwrap().0);
            }
            relar_sort_by_keys(values.cast(), 3, keys.cast(), 2, 0);
            for (index, expected) in ["first", "second", "third"].iter().enumerate() {
                let text =
                    Rc::from_raw(super::super::relar_seq_get_index(values, index) as *const Str);
                assert_eq!(text.as_str(), *expected);
            }
            assert!(texts.iter().all(|text| Rc::strong_count(text) == 2));
            relar_sort_by_keys(keys.cast(), 2, keys.cast(), 2, 0);
            for index in 0..3 {
                let value = crate::Int(crate::relar_list_get_index(keys, index));
                assert_eq!(value.as_small(), Some(index as i64 + 1));
            }
            super::super::relar_object_release(values);
            crate::relar_list_release(keys);
        }
        assert!(texts.iter().all(|text| Rc::strong_count(text) == 1));
    }

    #[test]
    fn joining_and_replacement_preserve_unicode_and_empty_matches() {
        let separator = Rc::new(Str::from("·"));
        let old = Rc::new(Str::from(""));
        let new = Rc::new(Str::from("/"));
        let texts = ["α", "", "🙂"].map(|text| Rc::new(Str::from(text)));
        // SAFETY: inputs stay rooted until both fresh returned owners are dropped.
        unsafe {
            let values = super::super::relar_seq_new(3);
            for text in &texts {
                super::super::relar_seq_append(values, Rc::as_ptr(text) as usize, 3);
            }
            let joined = Rc::from_raw(relar_str_join(Rc::as_ptr(&separator), values));
            assert_eq!(joined.as_str(), "α··🙂");
            let replaced = Rc::from_raw(relar_str_replace(
                Rc::as_ptr(&joined),
                Rc::as_ptr(&old),
                Rc::as_ptr(&new),
            ));
            assert_eq!(replaced.as_str(), "/α/·/·/🙂/");
            super::super::relar_object_release(values);
        }
        assert!(texts.iter().all(|text| Rc::strong_count(text) == 1));
    }

    #[test]
    fn unchanged_replacements_return_an_owner_of_the_source() {
        let value = Rc::new(Str::from("word".repeat(1024)));
        for (old, new) in [("absent", "!"), ("word", "word"), ("", "")] {
            let old = Rc::new(Str::from(old));
            let new = Rc::new(Str::from(new));
            // SAFETY: the three inputs stay rooted, and the result transfers an owner.
            let result = unsafe {
                Rc::from_raw(relar_str_replace(
                    Rc::as_ptr(&value),
                    Rc::as_ptr(&old),
                    Rc::as_ptr(&new),
                ))
            };
            assert!(Rc::ptr_eq(&result, &value));
        }
    }

    #[test]
    fn single_item_join_keeps_an_owner_after_the_input_list_is_released() {
        let separator = Rc::new(Str::from("unused separator"));
        let text = Rc::new(Str::from("an independently owned λ🙂 string"));
        // SAFETY: the list retains text; joining transfers a second string owner.
        unsafe {
            let values = super::super::relar_seq_new(3);
            super::super::relar_seq_append(values, Rc::as_ptr(&text) as usize, 3);
            let joined = Rc::from_raw(relar_str_join(Rc::as_ptr(&separator), values));
            assert!(Rc::ptr_eq(&text, &joined));
            super::super::relar_object_release(values);
            drop(text);
            assert_eq!(Rc::strong_count(&joined), 1);
            assert_eq!(joined.as_str(), "an independently owned λ🙂 string");
        }
    }

    #[test]
    fn fixed_float_formats_preserve_rounding_sign_and_nonfinite_spelling() {
        let plus = u32::from(b'+') << 8;
        let space = u32::from(b' ') << 8;
        for (value, precision, flags, expected) in [
            (1.25, 1, 0, "1.2"),
            (2.675, 2, 0, "2.67"),
            (-0.0, 2, 0, "-0.00"),
            (0.125, 0, 1, "12%"),
            (f64::INFINITY, 3, 1, "inf%"),
            (f64::NEG_INFINITY, 0, 0, "-inf"),
            (-f64::NAN, 2, 0, "nan"),
            (-f64::NAN, 2, plus | 1, "+nan%"),
            (0.0, 2, space, " 0.00"),
            (-0.0, 2, plus | 1, "-0.00%"),
        ] {
            // SAFETY: the formatter transfers exactly one fresh string owner.
            let actual = unsafe { Rc::from_raw(relar_float_format(value, precision, flags)) };
            assert_eq!(actual.as_str(), expected);
        }
    }
}
