//! Immutable byte values with exact ownership, Python repr, and strict UTF-8.

use std::rc::Rc;

use num_traits::ToPrimitive;
use ruff_python_literal::escape::{AsciiEscape, Escape};

use super::exceptions::{Error, Kind, Result, finish};
use super::sequences::SliceIndices;
use super::words::{OwnedWord, WordKind};
use super::{Bytes, Int, List, Object, Storage, StorageCell, Str, acquire_rc, fail};

/// Reserves an entire immutable result before copying any bytes into it.
fn allocate(length: usize) -> Result<Bytes> {
    let mut value = Vec::new();
    value
        .try_reserve_exact(length)
        .map_err(|_| Error::new(Kind::MemoryError, "bytes allocation failed"))?;
    Ok(value)
}

/// Converts a Python index-sized argument without truncating large integers.
fn native_size(value: &Int) -> Result<isize> {
    value
        .as_small()
        .map(|value| value as isize)
        .or_else(|| value.big().to_isize())
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "cannot fit 'int' into an index-sized integer",
            )
        })
}

/// Uses Ruff's byte repr rules, including quote choice and lowercase hex escapes.
pub(super) fn repr(value: &[u8]) -> Str {
    let escaped = AsciiEscape::new_repr(value);
    let quote = escaped.layout().quote.as_char();
    let mut result = String::from("b");
    result.push(quote);
    escaped
        .write_body(&mut result)
        .expect("writing to a String cannot fail");
    result.push(quote);
    Str::from(result)
}

/// # Safety
/// `data` points to `length` initialized bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_literal(data: *const u8, length: usize) -> *const Bytes {
    // SAFETY: the compiler passes a live constant byte array, including for length zero.
    let data = unsafe { std::slice::from_raw_parts(data, length) };
    let mut value =
        allocate(length).unwrap_or_else(|_| fail("MemoryError", "bytes allocation failed"));
    value.extend_from_slice(data);
    Rc::into_raw(Rc::new(value))
}

/// # Safety
/// The caller roots one bytes allocation, or passes null for an absent optional.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_retain(value: *const Bytes) {
    if !value.is_null() {
        // SAFETY: the rooted pointer identifies an exact Rc<Bytes> allocation.
        unsafe { Rc::increment_strong_count(value) };
    }
}

/// # Safety
/// Transfers one bytes owner, or accepts null during nullable/partial cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_release(value: *const Bytes) {
    if !value.is_null() {
        // SAFETY: the caller transfers one exact Rc<Bytes> reference.
        unsafe { Rc::decrement_strong_count(value) };
    }
}

/// # Safety
/// The immutable bytes value remains rooted throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_str(value: *const Bytes) -> *const Str {
    // SAFETY: the rooted input is immutable and repr retains no borrowed data.
    Rc::into_raw(Rc::new(repr(unsafe { &*value })))
}

/// # Safety
/// The immutable bytes value remains rooted throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_print_bytes(value: *const Bytes) {
    // SAFETY: the rooted input is immutable and repr retains no borrowed data.
    let text = repr(unsafe { &*value });
    crate::output_bytes(text.as_bytes());
}

/// # Safety
/// The immutable bytes value remains rooted throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_len(value: *const Bytes) -> usize {
    // SAFETY: the input's allocation cannot change while rooted.
    Int::from_usize(unsafe { &*value }.len()).into_raw()
}

/// Tests emptiness without constructing a tagged length.
///
/// # Safety
/// The immutable bytes value remains rooted throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_truthy(value: *const Bytes) -> u8 {
    // SAFETY: the input's allocation cannot change while rooted.
    u8::from(!unsafe { &*value }.is_empty())
}

/// Returns a signed ordering result (-1, 0, 1) for unsigned byte lexicographic order.
///
/// # Safety
/// Both exact immutable byte handles remain rooted throughout the comparison.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_compare(left: *const Bytes, right: *const Bytes) -> i32 {
    // SAFETY: both immutable inputs remain alive and no borrow escapes.
    unsafe { (&*left).cmp(&*right) as i32 }
}

/// # Safety
/// Root the bytes value and exact integer index; pending error state is clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_get(value: *const Bytes, at: usize) -> usize {
    // SAFETY: the immutable bytes allocation remains rooted throughout the read.
    let value = unsafe { &*value };
    // The sign check matters even with bounds checking: an unsigned shift of a
    // negative tagged index could fit an exceptionally large bytes allocation.
    if at & 1 != 0
        && (at as isize) >= 0
        && let Some(&byte) = value.get(at >> 1)
    {
        return (usize::from(byte) << 1) | 1;
    }
    // SAFETY: the caller roots the exact canonical integer and bytes value.
    unsafe { bytes_get_slow(value, at) }
}

/// # Safety
/// `value` roots immutable bytes and `at` is an exact nonnegative native index,
/// not a tagged integer. Enter with no pending error. The result is a canonical
/// tagged byte, or zero with the same IndexError as ordinary bytes indexing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_get_index(value: *const Bytes, at: usize) -> usize {
    // SAFETY: the immutable bytes allocation remains rooted throughout the read.
    if let Some(&byte) = unsafe { &*value }.get(at) {
        (usize::from(byte) << 1) | 1
    } else {
        bytes_index_error(at)
    }
}

/// Preserves index-sized conversion errors without expanding the checked load.
#[cold]
#[inline(never)]
fn bytes_index_error(at: usize) -> usize {
    let message = if at > isize::MAX as usize {
        "cannot fit 'int' into an index-sized integer"
    } else {
        "index out of range"
    };
    Error::new(Kind::IndexError, message).returning(0)
}

/// Keeps negative/BigInt normalization and exception construction outside the
/// small checked-load wrapper imported into native caller loops.
///
/// # Safety
/// `at` is a rooted canonical integer handle and pending error state is clear.
#[cold]
#[inline(never)]
unsafe fn bytes_get_slow(value: &Bytes, at: usize) -> usize {
    // SAFETY: the exact integer remains rooted until the result has been copied.
    let at = unsafe { Int::borrow(at) };
    let result = (|| {
        let at = native_size(&at).map_err(|_| {
            Error::new(
                Kind::IndexError,
                "cannot fit 'int' into an index-sized integer",
            )
        })?;
        let at = if at < 0 {
            at + value.len() as isize
        } else {
            at
        };
        let byte = usize::try_from(at)
            .ok()
            .and_then(|at| value.get(at))
            .ok_or_else(|| Error::new(Kind::IndexError, "index out of range"))?;
        Ok(Int::from_usize(usize::from(*byte)).into_raw())
    })();
    finish(result, 0)
}

/// # Safety
/// Root the bytes value and nonzero integer bounds; zero means None/omitted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_slice(
    value: *const Bytes,
    start: usize,
    stop: usize,
    step: usize,
) -> *const Bytes {
    // SAFETY: all source owners remain rooted through normalization and copying.
    let result = unsafe {
        let value = &*value;
        SliceIndices::new(value.len(), start, stop, step).and_then(|slice| {
            let mut result = allocate(slice.len())?;
            result.extend(slice.map(|at| value[at]));
            Ok(Rc::into_raw(Rc::new(result)))
        })
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// Root the integer count; pending error state must be clear on entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_count(count: usize) -> *const Bytes {
    // SAFETY: the count is an exact canonical integer rooted by the caller.
    let count = unsafe { Int::borrow(count) };
    let result = (|| {
        let count = native_size(&count)?;
        if count < 0 {
            return Err(Error::new(Kind::ValueError, "negative count"));
        }
        let mut value = allocate(count as usize)?;
        value.resize(count as usize, 0);
        Ok(Rc::into_raw(Rc::new(value)))
    })();
    finish(result, std::ptr::null())
}

/// # Safety
/// Root an exact list[int]; no mutation or callback can occur during conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_list(value: *const List) -> *const Bytes {
    // SAFETY: the rooted list is borrowed only until the immutable result is owned.
    let result = unsafe {
        (&*value).0.with(|values| {
            let mut result = allocate(values.len())?;
            for value in values {
                let byte = value
                    .as_small()
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or_else(|| {
                        Error::new(Kind::ValueError, "bytes must be in range(0, 256)")
                    })?;
                result.push(byte);
            }
            Ok(Rc::into_raw(Rc::new(result)))
        })
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// Both immutable byte inputs remain rooted; the result owns independent storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_concat(
    left: *const Bytes,
    right: *const Bytes,
) -> *const Bytes {
    // SAFETY: both exact immutable inputs remain rooted throughout the copy.
    let (left, right) = unsafe { (&*left, &*right) };
    let result = (|| {
        let length = left
            .len()
            .checked_add(right.len())
            .ok_or_else(|| Error::new(Kind::MemoryError, "bytes allocation failed"))?;
        let mut result = allocate(length)?;
        result.extend_from_slice(left);
        result.extend_from_slice(right);
        Ok(Rc::into_raw(Rc::new(result)))
    })();
    finish(result, std::ptr::null())
}

/// # Safety
/// Root the byte sequence and exact integer count; pending error state is clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_repeat(value: *const Bytes, count: usize) -> *const Bytes {
    // SAFETY: both exact immutable inputs remain rooted throughout repetition.
    let (value, count) = unsafe { (&*value, Int::borrow(count)) };
    let result = (|| {
        // Conversion precedes the empty-input/negative-count fast path in Python.
        let count = native_size(&count)?.max(0) as usize;
        let length = value
            .len()
            .checked_mul(count)
            .filter(|length| *length <= isize::MAX as usize)
            .ok_or_else(|| Error::new(Kind::OverflowError, "repeated bytes are too long"))?;
        let mut result = allocate(length)?;
        if !value.is_empty() {
            for _ in 0..count {
                result.extend_from_slice(value);
            }
        }
        Ok(Rc::into_raw(Rc::new(result)))
    })();
    finish(result, std::ptr::null())
}

/// # Safety
/// Root an exact valid-UTF-8 string; returned bytes own a copy of its encoding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_encode(value: *const Str) -> *const Bytes {
    // SAFETY: the input is rooted and its encoded bytes remain readable for the copy.
    let value = unsafe { &*value };
    // SAFETY: the UTF-8 byte slice satisfies the literal helper's initialized range.
    unsafe { relar_bytes_literal(value.as_ptr(), value.len()) }
}

/// Renders Python's strict UTF-8 decode error from the invalid sequence bounds.
fn decode_error(value: &[u8], error: std::str::Utf8Error) -> Error {
    let start = error.valid_up_to();
    let end = error
        .error_len()
        .map_or(value.len(), |length| start + length);
    let reason = if error.error_len().is_none() {
        "unexpected end of data"
    } else if !(0xc2..=0xf4).contains(&value[start]) {
        "invalid start byte"
    } else {
        "invalid continuation byte"
    };
    let location = if end == start + 1 {
        format!("byte 0x{:02x} in position {start}", value[start])
    } else {
        format!("bytes in position {start}-{}", end - 1)
    };
    Error::new(
        Kind::UnicodeDecodeError,
        format!("'utf-8' codec can't decode {location}: {reason}"),
    )
}

/// # Safety
/// Root exact bytes; invalid UTF-8 returns null and sets UnicodeDecodeError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_decode(value: *const Bytes) -> *const Str {
    // SAFETY: immutable bytes remain rooted throughout decoding and error rendering.
    let value = unsafe { &*value };
    let result = std::str::from_utf8(value)
        .map(|text| Rc::into_raw(Rc::new(Str::from(text))))
        .map_err(|error| decode_error(value, error));
    finish(result, std::ptr::null())
}

/// A cursor owns its immutable input independently of the user's source binding.
pub(super) struct BytesIterator {
    owner: Rc<Bytes>,
    position: usize,
}

impl BytesIterator {
    pub(super) fn next(&mut self) -> Option<(OwnedWord, Option<OwnedWord>)> {
        let byte = *self.owner.get(self.position)?;
        self.position += 1;
        let word = Int::from_usize(usize::from(byte)).into_raw();
        // SAFETY: an immediate integer transfers its canonical owned word.
        let value = unsafe { OwnedWord::from_owned(word, WordKind::new(2)) };
        Some((value, None))
    }
}

/// # Safety
/// Root exact bytes; release the returned cursor on every loop exit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_bytes_iter(value: *const Bytes) -> *const Object {
    // SAFETY: the rooted source acquires an independent owner for the cursor.
    let owner = unsafe { acquire_rc(value as usize) };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::BytesIterator(
        BytesIterator { owner, position: 0 },
    )))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::exceptions::{
        relar_error_pending, relar_error_take, relar_exception_matches, relar_exception_message,
    };
    use crate::{BigInt, MAX_SMALL, MIN_SMALL};

    /// Consumes one indexing failure and checks the exact public diagnostic.
    fn assert_index_error(expected: &str) {
        let error = relar_error_take();
        assert!(!error.is_null());
        // SAFETY: taking the pending exception transfers one live owner, and
        // message extraction returns another independently owned string.
        unsafe {
            let error = Rc::from_raw(error);
            assert_eq!(
                relar_exception_matches(Rc::as_ptr(&error), Kind::IndexError as u32),
                1
            );
            let message = Rc::from_raw(relar_exception_message(Rc::as_ptr(&error)));
            assert_eq!(message.as_str(), expected);
        }
        assert_eq!(relar_error_pending(), 0);
    }

    #[test]
    fn indexed_bytes_preserve_tagged_zero_signed_bounds_and_bigint_errors() {
        let bytes: Bytes = (0..=u8::MAX).collect();
        assert_eq!(relar_error_pending(), 0);
        // SAFETY: bytes and each canonical integer index remain rooted, and
        // every error below is consumed before entering another indexed read.
        unsafe {
            for byte in 0..=u8::MAX {
                let positive = Int::from_usize(usize::from(byte));
                let negative = Int::small(i64::from(byte) - 256).unwrap();
                let expected = (usize::from(byte) << 1) | 1;
                assert_eq!(relar_bytes_get(&bytes, positive.0), expected);
                assert_eq!(relar_bytes_get(&bytes, negative.0), expected);
                assert_eq!(relar_bytes_get_index(&bytes, usize::from(byte)), expected);
                assert_eq!(relar_error_pending(), 0);
            }
            let huge = BigInt::from(1_u8) << 100_u32;
            let errors = [
                (Int::from_usize(256), "index out of range"),
                (Int::small(-257).unwrap(), "index out of range"),
                (Int::small(MIN_SMALL).unwrap(), "index out of range"),
                (Int::small(MAX_SMALL).unwrap(), "index out of range"),
                (Int::new(BigInt::from(MAX_SMALL) + 1), "index out of range"),
                (Int::new(BigInt::from(isize::MIN)), "index out of range"),
                (
                    Int::new(huge.clone()),
                    "cannot fit 'int' into an index-sized integer",
                ),
                (
                    Int::new(-huge),
                    "cannot fit 'int' into an index-sized integer",
                ),
            ];
            for (index, expected) in errors {
                assert_eq!(relar_bytes_get(&bytes, index.0), 0);
                assert_index_error(expected);
                assert_eq!(relar_bytes_get(&bytes, 1), 1);
            }
            for (index, expected) in [
                (bytes.len(), "index out of range"),
                (isize::MAX as usize, "index out of range"),
                (
                    isize::MAX as usize + 1,
                    "cannot fit 'int' into an index-sized integer",
                ),
                (usize::MAX, "cannot fit 'int' into an index-sized integer"),
            ] {
                assert_eq!(relar_bytes_get_index(&bytes, index), 0);
                assert_index_error(expected);
                assert_eq!(relar_bytes_get_index(&bytes, 0), 1);
            }
            assert_eq!(relar_bytes_get(&Bytes::new(), 1), 0);
            assert_index_error("index out of range");
            assert_eq!(relar_bytes_get_index(&Bytes::new(), 0), 0);
            assert_index_error("index out of range");
        }
    }
}
