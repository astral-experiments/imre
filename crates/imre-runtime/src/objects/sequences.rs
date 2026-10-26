//! Checked Unicode sequence access and shallow slices of concrete list storage.

use crate::StrOwner;
use num_traits::{Signed, ToPrimitive};

use super::exceptions::{Error, Kind, Result, finish};
use super::text::owned_string;
use super::words::{OwnedWord, WordKind, Words};
use super::{Int, List, Object, Storage, StorageCell, Str, fail, object};

/// Returns a native bound large enough to preserve every possible sequence
/// position. Unlike indexing, slicing clamps arbitrary-precision integers.
///
/// # Safety
/// Zero denotes an omitted/None component; every other word is a rooted Int.
unsafe fn slice_component(word: usize) -> Option<isize> {
    if word == 0 {
        return None;
    }
    // SAFETY: nonzero components borrow canonical integer handles.
    let value = unsafe { Int::borrow(word) };
    Some(if let Some(value) = value.as_small() {
        value as isize
    } else {
        let value = value.big();
        value.to_isize().unwrap_or_else(|| {
            if value.is_negative() {
                isize::MIN
            } else {
                isize::MAX
            }
        })
    })
}

/// A finite cursor over valid indices. Keeping a count avoids advancing past
/// the final item, where an arbitrarily large stride could otherwise overflow.
#[derive(Clone)]
pub(super) struct SliceIndices {
    at: usize,
    stride: usize,
    remaining: usize,
    reverse: bool,
}

impl SliceIndices {
    pub(super) fn is_full_forward(&self, length: usize) -> bool {
        !self.reverse && self.remaining == length
    }

    /// Maps an original slot to its position in this slice without walking skipped slots.
    pub(super) fn position_of(&self, source: usize) -> Option<usize> {
        let delta = if self.reverse {
            self.at.checked_sub(source)?
        } else {
            source.checked_sub(self.at)?
        };
        let pos = delta / self.stride;
        (pos < self.remaining && delta.is_multiple_of(self.stride)).then_some(pos)
    }

    /// Normalizes against the length at the time of access. Omitted negative
    /// stops stay at -1; an explicit -1 first counts backward from the length.
    ///
    /// ```python
    /// values[::-1]    # All elements in reverse.
    /// values[:-1:-1]  # Empty: explicit -1 names the final element.
    /// ```
    ///
    /// # Safety
    /// Components are omitted/None words or rooted canonical Int handles. The
    /// sequence length fits isize, as all runtime-owned allocations require.
    pub(super) unsafe fn new(
        length: usize,
        start: usize,
        stop: usize,
        step: usize,
    ) -> Result<Self> {
        debug_assert!(length <= isize::MAX as usize);
        // SAFETY: all components remain rooted for this normalization.
        let step = unsafe { slice_component(step) }.unwrap_or(1);
        if step == 0 {
            return Err(Error::new(Kind::ValueError, "slice step cannot be zero"));
        }
        let reverse = step < 0;
        let length = length as isize;
        let (lower, upper) = if reverse {
            (-1, length - 1)
        } else {
            (0, length)
        };
        let normalize = |value: isize| {
            let value = if value < 0 { value + length } else { value };
            value.clamp(lower, upper)
        };
        // SAFETY: each bound remains rooted; conversion cannot call user code.
        let start = unsafe { slice_component(start) }
            .map(normalize)
            .unwrap_or(if reverse { upper } else { lower });
        // SAFETY: the stop has the same nullable integer representation.
        let stop = unsafe { slice_component(stop) }
            .map(normalize)
            .unwrap_or(if reverse { lower } else { upper });
        let distance = if reverse { start - stop } else { stop - start };
        let stride = step.unsigned_abs();
        let remaining = if distance > 0 {
            (distance as usize - 1) / stride + 1
        } else {
            0
        };
        Ok(Self {
            at: if remaining == 0 { 0 } else { start as usize },
            stride,
            remaining,
            reverse,
        })
    }
}

impl Iterator for SliceIndices {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let at = self.at;
        self.remaining -= 1;
        if self.remaining != 0 {
            self.at = if self.reverse {
                at - self.stride
            } else {
                at + self.stride
            };
        }
        Some(at)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for SliceIndices {}

/// Returns an independently owned one-code-point string. Negative indices walk
/// backward through UTF-8, without first scanning the complete input for length.
///
/// # Safety
/// `value` and the exact canonical integer index remain rooted for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_get(value: *const Str, at: usize) -> *const Str {
    // SAFETY: the caller roots the integer throughout conversion.
    let at = unsafe { Int::borrow(at) };
    // SAFETY: the string is immutable and rooted throughout the read.
    let value = unsafe { &*value };
    let character = match string_character(value, &at) {
        Ok(character) => character,
        Err(error) => return error.returning(std::ptr::null()),
    };
    StrOwner::into_raw(owned_string(character.encode_utf8(&mut [0; 4])))
}

/// Shares Unicode indexing and exact failure messages between owned strings and
/// caller-buffer fragments; negative indices avoid a complete length scan.
pub(super) fn string_character(value: &str, at: &Int) -> Result<char> {
    let at = crate::decode_index(at).map_err(|_| {
        Error::new(
            Kind::IndexError,
            "cannot fit 'int' into an index-sized integer",
        )
    })?;
    let character = if at < 0 {
        value.chars().rev().nth(at.unsigned_abs() - 1)
    } else {
        value.chars().nth(at as usize)
    };
    character.ok_or_else(|| Error::new(Kind::IndexError, "string index out of range"))
}

/// Copies selected code points in one forward or backward pass. UTF-8 offsets
/// never escape, and a large stride is used only when another result exists.
fn copy_string_slice(
    mut characters: impl Iterator<Item = char>,
    mut skip: usize,
    slice: &SliceIndices,
    byte_length: usize,
) -> Str {
    let capacity = byte_length.min(slice.len().saturating_mul(4));
    let mut result = Str::try_with_capacity(capacity)
        .unwrap_or_else(|_| fail("MemoryError", "string allocation failed"));
    for _ in 0..slice.len() {
        let character = characters.nth(skip).expect("normalized string slice");
        result.push(character);
        skip = slice.stride - 1;
    }
    result
}

/// Slices by Unicode code points, preserving combining marks and embedded NULs
/// without grapheme grouping or normalization.
///
/// # Safety
/// The string and nonzero component words remain rooted. Zero denotes omitted
/// or None; other components are canonical Int handles. Returns one owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_slice(
    value: *const Str,
    start: usize,
    stop: usize,
    step: usize,
) -> *const Str {
    // SAFETY: the immutable input remains rooted through all scans.
    let text = unsafe { &*value };
    let length = text.chars().count();
    // SAFETY: all borrowed nullable integer components remain rooted.
    let slice = match unsafe { SliceIndices::new(length, start, stop, step) } {
        Ok(slice) => slice,
        Err(error) => return error.returning(std::ptr::null()),
    };
    if !slice.reverse && slice.stride == 1 && slice.len() == length {
        // SAFETY: a full slice may share immutable storage; acquire its new owner.
        return StrOwner::into_raw(unsafe { crate::StrOwner::acquire_word(value as usize) });
    }
    let result = if !slice.reverse && slice.stride == 1 {
        let selected = if slice.len() == 0 {
            ""
        } else if length == text.len() {
            // Equal byte and code-point counts prove that every byte is ASCII.
            &text[slice.at..slice.at + slice.len()]
        } else {
            let mut boundaries = text
                .char_indices()
                .map(|(offset, _)| offset)
                .chain(std::iter::once(text.len()));
            let start = boundaries.nth(slice.at).expect("normalized slice start");
            let end = boundaries
                .nth(slice.len() - 1)
                .expect("normalized slice end");
            &text[start..end]
        };
        // A contiguous slice needs one copy, without decoding and appending
        // every selected character again. The result owns its UTF-8 bytes.
        Str::new(selected)
    } else if slice.reverse && slice.len() != 0 {
        copy_string_slice(
            text.chars().rev(),
            length - 1 - slice.at,
            &slice,
            text.len(),
        )
    } else {
        copy_string_slice(text.chars(), slice.at, &slice, text.len())
    };
    StrOwner::into_raw(StrOwner::new(result))
}

/// Copies integer owners into a distinct list, including for a full slice.
///
/// # Safety
/// Root the source list and every nonzero canonical Int component. Zero denotes
/// an omitted/None component; the returned list owns all selected values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_slice(
    value: *const List,
    start: usize,
    stop: usize,
    step: usize,
) -> *const List {
    // SAFETY: cloning selected Int owners cannot reenter. Only a new owned
    // vector escapes the source storage borrow; pending is installed afterward.
    let result = unsafe {
        (&*value).0.with(|values| {
            let slice = SliceIndices::new(values.len(), start, stop, step)?;
            let mut result = crate::take_list_buffer(slice.len());
            result
                .try_reserve_exact(slice.len())
                .map_err(|_| Error::allocation())?;
            result.extend(slice.map(|at| values[at].clone()));
            Ok(result)
        })
    };
    finish(result.map(crate::new_list), std::ptr::null())
}

/// Reduces the selected integer values without allocating the temporary slice.
///
/// The compiler evaluates this subtotal where the original slice was created,
/// before evaluating and adding `sum`'s start. Later source mutations therefore
/// cannot change the numeric snapshot, and zero-step errors retain their order.
///
/// # Safety
/// Root the source list and every nonzero canonical Int component. Zero denotes
/// an omitted/None component. No source borrow escapes; success returns one
/// owned canonical integer, while failure installs an error and returns tagged zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_slice_sum(
    value: *const List,
    start: usize,
    stop: usize,
    step: usize,
) -> usize {
    // SAFETY: normalization and exact integer addition cannot reenter the
    // rooted list. Only an independently owned subtotal escapes its borrow.
    let subtotal = unsafe {
        (&*value).0.with(|values| {
            let slice = SliceIndices::new(values.len(), start, stop, step)?;
            Ok(sum_integer_slice(values, slice))
        })
    };
    finish(subtotal.map(Int::into_raw), 1)
}

/// Exposes unit and alternating strides as bounded slice iterators, so the
/// checked integer reduction can vectorize without a bounds check per item.
/// Both its speculative pass and arbitrary-precision fallback retain the
/// original selection order, including reverse slices.
fn sum_integer_slice(values: &[Int], slice: SliceIndices) -> Int {
    if slice.len() == 0 {
        return Int::small(0).unwrap();
    }
    if slice.stride > 2 {
        return crate::sum_ints(slice.map(|at| &values[at]));
    }
    // Normalization proves both endpoints are valid and their distance fits
    // the source length. Slicing checks that bound once before iteration.
    let distance = (slice.len() - 1) * slice.stride;
    let (first, last) = if slice.reverse {
        (slice.at - distance, slice.at)
    } else {
        (slice.at, slice.at + distance)
    };
    let values = &values[first..=last];
    if slice.stride == 1 {
        if slice.reverse {
            crate::sum_ints(values.iter().rev())
        } else {
            crate::sum_ints(values.iter())
        }
    } else if slice.reverse {
        let (remainder, pairs) = values.as_rchunks::<2>();
        crate::sum_ints(
            pairs
                .iter()
                .rev()
                .map(|pair| &pair[1])
                .chain(remainder.iter().rev()),
        )
    } else {
        let (pairs, remainder) = values.as_chunks::<2>();
        crate::sum_ints(pairs.iter().map(|pair| &pair[0]).chain(remainder.iter()))
    }
}

/// Preserves the source's concrete element descriptor and acquires shallow
/// copies, so referenced objects retain shared identity after either list drops.
///
/// # Safety
/// Root the source sequence and all nonzero canonical Int component words.
/// Zero denotes omitted/None; the result transfers one independent list owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_slice(
    value: *const Object,
    start: usize,
    stop: usize,
    step: usize,
) -> *const Object {
    // SAFETY: exact-kind acquisition cannot reenter and only independent owners
    // leave storage. A normalization failure is raised after the borrow ends.
    let result = unsafe {
        object(value).0.with(|storage| {
            if let Some(rows) = storage.sequence_storage().rows() {
                return rows.slice(SliceIndices::new(rows.len(), start, stop, step)?);
            }
            let values = storage.sequence()?;
            let slice = SliceIndices::new(values.values.len(), start, stop, step)?;
            let mut result = Words::new(values.kind.raw());
            result
                .values
                .try_reserve_exact(slice.len())
                .map_err(|_| Error::allocation())?;
            for at in slice {
                result
                    .values
                    .push(values.kind.acquire(values.values[at]).into_word());
            }
            Ok(result)
        })
    };
    finish(
        result.and_then(|values| {
            crate::ObjectOwner::try_new(Object(StorageCell::new(values.into_storage())))
                .map(crate::ObjectOwner::into_raw)
                .map_err(|_| Error::memory_error())
        }),
        std::ptr::null(),
    )
}

/// Roots the immutable source and advances a UTF-8 byte boundary once per code
/// point. No character-index lookup or iterator borrow survives a loop body.
pub(super) struct StringIterator {
    owner: StrOwner,
    position: usize,
    reverse: bool,
}

impl StringIterator {
    pub(super) fn next(&mut self) -> Option<(OwnedWord, Option<OwnedWord>)> {
        let character = if self.reverse {
            let character = self.owner[..self.position].chars().next_back()?;
            self.position -= character.len_utf8();
            character
        } else {
            let character = self.owner[self.position..].chars().next()?;
            self.position += character.len_utf8();
            character
        };
        let value = owned_string(character.encode_utf8(&mut [0; 4]));
        let word = StrOwner::into_raw(value) as usize;
        // SAFETY: the string transfers one owner of the exact string kind.
        let value = unsafe { OwnedWord::from_owned(word, WordKind::new(3)) };
        Some((value, None))
    }
}

/// Creates a linear Unicode-code-point cursor for the shared iterator-next ABI.
/// Each successful step transfers an owned string in key_out; value_out is unused.
///
/// # Safety
/// The source string is rooted. Release the returned Object on every loop exit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_iter(value: *const Str) -> *const Object {
    // SAFETY: the rooted source acquires an independent owner for the cursor.
    let owner = unsafe { crate::StrOwner::acquire_word(value as usize) };
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::StringIterator(StringIterator {
            owner,
            position: 0,
            reverse: false,
        }),
    ))))
}

/// Creates a reverse Unicode cursor without copying or rescanning the prefix.
///
/// # Safety
/// The source is rooted. Release the returned iterator on every consumer exit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_reverse_iter(value: *const Str) -> *const Object {
    // SAFETY: the rooted source acquires an independent owner for this cursor.
    let owner: StrOwner = unsafe { crate::StrOwner::acquire_word(value as usize) };
    let position = owner.len();
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::StringIterator(StringIterator {
            owner,
            position,
            reverse: true,
        }),
    ))))
}

/// Builds the eager integer list without allocating one-character strings.
///
/// ```python
/// list(map(int, "1٢3"))  # [1, 2, 3]
/// ```
///
/// Only Unicode 16 decimal digits are valid single-character integer literals.
/// The first invalid code point installs the ordinary `int` ValueError and
/// returns null; success transfers a fresh list containing immediate integers.
///
/// # Safety
/// The caller roots the immutable string for this call. No borrowed source or
/// private output buffer escapes, and conversion cannot invoke user code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_int_list(value: *const Str) -> *const List {
    // SAFETY: the caller keeps this immutable string rooted throughout conversion.
    let text = unsafe { &*value };
    let ascii = text.is_ascii();
    let length = if ascii {
        text.len()
    } else {
        text.chars().count()
    };
    let mut values = crate::take_list_buffer(length);
    if values.try_reserve_exact(length).is_err() {
        return Error::allocation().returning(std::ptr::null());
    }
    if ascii {
        let mut invalid = false;
        for (slot, &byte) in values.spare_capacity_mut()[..length]
            .iter_mut()
            .zip(text.as_bytes())
        {
            let digit = byte.wrapping_sub(b'0');
            invalid |= digit > 9;
            slot.write(Int::small(i64::from(digit)).unwrap());
        }
        if invalid {
            let index = text
                .as_bytes()
                .iter()
                .position(|byte| !byte.is_ascii_digit())
                .unwrap();
            return crate::integer_text::invalid_literal(&text[index..index + 1])
                .returning(std::ptr::null());
        }
    } else {
        for (slot, character) in values.spare_capacity_mut()[..length]
            .iter_mut()
            .zip(text.chars())
        {
            let Some(digit) = crate::integer_text::decimal_digit(character) else {
                let mut encoded = [0; 4];
                return crate::integer_text::invalid_literal(character.encode_utf8(&mut encoded))
                    .returning(std::ptr::null());
            };
            slot.write(Int::small(i64::from(digit)).unwrap());
        }
    }
    // SAFETY: both equal-length zips initialized every slot. All written values,
    // including an invalid ASCII byte's unpublished conversion, fit immediate
    // Int storage and own no allocation. Errors leave length zero for cleanup.
    unsafe { values.set_len(length) };
    crate::new_list(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::exceptions::{
        imre_error_pending, imre_error_take, imre_exception_matches, imre_exception_message,
    };
    use crate::objects::mappings::imre_mapping_iter_next;
    use crate::objects::{imre_seq_append, imre_seq_get_index, imre_seq_new};
    use crate::{
        imre_list_append, imre_list_get_index, imre_list_len, imre_list_new, imre_list_set_index,
    };
    use num_bigint::BigInt;
    use std::rc::Rc;

    fn integer(value: i64) -> usize {
        Int::small(value).unwrap().into_raw()
    }

    fn indices(length: usize, components: [Option<BigInt>; 3]) -> Vec<usize> {
        let components = components.map(|value| value.map(Int::new));
        let [start, stop, step] = components
            .each_ref()
            .map(|value| value.as_ref().map_or(0, |value| value.0));
        // SAFETY: all component owners remain live while the slice is normalized.
        unsafe { SliceIndices::new(length, start, stop, step) }
            .unwrap_or_else(|_| panic!("unexpected slice error"))
            .collect()
    }

    fn clear_error(kind: Kind, expected: &str) {
        assert_eq!(imre_error_pending(), 1);
        // SAFETY: taking pending transfers one exception owner; reading its
        // message acquires a separate string owner. Each is released by its matching owner.
        unsafe {
            let error = crate::ObjectOwner::from_raw(imre_error_take());
            assert_eq!(
                imre_exception_matches(crate::ObjectOwner::as_ptr(&error), kind as u32),
                1
            );
            let message =
                StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(&error)));
            assert_eq!(message.as_str(), expected);
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn integer_mapping_owns_canonical_digits_without_retaining_the_source() {
        for (text, expected) in [
            ("", vec![]),
            ("0123456789", (0..10).collect()),
            ("1٢३４𝟝", vec![1, 2, 3, 4, 5]),
            ("\u{10d40}\u{116da}\u{1ccf7}\u{1e5f9}", vec![0, 0, 7, 8]),
        ] {
            let source = StrOwner::new(Str::new(text));
            let weak = StrOwner::downgrade(&source);
            // SAFETY: source stays rooted during conversion. Each checked,
            // nonnull result transfers one independent list owner to Rc.
            unsafe {
                let result = imre_str_int_list(StrOwner::as_ptr(&source));
                assert!(!result.is_null());
                let result = Rc::from_raw(result);
                let other = imre_str_int_list(StrOwner::as_ptr(&source));
                assert!(!other.is_null());
                let other = Rc::from_raw(other);
                assert!(!Rc::ptr_eq(&result, &other));
                assert_eq!(StrOwner::strong_count(&source), 1);
                drop(source);
                assert!(weak.upgrade().is_none());
                result.0.with(|values| {
                    assert_eq!(values.len(), expected.len());
                    for (value, expected) in values.iter().zip(&expected) {
                        assert_eq!(value.as_small(), Some(*expected));
                    }
                });
                if !expected.is_empty() {
                    assert_eq!(imre_list_set_index(Rc::as_ptr(&result), 0, integer(9)), 1);
                    assert_eq!(
                        Int(imre_list_get_index(Rc::as_ptr(&other), 0)).as_small(),
                        Some(expected[0])
                    );
                }
            }
            assert_eq!(imre_error_pending(), 0);
        }
    }

    #[test]
    fn integer_mapping_reports_the_first_invalid_character_with_exact_repr() {
        for (text, literal) in [
            ("12x3y", "'x'"),
            ("12y3x", "'y'"),
            ("1٢x３y", "'x'"),
            ("1x٢\u{11de0}", "'x'"),
            ("+12", "'+'"),
            ("-12", "'-'"),
            ("1_2", "'_'"),
            ("1 2", "' '"),
            ("12\t3", "'\\t'"),
            ("12\0x", "'\\x00'"),
            ("12'\"", "\"'\""),
            ("12\\x", "'\\\\'"),
            ("12²3", "'²'"),
            ("12\u{301}x", "'\u{301}'"),
            ("１２\u{11de0}x", "'\\U00011de0'"),
            ("\u{88f}1", "'\\u088f'"),
        ] {
            let source = StrOwner::new(Str::new(text));
            // SAFETY: the immutable source remains rooted, and each pending
            // exception is consumed before another recoverable call.
            unsafe {
                assert!(imre_str_int_list(StrOwner::as_ptr(&source)).is_null());
            }
            clear_error(
                Kind::ValueError,
                &format!("invalid literal for int() with base 10: {literal}"),
            );
            assert_eq!(source.as_str(), text);
            assert_eq!(StrOwner::strong_count(&source), 1);
        }
    }

    #[test]
    fn integer_mapping_repeatedly_cleans_private_buffers_and_reuses_successes() {
        for text in ["0123456789".repeat(128), "٠١٢٣٤٥٦٧٨٩".repeat(128)] {
            let source = Str::from(text.clone());
            let invalid = Str::from(format!("{text}\0x"));
            for _ in 0..64 {
                // SAFETY: both strings remain rooted. Successful lists are
                // checked before adoption; failed partial results never escape.
                unsafe {
                    let result = imre_str_int_list(&source);
                    assert!(!result.is_null());
                    let result = Rc::from_raw(result);
                    assert!(imre_str_int_list(&invalid).is_null());
                    clear_error(
                        Kind::ValueError,
                        "invalid literal for int() with base 10: '\\x00'",
                    );
                    result.0.with(|values| {
                        assert_eq!(values.len(), 1280);
                        for (index, value) in values.iter().enumerate() {
                            assert_eq!(value.as_small(), Some((index % 10) as i64));
                        }
                    });
                }
                assert_eq!(imre_error_pending(), 0);
            }
        }
    }

    #[test]
    fn slices_distinguish_omitted_negative_bounds_and_clamp_large_components() {
        for (components, expected) in [
            ([None, None, None], vec![0, 1, 2, 3, 4]),
            ([None, None, Some(-1)], vec![4, 3, 2, 1, 0]),
            ([None, Some(-1), Some(-1)], vec![]),
            ([Some(-100), Some(100), Some(2)], vec![0, 2, 4]),
            ([Some(-1), Some(-6), Some(-2)], vec![4, 2, 0]),
            ([Some(4), Some(1), Some(-1)], vec![4, 3, 2]),
            ([Some(1), Some(4), Some(-1)], vec![]),
            ([Some(100), Some(-100), Some(-2)], vec![4, 2, 0]),
            ([Some(-100), None, Some(-1)], vec![]),
            ([None, Some(100), Some(-1)], vec![]),
        ] {
            assert_eq!(
                indices(5, components.map(|value| value.map(BigInt::from))),
                expected
            );
        }
        let huge = BigInt::from(1_u8) << 400_u32;
        assert_eq!(
            indices(5, [Some(-&huge), Some(huge.clone()), None]),
            vec![0, 1, 2, 3, 4]
        );
        assert_eq!(
            indices(
                5,
                [Some(huge.clone()), Some(-&huge), Some(BigInt::from(-1))]
            ),
            vec![4, 3, 2, 1, 0]
        );
        assert_eq!(indices(5, [None, None, Some(huge.clone())]), vec![0]);
        assert_eq!(indices(5, [None, None, Some(-&huge)]), vec![4]);
        assert!(indices(0, [None, None, Some(-&huge)]).is_empty());
        assert_eq!(
            indices(isize::MAX as usize, [None, None, Some(huge)]),
            vec![0]
        );
        assert_eq!(
            indices(
                isize::MAX as usize,
                [None, None, Some(BigInt::from(isize::MIN))]
            ),
            vec![isize::MAX as usize - 1]
        );
    }

    #[test]
    fn integer_slice_sums_preserve_signed_and_arbitrary_precision_selection() {
        let huge = BigInt::from(1) << 400_u32;
        let mut cases = [
            ([None, None, None], 3),
            ([None, None, Some(-1)], 3),
            ([None, Some(-1), Some(-1)], 0),
            ([Some(-100), Some(100), Some(2)], 6),
            ([Some(-1), Some(-6), Some(-2)], 6),
            ([Some(4), Some(1), Some(-1)], 3),
            ([Some(1), Some(4), Some(-1)], 0),
            ([Some(100), Some(-100), Some(-2)], 6),
            ([Some(-100), None, Some(-1)], 0),
            ([None, Some(100), Some(-1)], 0),
            ([Some(0), Some(2), None], 0),
        ]
        .map(|(components, expected)| (components.map(|value| value.map(BigInt::from)), expected))
        .to_vec();
        cases.extend([
            ([Some(-&huge), Some(huge.clone()), None], 3),
            ([Some(huge.clone()), Some(-&huge), Some((-1).into())], 3),
            ([None, None, Some(huge.clone())], 1),
            ([None, None, Some(-&huge)], 3),
        ]);
        for scale in [BigInt::from(1), huge] {
            let source = List::new(
                [1, -1, 2, -2, 3]
                    .map(|value| Int::new(&scale * value))
                    .into(),
            );
            for (components, expected) in &cases {
                let components = components.clone().map(|value| value.map(Int::new));
                let [start, stop, step] = components
                    .each_ref()
                    .map(|value| value.as_ref().map_or(0, |value| value.0));
                // SAFETY: source and all nullable slice component owners stay
                // rooted. A successful subtotal transfers one integer owner.
                let result = Int(unsafe { imre_list_slice_sum(&source, start, stop, step) });
                assert_eq!(imre_error_pending(), 0);
                assert_eq!(result.big().as_ref(), &(&scale * *expected));
                if *expected == 0 {
                    assert_eq!(result.as_small(), Some(0));
                }
            }
        }
    }

    #[test]
    fn integer_slice_sums_cross_vector_and_remainder_boundaries() {
        for length in [1, 2, 3, 7, 8, 9, 15, 16, 17, 31, 32, 33, 128, 129] {
            for heap_at in [None, Some(0), Some(length / 2), Some(length - 1)] {
                let mut numbers: Vec<BigInt> = (0..length)
                    .map(|at| BigInt::from([i32::MIN, i32::MAX, 1, -1][at % 4]))
                    .collect();
                if let Some(at) = heap_at {
                    numbers[at] = BigInt::from(1) << 400_u32;
                }
                let source = List::new(numbers.iter().cloned().map(Int::new).collect());
                for step in [-2_i64, -1, 1, 2] {
                    let parity = if step < 0 { (length - 1) % 2 } else { 0 };
                    let expected: BigInt = numbers
                        .iter()
                        .enumerate()
                        .filter(|(at, _)| step.abs() == 1 || at % 2 == parity)
                        .map(|(_, value)| value)
                        .sum();
                    // SAFETY: the source and immediate step stay rooted; the
                    // subtotal owns its result across vector and scalar tails.
                    let actual = Int(unsafe { imre_list_slice_sum(&source, 0, 0, integer(step)) });
                    assert_eq!(actual.big().as_ref(), &expected);
                    assert_eq!(imre_error_pending(), 0);
                }
            }
        }
    }

    #[test]
    fn unicode_indexing_and_slicing_use_code_points_and_preserve_error_messages() {
        let source = StrOwner::new(Str::new("aé\0e\u{301}🦀"));
        // SAFETY: every argument stays rooted; each nonnull result transfers one
        // string reference to StrOwner and each failing call clears pending before reuse.
        unsafe {
            for (position, expected) in ["a", "é", "\0", "e", "\u{301}", "🦀"]
                .into_iter()
                .enumerate()
            {
                for index in [position as i64, position as i64 - 6] {
                    let result =
                        StrOwner::from_raw(imre_str_get(StrOwner::as_ptr(&source), integer(index)));
                    assert_eq!(result.as_str(), expected);
                }
            }
            for at in [6, -7] {
                assert!(imre_str_get(StrOwner::as_ptr(&source), integer(at)).is_null());
                clear_error(Kind::IndexError, "string index out of range");
            }
            let huge = Int::new(BigInt::from(1_u8) << 400_u32);
            assert!(imre_str_get(StrOwner::as_ptr(&source), huge.0).is_null());
            clear_error(
                Kind::IndexError,
                "cannot fit 'int' into an index-sized integer",
            );
            for (components, expected) in [
                ([None, None, None], "aé\0e\u{301}🦀"),
                ([None, None, Some(-1)], "🦀\u{301}e\0éa"),
                ([Some(1), Some(5), Some(2)], "ée"),
                ([None, None, Some(2)], "a\0\u{301}"),
                ([Some(5), None, Some(-2)], "🦀eé"),
                ([Some(4), Some(0), Some(-2)], "\u{301}\0"),
                ([Some(1), Some(5), None], "é\0e\u{301}"),
                ([None, Some(-1), Some(-1)], ""),
            ] {
                let [start, stop, step] = components.map(|value| value.map_or(0, integer));
                let result = StrOwner::from_raw(imre_str_slice(
                    StrOwner::as_ptr(&source),
                    start,
                    stop,
                    step,
                ));
                assert_eq!(result.as_str(), expected);
            }
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn zero_slice_step_raises_even_for_empty_sequences_and_recovers() {
        let text = StrOwner::new(Str::new(""));
        // SAFETY: constructors transfer one owner to Rc. All calls borrow these
        // live exact handles, and error pointers are checked before adoption.
        unsafe {
            let list = Rc::from_raw(imre_list_new());
            let sequence = crate::ObjectOwner::from_raw(imre_seq_new(3));
            assert!(imre_str_slice(StrOwner::as_ptr(&text), 0, 0, integer(0)).is_null());
            clear_error(Kind::ValueError, "slice step cannot be zero");
            assert!(imre_list_slice(Rc::as_ptr(&list), 0, 0, integer(0)).is_null());
            clear_error(Kind::ValueError, "slice step cannot be zero");
            assert_eq!(imre_list_slice_sum(Rc::as_ptr(&list), 0, 0, integer(0)), 1);
            clear_error(Kind::ValueError, "slice step cannot be zero");
            assert_eq!(imre_list_slice_sum(Rc::as_ptr(&list), 0, 0, integer(-1)), 1);
            assert_eq!(imre_error_pending(), 0);
            assert!(
                imre_seq_slice(crate::ObjectOwner::as_ptr(&sequence), 0, 0, integer(0)).is_null()
            );
            clear_error(Kind::ValueError, "slice step cannot be zero");
            let empty =
                StrOwner::from_raw(imre_str_slice(StrOwner::as_ptr(&text), 0, 0, integer(-1)));
            assert_eq!(empty.as_str(), "");
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn integer_list_slices_own_selected_values_after_source_mutation() {
        let big = Rc::new(BigInt::from(1_u8) << 400_u32);
        let weak = Rc::downgrade(&big);
        // SAFETY: source and slice are distinct owned lists. Every selected
        // BigInt gains an independent reference, as does each successful read.
        unsafe {
            let source = Rc::from_raw(imre_list_new());
            imre_list_append(Rc::as_ptr(&source), Rc::as_ptr(&big) as usize);
            imre_list_append(Rc::as_ptr(&source), integer(7));
            let slice = Rc::from_raw(imre_list_slice(Rc::as_ptr(&source), 0, 0, integer(-1)));
            let subtotal = Int(imre_list_slice_sum(Rc::as_ptr(&source), 0, 0, integer(-1)));
            assert!(!Rc::ptr_eq(&source, &slice));
            assert_eq!(imre_list_set_index(Rc::as_ptr(&source), 0, integer(9)), 1);
            drop((source, big));
            assert_eq!(weak.strong_count(), 1);
            assert_eq!(
                Int(imre_list_get_index(Rc::as_ptr(&slice), 0)).as_small(),
                Some(7)
            );
            let saved = Int(imre_list_get_index(Rc::as_ptr(&slice), 1));
            drop(slice);
            assert_eq!(saved.big().as_ref(), &(BigInt::from(1_u8) << 400_u32));
            assert_eq!(
                subtotal.big().as_ref(),
                &((BigInt::from(1_u8) << 400_u32) + 7)
            );
            drop(saved);
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn generic_slices_preserve_shallow_aliases_and_nullable_scalar_descriptors() {
        // SAFETY: a kind-4 sequence stores exact List pointers; slice copies own
        // the same child allocation, while sequence buffers stay independent.
        unsafe {
            let child = Rc::from_raw(imre_list_new());
            let weak = Rc::downgrade(&child);
            let source = crate::ObjectOwner::from_raw(imre_seq_new(4));
            imre_seq_append(
                crate::ObjectOwner::as_ptr(&source),
                Rc::as_ptr(&child) as usize,
                4,
            );
            let slice = crate::ObjectOwner::from_raw(imre_seq_slice(
                crate::ObjectOwner::as_ptr(&source),
                0,
                0,
                0,
            ));
            assert!(!crate::ObjectOwner::ptr_eq(&source, &slice));
            drop(source);
            imre_list_append(Rc::as_ptr(&child), integer(17));
            drop(child);
            let saved = Rc::from_raw(
                imre_seq_get_index(crate::ObjectOwner::as_ptr(&slice), 0) as *const List
            );
            drop(slice);
            assert_eq!(Int(imre_list_len(Rc::as_ptr(&saved))).as_small(), Some(1));
            assert_eq!(
                Int(imre_list_get_index(Rc::as_ptr(&saved), 0)).as_small(),
                Some(17)
            );
            drop(saved);
            assert!(weak.upgrade().is_none());

            for (kind, values) in [
                (0, vec![0, 0]),
                (1, vec![0, 1]),
                (257, vec![0, 1, 2]),
                (258, vec![0, integer(9)]),
            ] {
                let source = crate::ObjectOwner::from_raw(imre_seq_new(kind));
                for &value in &values {
                    imre_seq_append(crate::ObjectOwner::as_ptr(&source), value, kind);
                }
                let slice = crate::ObjectOwner::from_raw(imre_seq_slice(
                    crate::ObjectOwner::as_ptr(&source),
                    0,
                    0,
                    integer(-1),
                ));
                drop(source);
                assert_eq!(
                    slice.0.with(|storage| storage
                        .sequence()
                        .ok()
                        .expect("fixture sequence")
                        .kind
                        .raw()),
                    kind
                );
                for (at, &expected) in values.iter().rev().enumerate() {
                    assert_eq!(
                        imre_seq_get_index(crate::ObjectOwner::as_ptr(&slice), at),
                        expected
                    );
                }
            }
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn string_iterator_roots_its_source_and_yields_independent_owned_code_points() {
        let source = StrOwner::new(Str::new("aé\0e\u{301}🦀"));
        let weak = StrOwner::downgrade(&source);
        // SAFETY: iterator creation acquires the source; every output is adopted
        // once before reusing its slot. Exhausted calls must not touch outputs.
        unsafe {
            let iterator = crate::ObjectOwner::from_raw(imre_str_iter(StrOwner::as_ptr(&source)));
            let indexed = StrOwner::from_raw(imre_str_get(StrOwner::as_ptr(&source), integer(1)));
            drop(source);
            assert_eq!(weak.strong_count(), 1);
            let mut output = 73;
            let mut unused = 91;
            let mut values = Vec::new();
            while imre_mapping_iter_next(
                crate::ObjectOwner::as_ptr(&iterator),
                &mut output,
                &mut unused,
            ) != 0
            {
                values.push(StrOwner::from_raw(output as *const Str));
                assert_eq!(unused, 91);
            }
            output = 73;
            assert_eq!(
                imre_mapping_iter_next(
                    crate::ObjectOwner::as_ptr(&iterator),
                    &mut output,
                    std::ptr::null_mut()
                ),
                0
            );
            assert_eq!(output, 73);
            drop(iterator);
            assert!(weak.upgrade().is_none());
            assert!(StrOwner::ptr_eq(&indexed, &values[1]));
            assert_eq!(
                values
                    .iter()
                    .map(|value| value.as_str())
                    .collect::<Vec<_>>(),
                ["a", "é", "\0", "e", "\u{301}", "🦀"]
            );
        }
        assert_eq!(imre_error_pending(), 0);
    }
}
