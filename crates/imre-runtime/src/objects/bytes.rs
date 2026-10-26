//! Immutable byte values with exact ownership, Python repr, and strict UTF-8.

use crate::StrOwner;

use num_traits::ToPrimitive;
use ruff_python_literal::escape::{AsciiEscape, Escape};

use super::exceptions::{Error, Kind, Result, finish};
use super::sequences::SliceIndices;
use super::words::{OwnedWord, WordKind};
use super::{Bytes, Int, List, Object, Storage, StorageCell, Str, fail};

/// Reserves an entire immutable result before copying any bytes into it.
pub(super) fn allocate(length: usize) -> Result<Bytes> {
    let mut value = Vec::new();
    value
        .try_reserve_exact(length)
        .map_err(|_| Error::new(Kind::MemoryError, "bytes allocation failed"))?;
    Ok(value)
}

/// Private append-only integer list whose only consumer converts it to bytes.
pub(super) struct ByteOutput {
    bytes: Vec<u8>,
    invalid: bool,
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_byte_output_new() -> *const Object {
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::ByteOutput(ByteOutput {
            bytes: Vec::new(),
            invalid: false,
        }),
    ))))
}

/// Hints at most 256 bytes per private buffer. A loop can break or raise before
/// filling its output, and a failed hint must leave the next append unchanged.
///
/// # Safety
/// The builder is rooted or null. Root all three canonical range arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_byte_output_reserve_range(
    output: *const Object,
    start: usize,
    stop: usize,
    step: usize,
    per_iteration: usize,
) {
    if output.is_null() {
        return;
    }
    // SAFETY: the caller supplies the same captured operands as the range loop.
    let Some(trips) = (unsafe { crate::small_range_trips(start, stop, step) }) else {
        return;
    };
    let capacity = trips.saturating_mul(per_iteration).min(256);
    // SAFETY: the private output is rooted and no storage borrow escapes.
    unsafe {
        super::object(output).0.with_mut(|storage| {
            let Storage::ByteOutput(output) = storage else {
                unreachable!()
            };
            if output.bytes.is_empty() && output.bytes.capacity() < capacity {
                let _ = output.bytes.try_reserve_exact(capacity);
            }
        });
    }
}

/// # Safety
/// Root the private output and its canonical integer arguments for this call.
unsafe fn append_bytes<const N: usize>(output: *const Object, words: [usize; N]) -> i32 {
    // SAFETY: only compiler-generated appends access this rooted private object.
    unsafe {
        super::object(output).0.with_mut(|storage| {
            let Storage::ByteOutput(output) = storage else {
                unreachable!()
            };
            let count = N.min(super::MAX_LIST_LEN - output.bytes.len());
            let writable = if output.bytes.try_reserve(count).is_ok() {
                count
            } else {
                // A fused pair can still commit its first Python append in a spare slot.
                count.min(output.bytes.capacity() - output.bytes.len())
            };
            for (index, word) in words.into_iter().enumerate() {
                if index == writable {
                    super::exceptions::sequence_allocation_failed();
                    return 0;
                }
                // Canonical 0..=255 are exactly the odd words in 1..=511.
                // Invalid Ints have no observable destructor. Record the error
                // without raising: a later producer exception must still win.
                output.invalid |= word & !0x1fe != 1;
                output.bytes.push((word >> 1) as u8);
            }
            1
        })
    }
}

/// # Safety
/// Root the private output and the exact integer throughout the append.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_byte_output_append(output: *const Object, word: usize) -> i32 {
    // SAFETY: the caller roots both the output and the canonical integer.
    unsafe { append_bytes(output, [word]) }
}

/// # Safety
/// Root the private output and both already-evaluated exact integers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_byte_output_append_pair(
    output: *const Object,
    first: usize,
    second: usize,
) -> i32 {
    // SAFETY: the caller roots the output and both canonical integers.
    unsafe { append_bytes(output, [first, second]) }
}

/// # Safety
/// Root the private output at its sole consuming bytes() expression.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_byte_output_finish(output: *const Object) -> *const Bytes {
    // SAFETY: no Python-visible reference can observe the buffer transfer.
    let result = unsafe {
        super::object(output).0.with_mut(|storage| {
            let Storage::ByteOutput(output) = storage else {
                unreachable!()
            };
            if output.invalid {
                return Err(Error::new(
                    Kind::ValueError,
                    "bytes must be in range(0, 256)",
                ));
            }
            Ok(crate::BytesOwner::into_raw(crate::BytesOwner::new(
                std::mem::take(&mut output.bytes),
            )))
        })
    };
    finish(result, std::ptr::null())
}

/// Converts a Python index-sized argument without truncating large integers.
pub(super) fn native_size(value: &Int) -> Result<isize> {
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
pub(super) fn repr(value: &[u8]) -> Result<Str> {
    let mut output = String::new();
    write_quoted_repr(&mut super::values::ReprWriter(&mut output), value)
        .map_err(|_| Error::allocation())?;
    Ok(Str::from(output))
}

pub(super) fn write_repr(output: &mut String, value: &[u8]) {
    write_quoted_repr(output, value).expect("writing to a String cannot fail");
}

pub(super) fn write_quoted_repr(
    output: &mut impl super::values::ReprTarget,
    value: &[u8],
) -> std::fmt::Result {
    let escaped = AsciiEscape::new_repr(value);
    let layout = escaped.layout();
    use std::fmt::Write as _;
    let size = layout
        .len
        .and_then(|body| body.checked_add(3))
        .ok_or(std::fmt::Error)?;
    let output = output.quoted_buffer(size)?;
    let quote = layout.quote.as_char();
    output.write_char('b')?;
    output.write_char(quote)?;
    escaped.write_body(output)?;
    output.write_char(quote)
}

/// # Safety
/// `data` points to `length` initialized bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_literal(data: *const u8, length: usize) -> *const Bytes {
    // SAFETY: the compiler passes a live constant byte array, including for length zero.
    let data = unsafe { std::slice::from_raw_parts(data, length) };
    let mut value =
        allocate(length).unwrap_or_else(|_| fail("MemoryError", "bytes allocation failed"));
    value.extend_from_slice(data);
    crate::BytesOwner::into_raw(crate::BytesOwner::new(value))
}

/// # Safety
/// The caller roots one bytes allocation, or passes null for an absent optional.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_retain(value: *const Bytes) {
    if !value.is_null() {
        // SAFETY: the rooted pointer identifies an exact crate::BytesOwner allocation.
        unsafe { retained(value) };
    }
}

/// Returns a new owner of the caller's rooted immutable bytes.
unsafe fn retained(value: *const Bytes) -> *const Bytes {
    // SAFETY: the caller roots this exact crate::BytesOwner allocation.
    unsafe { crate::retain_bytes(value) };
    value
}

/// # Safety
/// Transfers one bytes owner, or accepts null during nullable/partial cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_release(value: *const Bytes) {
    if !value.is_null() {
        // SAFETY: the caller transfers one exact crate::BytesOwner reference.
        unsafe { crate::release_bytes(value) };
    }
}

/// # Safety
/// The immutable bytes value remains rooted throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_str(value: *const Bytes) -> *const Str {
    // SAFETY: the rooted input is immutable and repr retains no borrowed data.
    finish(
        repr(unsafe { &*value }).and_then(|text| {
            StrOwner::try_new(text)
                .map(StrOwner::into_raw)
                .map_err(|_| Error::allocation())
        }),
        std::ptr::null(),
    )
}

/// # Safety
/// The immutable bytes value remains rooted throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_print_bytes(value: *const Bytes) {
    // SAFETY: the rooted input is immutable and repr retains no borrowed data.
    match repr(unsafe { &*value }) {
        Ok(text) => crate::output_text(&text),
        Err(error) => error.raise(),
    }
}

/// # Safety
/// The immutable bytes value remains rooted throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_len(value: *const Bytes) -> usize {
    // SAFETY: the input's allocation cannot change while rooted.
    Int::from_usize(unsafe { &*value }.len()).into_raw()
}

/// Tests emptiness without constructing a tagged length.
///
/// # Safety
/// The immutable bytes value remains rooted throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_truthy(value: *const Bytes) -> u32 {
    // SAFETY: the input's allocation cannot change while rooted.
    u32::from(!unsafe { &*value }.is_empty())
}

/// Returns a signed ordering result (-1, 0, 1) for unsigned byte lexicographic order.
///
/// # Safety
/// Both exact immutable byte handles remain rooted throughout the comparison.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_compare(left: *const Bytes, right: *const Bytes) -> i32 {
    // SAFETY: both immutable inputs remain alive and no borrow escapes.
    unsafe { (&*left).cmp(&*right) as i32 }
}

/// Tests for an immutable subsequence; the empty needle matches even empty bytes.
///
/// # Safety
/// Both byte handles remain rooted throughout the search.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_contains(needle: *const Bytes, haystack: *const Bytes) -> u32 {
    // SAFETY: both byte buffers are immutable and rooted for the call.
    u32::from(unsafe { memchr::memmem::find(&*haystack, &*needle) }.is_some())
}

/// Integer needles are byte values, including on an empty haystack, not indices.
///
/// # Safety
/// Root the canonical integer and immutable bytes; pending error state is clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_contains_int(needle: usize, haystack: *const Bytes) -> u32 {
    // Canonical integers outside the tagged small range cannot represent a byte.
    // SAFETY: the caller roots the exact canonical integer until this call returns.
    let byte = unsafe { Int::borrow(needle) }
        .as_small()
        .and_then(|n| u8::try_from(n).ok());
    let Some(byte) = byte else {
        return finish(
            Err(Error::new(
                Kind::ValueError,
                "byte must be in range(0, 256)",
            )),
            0,
        );
    };
    // SAFETY: the caller keeps the immutable haystack alive.
    u32::from(memchr::memchr(byte, unsafe { &*haystack }).is_some())
}

/// # Safety
/// Root the bytes value and exact integer index; pending error state is clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_get(value: *const Bytes, at: usize) -> usize {
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
pub unsafe extern "C" fn imre_bytes_get_index(value: *const Bytes, at: usize) -> usize {
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
pub unsafe extern "C" fn imre_bytes_slice(
    value: *const Bytes,
    start: usize,
    stop: usize,
    step: usize,
) -> *const Bytes {
    // SAFETY: all source owners remain rooted through normalization and copying.
    let result = unsafe {
        let bytes = &*value;
        SliceIndices::new(bytes.len(), start, stop, step).and_then(|slice| {
            if slice.is_full_forward(bytes.len()) {
                return Ok(retained(value));
            }
            let mut result = allocate(slice.len())?;
            result.extend(slice.map(|at| bytes[at]));
            Ok(crate::BytesOwner::into_raw(crate::BytesOwner::new(result)))
        })
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// Root the integer count; pending error state must be clear on entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_count(count: usize) -> *const Bytes {
    // SAFETY: the count is an exact canonical integer rooted by the caller.
    let count = unsafe { Int::borrow(count) };
    let result = (|| {
        let count = native_size(&count)?;
        if count < 0 {
            return Err(Error::new(Kind::ValueError, "negative count"));
        }
        let mut value = allocate(count as usize)?;
        value.resize(count as usize, 0);
        Ok(crate::BytesOwner::into_raw(crate::BytesOwner::new(value)))
    })();
    finish(result, std::ptr::null())
}

/// # Safety
/// Root an exact list[int]; no mutation or callback can occur during conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_list(value: *const List) -> *const Bytes {
    // SAFETY: the rooted list is borrowed only until the immutable result is owned.
    let result = unsafe {
        (&*value).0.with(|values| {
            let mut result = allocate(values.len())?;
            for (output, input) in result.spare_capacity_mut()[..values.len()]
                .chunks_mut(64)
                .zip(values.chunks(64))
            {
                // Canonical tagged bytes have bit zero set and no bits above
                // bit eight. This reduction permits vectorized validation;
                // chunking bounds speculative work before an invalid element.
                let mut invalid = 0;
                for (slot, value) in output.iter_mut().zip(input) {
                    invalid |= value.0 ^ 1;
                    slot.write((value.0 >> 1) as u8);
                }
                if invalid & !0x1fe != 0 {
                    // Every invalid exact Int has this same error; no value
                    // conversion can call user code or dereference a heap word.
                    return Err(Error::new(
                        Kind::ValueError,
                        "bytes must be in range(0, 256)",
                    ));
                }
            }
            // SAFETY: equal-length chunk pairs initialized every output byte.
            // An error leaves only unpublished u8 values, which need no cleanup.
            result.set_len(values.len());
            Ok(crate::BytesOwner::into_raw(crate::BytesOwner::new(result)))
        })
    };
    finish(result, std::ptr::null())
}

/// Copy two byte slices into one owned result after checking the total allocation.
pub(crate) fn concat_bytes(left: &[u8], right: &[u8]) -> Result<Bytes> {
    let length = left
        .len()
        .checked_add(right.len())
        .ok_or_else(|| Error::new(Kind::MemoryError, "bytes allocation failed"))?;
    let mut result = allocate(length)?;
    result.extend_from_slice(left);
    result.extend_from_slice(right);
    Ok(result)
}

/// # Safety
/// Both immutable byte inputs remain rooted; the caller owns the result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_concat(
    left: *const Bytes,
    right: *const Bytes,
) -> *const Bytes {
    // SAFETY: both exact immutable inputs remain rooted throughout the copy.
    let (left_bytes, right_bytes) = unsafe { (&*left, &*right) };
    if left_bytes.is_empty() || right_bytes.is_empty() {
        // SAFETY: the nonempty operand, or either empty operand, remains rooted.
        return unsafe { retained(if left_bytes.is_empty() { right } else { left }) };
    }
    let result = concat_bytes(left_bytes, right_bytes)
        .map(|bytes| crate::BytesOwner::into_raw(crate::BytesOwner::new(bytes)));
    finish(result, std::ptr::null())
}

/// # Safety
/// Root the byte sequence and exact integer count; pending error state is clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_repeat(value: *const Bytes, count: usize) -> *const Bytes {
    // SAFETY: both exact immutable inputs remain rooted throughout repetition.
    let (bytes, count) = unsafe { (&*value, Int::borrow(count)) };
    let result = (|| {
        // Conversion precedes the empty-input/negative-count fast path in Python.
        let count = native_size(&count)?.max(0) as usize;
        if count == 1 || bytes.is_empty() {
            // SAFETY: repetition preserves this rooted immutable input.
            return Ok(unsafe { retained(value) });
        }
        let length = bytes
            .len()
            .checked_mul(count)
            .filter(|length| *length <= isize::MAX as usize)
            .ok_or_else(|| Error::new(Kind::OverflowError, "repeated bytes are too long"))?;
        let mut result = allocate(length)?;
        for _ in 0..count {
            result.extend_from_slice(bytes);
        }
        Ok(crate::BytesOwner::into_raw(crate::BytesOwner::new(result)))
    })();
    finish(result, std::ptr::null())
}

/// # Safety
/// Root an exact valid-UTF-8 string; returned bytes own a copy of its encoding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_encode(value: *const Str) -> *const Bytes {
    // SAFETY: the input is rooted and its encoded bytes remain readable for the copy.
    let value = unsafe { &*value };
    // SAFETY: the UTF-8 byte slice satisfies the literal helper's initialized range.
    unsafe { imre_bytes_literal(value.as_ptr(), value.len()) }
}

/// A borrowed file buffer must become the immutable input in a codec exception.
pub(super) fn owned_input(value: &[u8]) -> Result<crate::BytesOwner> {
    let mut bytes = allocate(value.len())?;
    bytes.extend_from_slice(value);
    crate::BytesOwner::try_new(bytes).map_err(|_| Error::memory_error())
}

/// A borrowed slice must become the immutable byte object in the exception arguments.
pub(super) fn decode_error(value: &[u8], error: std::str::Utf8Error) -> Error {
    match owned_input(value) {
        Ok(bytes) => decode_owned_error(bytes, error),
        Err(error) => error,
    }
}

pub(super) fn ascii_decode_error(bytes: crate::BytesOwner, index: usize) -> Error {
    let byte = bytes[index];
    let reason = "ordinal not in range(128)";
    Error::unicode_decode(
        "ascii",
        bytes,
        index..index + 1,
        reason,
        format!("'ascii' codec can't decode byte 0x{byte:02x} in position {index}: {reason}"),
    )
}

/// Most decoders already own the immutable input. Retain it only on the failing path.
pub(super) fn decode_owned_error(value: crate::BytesOwner, error: std::str::Utf8Error) -> Error {
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
    Error::unicode_decode(
        "utf-8",
        value,
        start..end,
        reason,
        format!("'utf-8' codec can't decode {location}: {reason}"),
    )
}

/// # Safety
/// Root exact bytes; invalid UTF-8 returns null and sets UnicodeDecodeError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_decode(value: *const Bytes) -> *const Str {
    // SAFETY: immutable bytes remain rooted throughout decoding and error rendering.
    let bytes = unsafe { &*value };
    let result = std::str::from_utf8(bytes)
        .map(|text| StrOwner::into_raw(StrOwner::new(Str::from(text))))
        // SAFETY: this caller still roots the exact byte object passed to the failing decode.
        .map_err(|error| {
            // SAFETY: this exact bytes input remains rooted; acquire takes an owner for the error.
            decode_owned_error(unsafe { crate::acquire_bytes(value as usize) }, error)
        });
    finish(result, std::ptr::null())
}

/// Decode strict ASCII, reporting the first offending byte as CPython does.
///
/// # Safety
/// The caller roots the immutable input through validation and copying.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_decode_ascii(value: *const Bytes) -> *const Str {
    // SAFETY: the exact bytes remain owned by the caller.
    let data = unsafe { &*value };
    let result = if let Some(index) = data.iter().position(|b| !b.is_ascii()) {
        // SAFETY: the original immutable bytes are still owned by this caller.
        Err(ascii_decode_error(
            unsafe { crate::acquire_bytes(value as usize) },
            index,
        ))
    } else {
        let text = std::str::from_utf8(data).expect("ASCII is valid UTF-8");
        Str::try_new(text)
            .map(|s| StrOwner::into_raw(StrOwner::new(s)))
            .map_err(|_| Error::allocation())
    };
    finish(result, std::ptr::null())
}

/// Splits a strict UTF-8 decode without copying the whole input.
///
/// # Safety
/// The exact bytes remain rooted through validation and splitting.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_decode_splitlines(value: *const Bytes) -> *const Object {
    // SAFETY: the caller roots the immutable bytes through the split.
    let bytes = unsafe { &*value };
    let result = std::str::from_utf8(bytes)
        // SAFETY: this caller still roots the exact byte object passed to the failing decode.
        .map_err(|error| decode_owned_error(unsafe { crate::acquire_bytes(value as usize) }, error))
        .and_then(|text| {
            super::split_lines(text, |range| super::text::try_owned_string(&text[range]))
        });
    finish(result, std::ptr::null())
}

/// Validates UTF-8 before later comprehension captures and retains the input.
/// The compiler may then gather characters from these immutable bytes without
/// allocating an intermediate string. Failure matches ordinary bytes.decode().
///
/// # Safety
/// Root an exact bytes value; success transfers an additional owner of the same
/// allocation. Invalid UTF-8 returns null and sets UnicodeDecodeError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_validate_utf8(value: *const Bytes) -> *const Bytes {
    // SAFETY: the caller roots immutable bytes throughout validation and errors.
    let bytes = unsafe { &*value };
    if let Err(error) = std::str::from_utf8(bytes) {
        // SAFETY: the validator's caller roots this exact immutable byte owner.
        return decode_owned_error(unsafe { crate::acquire_bytes(value as usize) }, error)
            .returning(std::ptr::null());
    }
    // SAFETY: the rooted exact crate::BytesOwner allocation provides the returned owner.
    unsafe { retained(value) }
}

/// A cursor owns its immutable input independently of the user's source binding.
pub(super) struct BytesIterator {
    owner: crate::BytesOwner,
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
pub unsafe extern "C" fn imre_bytes_iter(value: *const Bytes) -> *const Object {
    // SAFETY: the rooted source acquires an independent owner for the cursor.
    let owner = unsafe { crate::acquire_bytes(value as usize) };
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::BytesIterator(BytesIterator { owner, position: 0 }),
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::exceptions::{
        imre_error_pending, imre_error_take, imre_exception_matches, imre_exception_message,
    };
    use crate::{BigInt, MAX_SMALL, MIN_SMALL};

    /// Consumes one indexing failure and checks the exact public diagnostic.
    fn assert_index_error(expected: &str) {
        let error = imre_error_take();
        assert!(!error.is_null());
        // SAFETY: taking the pending exception transfers one live owner, and
        // message extraction returns another independently owned string.
        unsafe {
            let error = crate::ObjectOwner::from_raw(error);
            assert_eq!(
                imre_exception_matches(crate::ObjectOwner::as_ptr(&error), Kind::IndexError as u32),
                1
            );
            let message =
                StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(&error)));
            assert_eq!(message.as_str(), expected);
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn unchanged_bytes_results_share_the_input_allocation() {
        let value = crate::BytesOwner::new(vec![0, 1, 2]);
        let empty = crate::BytesOwner::new(Vec::new());
        let input = crate::BytesOwner::as_ptr(&value);
        let empty_ptr = crate::BytesOwner::as_ptr(&empty);
        // SAFETY: the two byte owners root the inputs, and each result transfers one owner.
        unsafe {
            for result in [
                imre_bytes_slice(input, 0, 0, 0),
                imre_bytes_repeat(input, Int::small(1).unwrap().into_raw()),
                imre_bytes_concat(input, empty_ptr),
                imre_bytes_concat(empty_ptr, input),
            ] {
                assert!(crate::BytesOwner::ptr_eq(
                    &crate::BytesOwner::from_raw(result),
                    &value
                ));
            }
        }
        assert_eq!(crate::BytesOwner::strong_count(&value), 1);
    }

    #[test]
    fn indexed_bytes_preserve_tagged_zero_signed_bounds_and_bigint_errors() {
        let bytes: Bytes = (0..=u8::MAX).collect();
        assert_eq!(imre_error_pending(), 0);
        // SAFETY: bytes and each canonical integer index remain rooted, and
        // every error below is consumed before entering another indexed read.
        unsafe {
            for byte in 0..=u8::MAX {
                let positive = Int::from_usize(usize::from(byte));
                let negative = Int::small(i64::from(byte) - 256).unwrap();
                let expected = (usize::from(byte) << 1) | 1;
                assert_eq!(imre_bytes_get(&bytes, positive.0), expected);
                assert_eq!(imre_bytes_get(&bytes, negative.0), expected);
                assert_eq!(imre_bytes_get_index(&bytes, usize::from(byte)), expected);
                assert_eq!(imre_error_pending(), 0);
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
                assert_eq!(imre_bytes_get(&bytes, index.0), 0);
                assert_index_error(expected);
                assert_eq!(imre_bytes_get(&bytes, 1), 1);
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
                assert_eq!(imre_bytes_get_index(&bytes, index), 0);
                assert_index_error(expected);
                assert_eq!(imre_bytes_get_index(&bytes, 0), 1);
            }
            assert_eq!(imre_bytes_get(&Bytes::new(), 1), 0);
            assert_index_error("index out of range");
            assert_eq!(imre_bytes_get_index(&Bytes::new(), 0), 0);
            assert_index_error("index out of range");
        }
    }

    #[test]
    fn ascii_decode_error_retains_its_input_and_offsets() {
        let input = crate::BytesOwner::new(b"a\xffb".to_vec());
        let observer = input.clone();
        // SAFETY: input roots the bytes until the decoder returns.
        assert!(unsafe { imre_bytes_decode_ascii(crate::BytesOwner::as_ptr(&input)) }.is_null());
        drop(input);
        // SAFETY: taking the pending error transfers its owner.
        let error = unsafe { crate::ObjectOwner::from_raw(imre_error_take()) };
        // SAFETY: error roots the exception, and repr transfers a new string owner.
        let repr = unsafe {
            crate::StrOwner::from_raw(crate::objects::exceptions::imre_exception_repr(
                crate::ObjectOwner::as_ptr(&error),
            ))
        };
        assert_eq!(
            repr.as_str(),
            r"UnicodeDecodeError('ascii', b'a\xffb', 1, 2, 'ordinal not in range(128)')"
        );
        assert_eq!(crate::BytesOwner::strong_count(&observer), 2);
        drop(error);
        assert_eq!(crate::BytesOwner::strong_count(&observer), 1);
    }

    #[test]
    fn utf8_validation_preserves_identity_ownership_and_decode_errors() {
        for text in ["", "ascii\0", "é\u{10ffff}"] {
            let source = crate::BytesOwner::new(text.as_bytes().to_vec());
            let observer = source.clone();
            // SAFETY: validation borrows one rooted input and transfers one
            // owner of that same allocation, which outlives the original root.
            unsafe {
                let result = imre_bytes_validate_utf8(crate::BytesOwner::as_ptr(&source));
                assert_eq!(result, crate::BytesOwner::as_ptr(&source));
                let result = crate::BytesOwner::from_raw(result);
                assert_eq!(crate::BytesOwner::strong_count(&source), 3);
                drop(source);
                assert_eq!(result.as_slice(), text.as_bytes());
                drop(result);
                assert_eq!(crate::BytesOwner::strong_count(&observer), 1);
            }
        }
        for bytes in [vec![0xff], vec![b'a', 0xe2, 0x82], vec![b'a', 0xe2, b' ']] {
            let source = crate::BytesOwner::new(bytes);
            // SAFETY: each failure transfers one exception; both are consumed
            // before the next call, and failed validation acquires no owner.
            unsafe {
                assert!(imre_bytes_decode(crate::BytesOwner::as_ptr(&source)).is_null());
                let ordinary = crate::ObjectOwner::from_raw(imre_error_take());
                let expected = StrOwner::from_raw(imre_exception_message(
                    crate::ObjectOwner::as_ptr(&ordinary),
                ));
                assert!(imre_bytes_validate_utf8(crate::BytesOwner::as_ptr(&source)).is_null());
                let error = crate::ObjectOwner::from_raw(imre_error_take());
                assert_eq!(
                    imre_exception_matches(
                        crate::ObjectOwner::as_ptr(&error),
                        Kind::UnicodeDecodeError as u32
                    ),
                    1
                );
                let message =
                    StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(&error)));
                assert_eq!(message, expected);
                // Both failures own the original bytes for exception repr, without copying them.
                assert_eq!(crate::BytesOwner::strong_count(&source), 3);
                drop(error);
                drop(ordinary);
                assert_eq!(crate::BytesOwner::strong_count(&source), 1);
                assert_eq!(imre_error_pending(), 0);
            }
        }
    }

    #[test]
    fn byte_output_transfers_its_allocation_at_conversion() {
        // SAFETY: the sole owner roots this private builder through conversion.
        unsafe {
            let output = crate::ObjectOwner::from_raw(imre_byte_output_new());
            imre_byte_output_reserve_range(std::ptr::null(), 1, 2001, 3, 2);
            imre_byte_output_reserve_range(
                crate::ObjectOwner::as_ptr(&output),
                1,
                2001,
                3,
                usize::MAX,
            );
            output.0.with(|storage| {
                let Storage::ByteOutput(output) = storage else {
                    unreachable!()
                };
                assert_eq!(output.bytes.capacity(), 256);
                assert!(output.bytes.is_empty());
            });
            assert_eq!(
                imre_byte_output_append_pair(crate::ObjectOwner::as_ptr(&output), 1, 511),
                1
            );
            let buffer = output.0.with(|storage| {
                let Storage::ByteOutput(output) = storage else {
                    unreachable!()
                };
                output.bytes.as_ptr()
            });
            let result = crate::BytesOwner::from_raw(imre_byte_output_finish(
                crate::ObjectOwner::as_ptr(&output),
            ));
            drop(output);
            assert_eq!(result.as_slice().as_ptr(), buffer);
            assert_eq!(result.as_slice(), [0, 255]);
        }
    }

    #[test]
    fn list_conversion_copies_all_bytes_across_chunk_boundaries() {
        for length in [0, 1, 63, 64, 65, 127, 128, 129, 255, 256, 257, 1024] {
            let expected = (0..length).map(|index| index as u8).collect::<Vec<_>>();
            let source = crate::ListOwner::new(List::new(
                expected
                    .iter()
                    .map(|&byte| Int::from_usize(usize::from(byte)))
                    .collect(),
            ));
            let alias = source.clone();
            // SAFETY: both aliases root the exact list and conversion returns
            // one independently owned immutable bytes object.
            unsafe {
                let converted =
                    crate::BytesOwner::from_raw(imre_bytes_list(crate::ListOwner::as_ptr(&source)));
                assert_eq!(*converted, expected);
                assert_eq!(crate::ListOwner::strong_count(&source), 2);
                alias.0.with_mut(|values| {
                    assert_eq!(values.len(), expected.len());
                    for (value, &byte) in values.iter().zip(&expected) {
                        assert_eq!(value.as_small(), Some(i64::from(byte)));
                    }
                    if let Some(first) = values.first_mut() {
                        *first = Int::from_usize(255);
                    }
                });
                assert_eq!(*converted, expected);
            }
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn list_conversion_rejects_invalid_words_without_changing_the_source() {
        let invalid = [
            Int::small(-1).unwrap(),
            Int::from_usize(256),
            Int::small(MIN_SMALL).unwrap(),
            Int::small(MAX_SMALL).unwrap(),
            Int::new(BigInt::from(1_u8) << 100_u32),
            Int::new(-(BigInt::from(1_u8) << 100_u32)),
        ];
        for invalid in &invalid {
            for position in [0, 1, 63, 64, 65, 127, 128] {
                let mut values = (0..129).map(|_| Int::from_usize(17)).collect::<Vec<_>>();
                values[position] = invalid.clone();
                let source = crate::ListOwner::new(List::new(values));
                // SAFETY: the source roots every exact integer while failed
                // conversion transfers only a pending exception owner.
                unsafe {
                    assert!(imre_bytes_list(crate::ListOwner::as_ptr(&source)).is_null());
                    let error = crate::ObjectOwner::from_raw(imre_error_take());
                    assert_eq!(
                        imre_exception_matches(
                            crate::ObjectOwner::as_ptr(&error),
                            Kind::ValueError as u32
                        ),
                        1
                    );
                    let message = StrOwner::from_raw(imre_exception_message(
                        crate::ObjectOwner::as_ptr(&error),
                    ));
                    assert_eq!(message.as_str(), "bytes must be in range(0, 256)");
                    source.0.with(|values| {
                        assert_eq!(values.len(), 129);
                        assert_eq!(values[position].0, invalid.0);
                        assert_eq!(values[position].big(), invalid.big());
                    });
                }
                assert_eq!(imre_error_pending(), 0);
            }
        }
    }

    #[test]
    fn standalone_bytes_repr_uses_the_escaped_size() {
        let input = vec![0; 16 * 1024];
        let mut output = String::new();
        write_repr(&mut output, &input);
        assert_eq!(output.len(), 4 * input.len() + 3);
        assert!(output.starts_with("b\x27\\x00"));
        assert!(output.ends_with("\\x00\x27"));
        assert!(output.capacity() < 128 * 1024);
    }
}
