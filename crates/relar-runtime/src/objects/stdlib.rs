//! Sealed scalar sorting and UTF-8 builders. User callbacks finish before entry.

use std::cmp::Ordering;
use std::rc::Rc;

use super::{Bytes, List, Object, Str, fail, object};

/// Sorts original indices instead of moving key owners. Reversing comparison,
/// rather than the finished sequence, preserves the order of equal keys.
fn ordered_indices(
    length: usize,
    reverse: bool,
    mut compare: impl FnMut(usize, usize) -> Ordering,
) -> Vec<usize> {
    let mut order: Vec<_> = (0..length).collect();
    order.sort_by(|&left, &right| {
        let order = compare(left, right);
        if reverse { order.reverse() } else { order }
    });
    order
}

/// Applies a destination-to-source permutation using owner-preserving swaps.
fn permute<T>(values: &mut [T], order: &[usize]) {
    if values.len() != order.len() {
        fail("SystemError", "sort key count differs from value count");
    }
    let mut visited = vec![false; values.len()];
    for start in 0..values.len() {
        let mut current = start;
        while !visited[current] {
            visited[current] = true;
            let next = order[current];
            if visited[next] {
                break;
            }
            values.swap(current, next);
            current = next;
        }
    }
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
unsafe fn key_order(keys: *const (), kind: u32, reverse: bool) -> Vec<usize> {
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
                    3 => (&*(left as *const Str)).cmp(&*(right as *const Str)),
                    6 => super::values::tuple_order(left as *const usize, right as *const usize),
                    8 => (&*(left as *const Bytes)).cmp(&*(right as *const Bytes)),
                    _ => fail("SystemError", "unsupported sort key representation"),
                }
            })
        })
    }
}

/// Reorders a private materialized result by already evaluated scalar keys.
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
    reverse: u8,
) {
    // SAFETY: both exact list owners remain rooted throughout this operation.
    let order = unsafe { key_order(keys, key_kind, reverse != 0) };
    if element_kind == 2 {
        // SAFETY: all immutable key access has ended; swaps transfer existing owners.
        unsafe {
            (&*values.cast::<List>())
                .0
                .with_mut(|values| permute(values, &order))
        };
    } else {
        // SAFETY: no storage reference escapes and swaps never drop an owner.
        unsafe {
            object(values.cast()).0.with_mut(|storage| {
                let values = storage.sequence_mut();
                values.check_kind(element_kind);
                permute(&mut values.values, &order);
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
    let count = value.matches(old.as_str()).count();
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
pub extern "C" fn relar_float_format(value: f64, precision: u32, percent: u8) -> *const Str {
    if precision > 1000 {
        fail("SystemError", "unchecked float format precision");
    }
    let value = if percent != 0 { value * 100.0 } else { value };
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
    if percent != 0 {
        result.push('%');
    }
    Rc::into_raw(Rc::new(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_permutations_preserve_ties_in_both_directions() {
        let keys = [2, 1, 2, 1, 0];
        for (reverse, expected) in [(false, [4, 1, 3, 0, 2]), (true, [0, 2, 1, 3, 4])] {
            let order = ordered_indices(keys.len(), reverse, |a, b| keys[a].cmp(&keys[b]));
            let mut values = [0, 1, 2, 3, 4];
            permute(&mut values, &order);
            assert_eq!(values, expected);
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
        for (value, precision, percent, expected) in [
            (1.25, 1, 0, "1.2"),
            (2.675, 2, 0, "2.67"),
            (-0.0, 2, 0, "-0.00"),
            (0.125, 0, 1, "12%"),
            (f64::INFINITY, 3, 1, "inf%"),
            (f64::NEG_INFINITY, 0, 0, "-inf"),
            (-f64::NAN, 2, 0, "nan"),
        ] {
            // SAFETY: the formatter transfers exactly one fresh string owner.
            let actual = unsafe { Rc::from_raw(relar_float_format(value, precision, percent)) };
            assert_eq!(actual.as_str(), expected);
        }
    }
}
