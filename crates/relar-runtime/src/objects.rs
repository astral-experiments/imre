//! Typed container handles with one representation descriptor per homogeneous buffer.
//!
//! Records have separate compact native allocations. Object handles cover sequences,
//! mappings, iterators, exceptions, and resources. All calls follow the parent's
//! single-threaded, nonreentrant ownership contract. No growable buffer escapes;
//! displaced owners are dropped only after mutable storage access ends.

mod args;
pub(crate) mod exceptions;
mod files;
mod mappings;
pub(crate) mod records;
mod sequences;
mod words;

use super::{
    Error, Int, Kind, List, MAX_LIST_LEN, StorageCell, Str, decode_index, fail, finish, index,
};
use num_traits::ToPrimitive;
use std::rc::Rc;
use words::{OwnedWord, Words};

/// An opaque allocation for runtime-managed containers and resource state.
pub struct Object(StorageCell<Storage>);

enum Storage {
    Sequence(Words),
    Dictionary(Box<mappings::Mapping>),
    Set(Box<mappings::Set>),
    MappingIterator(mappings::MappingIterator),
    StringIterator(sequences::StringIterator),
    Exception(exceptions::Exception),
    TextIO(Box<files::TextFile>),
}

/// Acquires a reference without consuming the caller's live Rc handle.
unsafe fn acquire_rc<T>(word: usize) -> Rc<T> {
    if word == 0 {
        fail("TypeError", "unexpected null runtime handle");
    }
    let pointer = word as *const T;
    // SAFETY: the caller supplies a rooted allocation of the exact type.
    unsafe {
        Rc::increment_strong_count(pointer);
        Rc::from_raw(pointer)
    }
}

/// Borrows a rooted object for one nonreentrant operation.
unsafe fn object<'a>(pointer: *const Object) -> &'a Object {
    if pointer.is_null() {
        fail("TypeError", "cannot access an absent optional value");
    }
    // SAFETY: the caller roots this allocation for the operation.
    unsafe { &*pointer }
}

impl Storage {
    fn sequence(&self) -> &Words {
        match self {
            Self::Sequence(values) => values,
            _ => fail("SystemError", "expected a sequence object"),
        }
    }
    fn sequence_mut(&mut self) -> &mut Words {
        match self {
            Self::Sequence(values) => values,
            _ => fail("SystemError", "expected a sequence object"),
        }
    }
}

/// # Safety
/// `value` is a live Object handle or an absent nullable value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_object_retain(value: *const Object) {
    if !value.is_null() {
        // SAFETY: the caller roots this allocation.
        unsafe { Rc::increment_strong_count(value) };
    }
}

/// # Safety
/// Transfers one Object reference, or accepts null during nullable/partial cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_object_release(value: *const Object) {
    if !value.is_null() {
        // SAFETY: the caller transfers one live strong reference.
        unsafe { Rc::decrement_strong_count(value) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_seq_new(kind: u32) -> *const Object {
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Sequence(
        Words::new(kind),
    )))))
}

/// # Safety
/// Root a sequence and exact canonical Int index. The result owns one value of
/// the sequence's declared kind; pending must be checked before interpreting zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_get(value: *const Object, at: usize) -> usize {
    // SAFETY: the index remains rooted for conversion.
    let at = unsafe { Int::borrow(at) };
    let at = match decode_index(&at) {
        Ok(at) => at,
        Err(error) => return error.returning(0),
    };
    // SAFETY: acquisition cannot reenter and only an owned word escapes.
    let owned = unsafe {
        object(value).0.with(|storage| {
            let values = storage.sequence();
            Ok(values
                .kind
                .acquire(values.values[index(at, values.values.len())?]))
        })
    };
    finish(owned.map(OwnedWord::into_word), 0)
}

/// # Safety
/// `at` is the exact compiler-proven nonnegative native index, not a tagged word.
/// The rooted sequence keeps current bounds checking and returns an owned value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_get_index(value: *const Object, at: usize) -> usize {
    // SAFETY: only an independently acquired value leaves the storage access.
    let owned = unsafe {
        object(value).0.with(|storage| {
            let values = storage.sequence();
            let word = *values.values.get(at).ok_or_else(Error::index)?;
            Ok(values.kind.acquire(word))
        })
    };
    finish(owned.map(OwnedWord::into_word), 0)
}

/// Acquires the replacement before any existing container owner can be displaced.
unsafe fn replacement(value: *const Object, word: usize, kind: u32) -> OwnedWord {
    // SAFETY: only copied descriptor metadata escapes this rooted storage read.
    let actual = unsafe {
        object(value).0.with(|storage| {
            let values = storage.sequence();
            values.check_kind(kind);
            values.kind
        })
    };
    // SAFETY: the compiler supplies a borrowed word of the exact element type.
    unsafe { actual.acquire(word) }
}

/// # Safety
/// All handles are rooted; the replacement has the sequence's declared kind and
/// preserves heap acyclicity. Failed indexing leaves the sequence unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_set(value: *const Object, at: usize, word: usize, kind: u32) {
    // SAFETY: the caller supplies the checked replacement and index representations.
    let mut new = Some(unsafe { replacement(value, word, kind) });
    // SAFETY: the caller roots the exact canonical integer index.
    let at = unsafe { Int::borrow(at) };
    let at = match decode_index(&at) {
        Ok(at) => at,
        Err(error) => return error.raise(),
    };
    // SAFETY: ownership leaves mutable storage before any displaced value drops.
    let old = unsafe {
        object(value).0.with_mut(|storage| {
            let values = storage.sequence_mut();
            let at = index(at, values.values.len())?;
            Ok(OwnedWord::from_owned(
                std::mem::replace(&mut values.values[at], new.take().unwrap().into_word()),
                values.kind,
            ))
        })
    };
    drop(new);
    finish(old.map(drop), ());
}

/// # Safety
/// As for set, but `at` is an exact nonnegative native index. Bounds are checked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_set_index(
    value: *const Object,
    at: usize,
    word: usize,
    kind: u32,
) {
    // SAFETY: the checked replacement is acquired before replacing its last owner.
    let mut new = Some(unsafe { replacement(value, word, kind) });
    // SAFETY: no reference escapes; the displaced owner is dropped afterward.
    let old = unsafe {
        object(value).0.with_mut(|storage| {
            let values = storage.sequence_mut();
            let slot = values.values.get_mut(at).ok_or_else(Error::index)?;
            Ok(OwnedWord::from_owned(
                std::mem::replace(slot, new.take().unwrap().into_word()),
                values.kind,
            ))
        })
    };
    drop(new);
    finish(old.map(drop), ());
}

/// # Safety
/// The rooted word matches this sequence's element kind and preserves acyclicity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_append(value: *const Object, word: usize, kind: u32) {
    // SAFETY: acquiring first also handles values rooted only by this container.
    let new = unsafe { replacement(value, word, kind) };
    // SAFETY: no callback or escaped buffer can overlap this mutation.
    unsafe {
        object(value).0.with_mut(|storage| {
            let values = &mut storage.sequence_mut().values;
            values
                .try_reserve(1)
                .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
            values.push(new.into_word());
        })
    };
}

/// # Safety
/// The sequence is rooted. Success transfers its removed owner's exact word;
/// check pending before interpreting zero, which may also be a valid element.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_pop(value: *const Object) -> usize {
    // SAFETY: only a transferred owner escapes mutable storage.
    let owned = unsafe {
        object(value).0.with_mut(|storage| {
            let values = storage.sequence_mut();
            let word = values
                .values
                .pop()
                .ok_or_else(|| Error::new(Kind::IndexError, "pop from empty list"))?;
            Ok(OwnedWord::from_owned(word, values.kind))
        })
    };
    finish(owned.map(OwnedWord::into_word), 0)
}

/// # Safety
/// `value` is a rooted concrete sequence.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_len(value: *const Object) -> usize {
    // SAFETY: only the copied length leaves storage.
    let length = unsafe {
        object(value)
            .0
            .with(|storage| storage.sequence().values.len())
    };
    debug_assert!(length <= MAX_LIST_LEN);
    (length << 1) | 1
}

/// Repetition preserves the element representation and shares referenced objects.
///
/// # Safety
/// Both sequence and canonical integer count remain rooted throughout the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_repeat(value: *const Object, count: usize) -> *const Object {
    // SAFETY: the caller roots the integer count.
    let count = unsafe { Int::borrow(count) };
    let count = count
        .as_small()
        .map(|value| value as isize)
        .or_else(|| count.big().to_isize());
    let Some(count) = count else {
        return Error::new(Kind::OverflowError, "list repeat count is too large")
            .returning(std::ptr::null());
    };
    // SAFETY: only independent owned words escape; acquisition cannot reenter.
    let result = unsafe {
        object(value).0.with(|storage| {
            let values = storage.sequence();
            let count = usize::try_from(count).unwrap_or(0);
            let length = values
                .values
                .len()
                .checked_mul(count)
                .filter(|&length| length <= MAX_LIST_LEN)
                .ok_or_else(|| Error::new(Kind::MemoryError, "list is too large"))?;
            let mut result = Words::new(values.kind.raw());
            result
                .values
                .try_reserve_exact(length)
                .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
            if !values.values.is_empty() {
                for _ in 0..count {
                    for &word in &values.values {
                        result.values.push(values.kind.acquire(word).into_word());
                    }
                }
            }
            Ok(result)
        })
    };
    finish(
        result.map(|values| {
            Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Sequence(values)))))
        }),
        std::ptr::null(),
    )
}

/// # Safety
/// Both arguments must be rooted, non-null immutable string handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_concat(a: *const Str, b: *const Str) -> *const Str {
    // SAFETY: both immutable strings remain rooted for this call.
    let (a, b) = unsafe { (&*a, &*b) };
    let length = a
        .len()
        .checked_add(b.len())
        .unwrap_or_else(|| fail("MemoryError", "string is too large"));
    let mut result = Str::try_with_capacity(length)
        .unwrap_or_else(|_| fail("MemoryError", "string allocation failed"));
    result.push_str(a);
    result.push_str(b);
    Rc::into_raw(Rc::new(result))
}

/// One borrowed concatenation fragment. `usize::MAX` identifies a rooted Str
/// handle; other lengths identify immutable, valid UTF-8 bytes, including
/// compiler literals and caller-owned formatting scratch.
#[repr(C)]
pub struct StringPart {
    data: *const u8,
    length: usize,
}

const _: () = assert!(size_of::<StringPart>() == 16);

impl StringPart {
    /// # Safety
    /// This fragment's handle or byte span remains readable and immutable for
    /// the borrow. Nonempty spans contain valid UTF-8; empty spans may be null.
    unsafe fn as_str(&self) -> &str {
        if self.length == usize::MAX {
            // SAFETY: the sentinel selects a rooted, correctly aligned Str handle.
            unsafe { &*self.data.cast::<Str>() }.as_str()
        } else if self.length == 0 {
            ""
        } else {
            // SAFETY: the caller supplies this immutable, valid UTF-8 span.
            unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.data, self.length))
            }
        }
    }
}

/// Formats one integer at its expression position without allocating for small
/// values. Null means the fragment borrows `buffer`; otherwise the result owns
/// the fallback string referenced by `part`. Neither outcome consumes `value`.
///
/// # Safety
/// `value` is a rooted canonical integer for this call. `buffer` points to 20
/// writable bytes, and `part` to one aligned writable descriptor; they do not
/// overlap each other or any borrowed input. Keep the written span immutable,
/// or the returned string rooted, until the fragment's concatenation finishes.
/// The returned nullable owner must be released once. Allocation failure is fatal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_int_str_part(
    value: usize,
    buffer: *mut u8,
    part: *mut StringPart,
) -> *const Str {
    // SAFETY: the caller roots this integer throughout formatting.
    let borrowed = unsafe { Int::borrow(value) };
    if let Some(value) = borrowed.as_small() {
        let mut formatted = itoa::Buffer::new();
        let formatted = formatted.format(value);
        // SAFETY: every i64 decimal spelling fits 20 bytes. The caller's scratch
        // and descriptor are writable and disjoint from this local formatter.
        unsafe {
            std::ptr::copy_nonoverlapping(formatted.as_ptr(), buffer, formatted.len());
            part.write(StringPart {
                data: buffer,
                length: formatted.len(),
            });
        }
        std::ptr::null()
    } else {
        // SAFETY: the input is still rooted; this conversion returns an independent
        // string owner and performs arbitrary-precision formatting exactly once.
        let result = unsafe { super::relar_int_str(value) };
        // SAFETY: the caller provides a writable descriptor, and the returned
        // owner keeps its immutable Str allocation alive for concatenation.
        unsafe {
            part.write(StringPart {
                data: result.cast(),
                length: usize::MAX,
            });
        }
        result
    }
}

/// Joins already evaluated fragments into one owned string without intermediate
/// strings or a temporary fragment vector. Capacity is checked before copying.
///
/// # Safety
/// `parts` is an aligned, readable array of `count` initialized descriptors,
/// or null when `count` is zero. Each handle remains rooted and each byte span
/// remains immutable and valid UTF-8 for this call. The array and spans obey Rust
/// slice size limits. This private ABI borrows every input; allocation exhaustion
/// is fatal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_concat_parts(
    parts: *const StringPart,
    count: usize,
) -> *const Str {
    // SAFETY: this entry point has the builder's same fragment-lifetime contract.
    Rc::into_raw(Rc::new(unsafe { concat_parts(parts, count) }))
}

/// Builds a complete concatenation before publishing it or accessing a writer.
///
/// # Safety
/// The descriptors, handles, and spans obey `relar_str_concat_parts`'s contract.
unsafe fn concat_parts(parts: *const StringPart, count: usize) -> Str {
    let parts = if count == 0 {
        &[]
    } else {
        // SAFETY: the compiler supplies a readable descriptor array of this length.
        unsafe { std::slice::from_raw_parts(parts, count) }
    };
    let length = parts.iter().fold(0_usize, |length, part| {
        // SAFETY: every fragment remains immutable and rooted throughout this call.
        let text = unsafe { part.as_str() };
        length
            .checked_add(text.len())
            .unwrap_or_else(|| fail("MemoryError", "string is too large"))
    });
    let mut result = Str::try_with_capacity(length)
        .unwrap_or_else(|_| fail("MemoryError", "string allocation failed"));
    {
        // SAFETY: the fresh result has capacity for the checked total. We fill
        // its spare bytes without reading or moving it, then publish length once.
        // At the inline capacity, the final byte also holds the tag: sequential
        // prefixes overwrite it only with the final byte of complete valid UTF-8.
        let output = unsafe { result.spare_capacity_mut() }
            .as_mut_ptr()
            .cast::<u8>();
        let mut offset = 0;
        for part in parts {
            // SAFETY: no immutable input has changed since the capacity pass.
            let text = unsafe { part.as_str() };
            // SAFETY: the checked total bounds every prefix, and the fresh result
            // cannot overlap any borrowed input. No uninitialized byte is read.
            unsafe {
                std::ptr::copy_nonoverlapping(text.as_ptr(), output.add(offset), text.len());
            }
            offset += text.len();
        }
    }
    // SAFETY: exactly `length` bytes were initialized within capacity; joining
    // complete UTF-8 fragments preserves UTF-8, including the full-inline case.
    unsafe { result.set_len(length) };
    result
}

/// Python also treats the four ASCII information separators as whitespace.
fn python_whitespace(value: char) -> bool {
    value.is_whitespace() || matches!(value, '\u{001c}'..='\u{001f}')
}

/// Splits with Python's no-separator whitespace rules, omitting empty fields.
///
/// # Safety
/// `value` must be a rooted, non-null string handle. The returned Object is an
/// owned sequence of strings and must use the generic sequence ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_split_whitespace(value: *const Str) -> *const Object {
    // SAFETY: the caller roots the immutable string throughout the split.
    let value = unsafe { &*value };
    let mut result = Words::new(3);
    for part in value
        .split(python_whitespace)
        .filter(|part| !part.is_empty())
    {
        result
            .values
            .try_reserve(1)
            .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
        result
            .values
            .push(Rc::into_raw(Rc::new(Str::new(part))) as usize);
    }
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Sequence(result)))))
}

/// Splits Python line boundaries without retaining separators. CRLF is one
/// boundary; a final boundary creates no extra item, and U+001F is not a boundary.
///
/// # Safety
/// `value` must be a rooted string. Returns an owned generic sequence of strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_splitlines(value: *const Str) -> *const Object {
    // SAFETY: the caller roots the immutable string throughout the split.
    let value = unsafe { &*value };
    let mut result = Words::new(3);
    let mut start = 0;
    let mut chars = value.char_indices().peekable();
    while let Some((at, character)) = chars.next() {
        if matches!(
            character,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            result
                .values
                .try_reserve(1)
                .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
            result
                .values
                .push(Rc::into_raw(Rc::new(Str::new(&value[start..at]))) as usize);
            start = at + character.len_utf8();
            if character == '\r' && chars.peek().is_some_and(|(_, next)| *next == '\n') {
                chars.next();
                start += 1;
            }
        }
    }
    if start < value.len() {
        result
            .values
            .try_reserve(1)
            .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
        result
            .values
            .push(Rc::into_raw(Rc::new(Str::new(&value[start..]))) as usize);
    }
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Sequence(result)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concatenation_initializes_inline_tag_and_heap_boundaries() {
        for (length, ending) in [
            (0, ""),
            (23, "é"),
            (24, "z"),
            (24, "\0"),
            (24, "🦀"),
            (25, "é"),
            (25, "🦀"),
        ] {
            let padding = "a".repeat(length - ending.len());
            let split = padding.len() / 2;
            let owned = Rc::new(Str::new(&padding[split..]));
            let parts = [
                StringPart {
                    data: padding.as_ptr(),
                    length: split,
                },
                StringPart {
                    data: std::ptr::null(),
                    length: 0,
                },
                StringPart {
                    data: Rc::as_ptr(&owned).cast(),
                    length: usize::MAX,
                },
                StringPart {
                    data: ending.as_ptr(),
                    length: ending.len(),
                },
                StringPart {
                    data: std::ptr::null(),
                    length: 0,
                },
            ];
            // SAFETY: the ASCII padding, complete UTF-8 ending, and owned handle
            // remain immutable; null descriptors have zero length.
            let mut result = unsafe { concat_parts(parts.as_ptr(), parts.len()) };
            drop(owned);
            let expected = format!("{padding}{ending}");
            assert_eq!(result.len(), length);
            assert_eq!(result.as_str(), expected);
            assert_eq!(result.is_heap_allocated(), length > 24);
            result.push_str("!");
            assert_eq!(result.as_str(), format!("{expected}!"));
        }
    }

    #[test]
    fn integer_parts_outlive_inputs_and_preserve_fallback_ownership() {
        use num_bigint::BigInt;
        use std::mem::MaybeUninit;

        for number in [
            BigInt::from(crate::MIN_SMALL),
            BigInt::from(crate::MAX_SMALL),
            BigInt::from(0),
            BigInt::from(-1),
            BigInt::from(-1001),
            BigInt::from(17),
            BigInt::from(crate::MIN_SMALL) - 1,
            BigInt::from(crate::MAX_SMALL) + 1,
            BigInt::from(1) << 400,
            -(BigInt::from(1) << 400_u32),
        ] {
            let expected = number.to_string();
            let integer = Int::new(number);
            let small = integer.as_small().is_some();
            let weak_integer = if small {
                None
            } else {
                // SAFETY: the integer owns this exact BigInt allocation. The
                // temporary acquired reference is released before formatting.
                let owner = unsafe { acquire_rc::<BigInt>(integer.0) };
                Some(Rc::downgrade(&owner))
            };
            let mut buffer = [0xcc_u8; 22];
            let scratch = buffer[1..21].as_mut_ptr();
            let mut part = MaybeUninit::<StringPart>::uninit();
            // SAFETY: the integer is rooted, and these disjoint caller allocations
            // provide 20 writable bytes and one aligned descriptor.
            let owner = unsafe { relar_int_str_part(integer.0, scratch, part.as_mut_ptr()) };
            // SAFETY: both successful helper paths initialize the descriptor.
            let part = unsafe { part.assume_init() };
            assert_eq!(owner.is_null(), small);
            assert_eq!((buffer[0], buffer[21]), (0xcc, 0xcc));
            let owner = if small {
                assert_eq!(part.data, scratch.cast_const());
                assert_eq!(part.length, expected.len());
                assert_eq!(
                    std::str::from_utf8(&buffer[1..1 + part.length]).unwrap(),
                    expected
                );
                assert!(buffer[1 + part.length..21].iter().all(|byte| *byte == 0xcc));
                None
            } else {
                assert_eq!(part.data, owner.cast());
                assert_eq!(part.length, usize::MAX);
                assert!(buffer.iter().all(|byte| *byte == 0xcc));
                assert_eq!(weak_integer.as_ref().unwrap().strong_count(), 1);
                // SAFETY: the helper transfers exactly one non-null Str owner.
                Some(unsafe { Rc::from_raw(owner) })
            };
            let weak_string = owner.as_ref().map(Rc::downgrade);
            drop(integer);
            if let Some(weak) = weak_integer {
                assert!(weak.upgrade().is_none());
            }
            let prefix = "é\0[";
            let suffix = "]🦀";
            let parts = [
                StringPart {
                    data: prefix.as_ptr(),
                    length: prefix.len(),
                },
                part,
                StringPart {
                    data: suffix.as_ptr(),
                    length: suffix.len(),
                },
            ];
            // SAFETY: caller scratch, literals, and the optional fallback owner
            // remain immutable and live; the original integer is already gone.
            let result =
                unsafe { Rc::from_raw(relar_str_concat_parts(parts.as_ptr(), parts.len())) };
            drop(owner);
            if let Some(weak) = weak_string {
                assert!(weak.upgrade().is_none());
            }
            assert_eq!(result.as_str(), format!("{prefix}{expected}{suffix}"));
        }
    }

    #[test]
    fn concatenation_parts_borrow_aliases_and_preserve_utf8_and_nuls() {
        let source = Rc::new(Str::new("α\0🦀"));
        let alias = source.clone();
        let weak = Rc::downgrade(&source);
        let prefix = "literalé\0:";
        let suffix = ":a suffix exceeding inline string capacity";
        let parts = [
            StringPart {
                data: prefix.as_ptr(),
                length: prefix.len(),
            },
            StringPart {
                data: Rc::as_ptr(&source).cast(),
                length: usize::MAX,
            },
            StringPart {
                data: std::ptr::null(),
                length: 0,
            },
            StringPart {
                data: Rc::as_ptr(&alias).cast(),
                length: usize::MAX,
            },
            StringPart {
                data: suffix.as_ptr(),
                length: suffix.len(),
            },
        ];
        // SAFETY: both aliases and all literal spans outlive the descriptor array's
        // use; the call returns one independently owned concatenated string.
        let result = unsafe { Rc::from_raw(relar_str_concat_parts(parts.as_ptr(), parts.len())) };
        assert_eq!(Rc::strong_count(&source), 2);
        assert_eq!(source.as_str(), "α\0🦀");
        drop((source, alias));
        assert!(weak.upgrade().is_none());
        assert_eq!(
            result.as_str(),
            "literalé\0:α\0🦀α\0🦀:a suffix exceeding inline string capacity"
        );
        // SAFETY: zero count permits a null array, and empty literal spans permit
        // null data; each returned reference is adopted and released once.
        unsafe {
            assert!(Rc::from_raw(relar_str_concat_parts(std::ptr::null(), 0)).is_empty());
            assert!(Rc::from_raw(relar_str_concat_parts(&parts[2], 1)).is_empty());
        }
    }

    #[test]
    fn sequence_words_preserve_niches_and_owned_snapshots() {
        let text = Rc::new(Str::from("optional payload"));
        let weak = Rc::downgrade(&text);
        let word = Rc::as_ptr(&text) as usize;
        // SAFETY: exact sequence kinds describe every scalar or pointer word;
        // nullable strings use null directly without any wrapper allocation.
        unsafe {
            let optional = relar_seq_new(259);
            relar_seq_append(optional, 0, 259);
            relar_seq_append(optional, word, 259);
            drop(text);
            relar_seq_set_index(optional, 1, word, 259);
            let saved = Rc::from_raw(relar_seq_get_index(optional, 1) as *const Str);
            relar_seq_set_index(optional, 1, 0, 259);
            assert_eq!(relar_seq_pop(optional), 0);
            assert_eq!(relar_seq_get_index(optional, 0), 0);
            relar_object_release(optional);
            assert_eq!(saved.as_str(), "optional payload");
            drop(saved);

            for (kind, values) in [
                (257, vec![0, 1, 2]),
                (258, vec![0, 1, Int::small(-1).unwrap().0]),
            ] {
                let sequence = relar_seq_new(kind);
                for &value in &values {
                    relar_seq_append(sequence, value, kind);
                }
                let empty = relar_seq_repeat(sequence, Int::small(-1).unwrap().0);
                assert_eq!(Int(relar_seq_len(empty)).as_small(), Some(0));
                assert_eq!(object(empty).0.with(|s| s.sequence().kind.raw()), kind);
                relar_object_release(empty);
                for (index, &value) in values.iter().enumerate() {
                    assert_eq!(relar_seq_get_index(sequence, index), value);
                }
                relar_object_release(sequence);
            }
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn repeated_record_sequences_share_mutations_and_transfer_pop_owners() {
        use records::{RecordDescriptor, relar_record_new, relar_record_release};
        let kinds = [2, 3];
        let descriptor = RecordDescriptor {
            length: 2,
            kinds: kinds.as_ptr(),
        };
        let text = Rc::new(Str::from("nested record root"));
        let weak = Rc::downgrade(&text);
        // SAFETY: metadata outlives all records and each copied record reference
        // is independently rooted until released; field writes transfer ownership.
        unsafe {
            let child = relar_record_new(&descriptor);
            child.write(Int::small(4).unwrap().into_raw());
            child.add(1).cast::<*const Str>().write(Rc::into_raw(text));
            let original = relar_seq_new(6);
            relar_seq_append(original, child as usize, 6);
            relar_record_release(child);
            let repeated = relar_seq_repeat(original, Int::small(2).unwrap().0);
            let popped = relar_seq_pop(original) as *mut usize;
            relar_object_release(original);
            popped.write(Int::small(9).unwrap().into_raw());
            let alias = relar_seq_get(repeated, Int::small(-1).unwrap().0) as *const usize;
            assert_eq!(Int(alias.read()).as_small(), Some(9));
            relar_object_release(repeated);
            relar_record_release(popped);
            assert!(weak.upgrade().is_some());
            relar_record_release(alias);
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn optional_bigint_sequence_replacement_releases_only_displaced_owners() {
        use num_bigint::BigInt;
        let integer = Rc::new(BigInt::from(1_u128 << 120));
        let weak = Rc::downgrade(&integer);
        let word = Rc::as_ptr(&integer) as usize;
        // SAFETY: nullable Int word0 and canonical nonzero words are distinct;
        // every returned BigInt owner is adopted exactly once.
        unsafe {
            let values = relar_seq_new(258);
            relar_seq_append(values, word, 258);
            drop(integer);
            relar_seq_set_index(values, 0, word, 258);
            let saved = Int(relar_seq_get_index(values, 0));
            relar_seq_set_index(values, 0, 0, 258);
            relar_object_release(values);
            assert_eq!(saved.big().as_ref(), &BigInt::from(1_u128 << 120));
            drop(saved);
        }
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn splitlines_distinguishes_every_python_boundary_and_retains_interior_empty_lines() {
        for (source, expected) in [
            ("", vec![]),
            ("\n", vec![""]),
            ("\r\n\r\n", vec!["", ""]),
            (
                "a\nb\rc\r\nd\x0be\x0cf\x1cg\x1dh\x1ei\u{85}j\u{2028}k\u{2029}",
                vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"],
            ),
            ("é\x1f🦀\nlast", vec!["é\x1f🦀", "last"]),
        ] {
            let source = Str::new(source);
            // SAFETY: the source is rooted; the returned sequence transfers one
            // owner, and inspection copies strings without retaining storage refs.
            let actual = unsafe {
                let result = Rc::from_raw(relar_str_splitlines(&source));
                result.0.with(|storage| {
                    storage
                        .sequence()
                        .values
                        .iter()
                        .map(|&word| (&*(word as *const Str)).to_string())
                        .collect::<Vec<_>>()
                })
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn splitting_owns_strings_and_matches_python_whitespace_boundaries() {
        // The complete whitespace set in CPython 3.14's _PyUnicode_IsWhitespace.
        let whitespace = "\u{9}\u{a}\u{b}\u{c}\u{d}\u{1c}\u{1d}\u{1e}\u{1f}\u{20}\u{85}\u{a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}";
        let text = Rc::new(Str::from(format!(
            "{whitespace}α{whitespace}β\u{200b}γ{whitespace}"
        )));
        let zero = Int::small(0).unwrap();
        let one = Int::small(1).unwrap();
        // SAFETY: the input is rooted; split returns an owned sequence of string references.
        unsafe {
            let parts = relar_str_split_whitespace(Rc::as_ptr(&text));
            drop(text);
            assert_eq!(Int(relar_seq_len(parts)).as_small(), Some(2));
            let first = Rc::from_raw(relar_seq_get(parts, zero.0) as *const Str);
            let second = Rc::from_raw(relar_seq_get(parts, one.0) as *const Str);
            relar_object_release(parts);
            assert_eq!(first.as_str(), "α");
            assert_eq!(second.as_str(), "β\u{200b}γ");
            let joined = Rc::from_raw(relar_str_concat(Rc::as_ptr(&first), Rc::as_ptr(&second)));
            drop((first, second));
            assert_eq!(joined.as_str(), "αβ\u{200b}γ");
        }
    }
}
