//! Typed container handles with one representation descriptor per homogeneous buffer.
//!
//! Records have separate compact native allocations. Object handles cover sequences,
//! mappings, iterators, exceptions, and resources. All calls follow the parent's
//! single-threaded, nonreentrant ownership contract. Growable buffers stay private
//! except for compiler-proven readonly loop views; displaced owners are dropped
//! only after mutable storage access ends.

mod args;
mod bytes;
pub(crate) mod exceptions;
mod files;
mod mappings;
pub(crate) mod records;
mod sequences;
mod unions;
mod words;

use super::{
    Bytes, Error, Int, Kind, List, MAX_LIST_LEN, StorageCell, Str, decode_index, fail, finish,
    index,
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
    BytesIterator(bytes::BytesIterator),
    Exception(exceptions::Exception),
    File(Box<files::FileState>),
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

/// Borrows one concrete sequence word after its current bounds were proved.
/// Zero remains an ordinary scalar or absent value, never an error sentinel.
///
/// # Safety
/// `value` must be a non-null rooted exact sequence, and `at` a decoded native index
/// strictly below its current length. No mutable storage access may overlap the
/// call. Consume or acquire the returned word with its exact declared type before
/// removing or replacing that element. This does not acquire a reference, and
/// neither a storage reference nor a growable buffer pointer escapes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_borrow_index_proven(value: *const Object, at: usize) -> usize {
    // SAFETY: SequenceGetInBounds supplies a non-null exact sequence and proves
    // its current bounds. Its two emission paths either acquire the result
    // immediately or keep an unchanged caller root. Neither a storage reference
    // nor a pointer into the movable buffer escapes this nonreentrant read.
    unsafe {
        (&*value).0.with(|storage| {
            let Storage::Sequence(values) = storage else {
                // Exact kind is part of the same caller proof as the index.
                std::hint::unreachable_unchecked()
            };
            *values.values.get_unchecked(at)
        })
    }
}

/// Exposes an immutable word buffer only for a compiler-proven readonly loop.
/// Empty sequences may return a dangling, aligned pointer that must not be read.
///
/// # Safety
/// `value` is a non-null rooted exact sequence. Keep that same owner alive and
/// prohibit every mutation, reserve, and reentrant access to its storage until
/// the last use of the result. The pointer may exist only as private compiler
/// state, never as a language value or an escaping return/field/container value.
/// Every read must prove its native index below the unchanged current length;
/// consume or acquire each word using the sequence's exact declared element type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_data_readonly(value: *const Object) -> *const usize {
    // SAFETY: this stronger, whole-loop contract deliberately bypasses `with`'s
    // per-call no-buffer-escape rule. Only a raw pointer escapes the short shared
    // storage borrow; the caller keeps its allocation rooted and immutable.
    unsafe {
        let Storage::Sequence(values) = &*(*value).0.0.get() else {
            std::hint::unreachable_unchecked()
        };
        values.values.as_ptr()
    }
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
/// compiler literals and caller-owned formatting scratch. A consuming builder
/// also roots fragments borrowed directly from its transferred left operand.
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

/// Writes one indexed Unicode scalar as a borrowed fragment without allocating.
/// Returns one on success; zero installs StringGet's error and leaves both
/// outputs untouched. The fragment remains valid after releasing both inputs.
///
/// # Safety
/// `value` and the canonical integer `at` remain rooted for this call. `buffer`
/// points to four writable bytes and `part` to one aligned writable descriptor;
/// both are disjoint from each other and from every input. Enter with no pending
/// error and check the status before reading either output. Keep the written
/// span immutable until its concatenation finishes; neither input is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_get_part(
    value: *const Str,
    at: usize,
    buffer: *mut u8,
    part: *mut StringPart,
) -> u8 {
    // SAFETY: both immutable inputs remain rooted throughout Unicode indexing.
    let (value, at) = unsafe { (&*value, Int::borrow(at)) };
    let character = match sequences::string_character(value, &at) {
        Ok(character) => character,
        Err(error) => return error.returning(0),
    };
    // SAFETY: the caller provides four disjoint writable bytes, enough for every
    // Unicode scalar. No output is touched before all recoverable checks succeed.
    let buffer_bytes = unsafe { std::slice::from_raw_parts_mut(buffer, 4) };
    let length = character.encode_utf8(buffer_bytes).len();
    // SAFETY: the writable descriptor borrows the caller's initialized UTF-8 span.
    unsafe {
        part.write(StringPart {
            data: buffer,
            length,
        });
    }
    1
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

/// Appends already evaluated fragments, reusing uniquely owned string storage.
/// Shared strings and suffixes borrowing the left operand use a fresh result;
/// the consumed owner keeps every such borrow alive until copying finishes.
/// CompactString's reservation grows geometrically across repeated appends.
///
/// # Safety
/// `left` transfers exactly one live, non-null Rc<Str> owner. `parts` obeys the
/// borrowing builder's descriptor and UTF-8 requirements, and its array is
/// disjoint from `left` and its storage. A suffix may borrow `left` directly,
/// through its handle or initialized bytes in its character storage, without
/// acquiring another owner. Other inputs stay rooted, immutable, and disjoint
/// from the transferred Str object's representation throughout the call.
/// The returned handle owns one reference; allocation exhaustion is fatal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_concat_parts_owned(
    left: *const Str,
    parts: *const StringPart,
    count: usize,
) -> *const Str {
    // SAFETY: the caller transfers this exact, live Rc owner once.
    let mut owner = unsafe { Rc::from_raw(left) };
    let parts = if count == 0 {
        &[]
    } else {
        // SAFETY: the caller provides this initialized, disjoint descriptor array.
        unsafe { std::slice::from_raw_parts(parts, count) }
    };
    let length = concatenated_length(
        owner.len(),
        parts.iter().map(|part| {
            // SAFETY: the consumed owner also roots direct self-borrowed fragments.
            unsafe { part.as_str() }.len()
        }),
    )
    .unwrap_or_else(|| fail("MemoryError", "string is too large"));
    if length == owner.len() {
        return Rc::into_raw(owner);
    }

    // Raw byte fragments need not acquire an Rc, so strong uniqueness alone is
    // insufficient. Include spare capacity: initialized spans there can also be
    // invalidated by reallocation. No borrowed view survives the mutable access.
    let start = owner.as_ptr() as usize;
    let end = start.saturating_add(owner.capacity());
    let borrows_left = parts.iter().any(|part| {
        if part.length == usize::MAX {
            part.data == left.cast()
        } else {
            let part_start = part.data as usize;
            part.length != 0 && part_start < end && start < part_start.saturating_add(part.length)
        }
    });
    if !borrows_left && let Some(value) = Rc::get_mut(&mut owner) {
        value
            .try_reserve(length - value.len())
            .unwrap_or_else(|_| fail("MemoryError", "string allocation failed"));
        {
            // SAFETY: reservation covers the complete suffix, and alias screening
            // excludes every borrowed source from the storage being mutated. The
            // spare slice begins after the initialized prefix, which stays intact.
            let output = unsafe { value.spare_capacity_mut() }
                .as_mut_ptr()
                .cast::<u8>();
            let mut offset = 0;
            for part in parts {
                // SAFETY: all inputs remain rooted, immutable, and disjoint.
                let text = unsafe { part.as_str() };
                // SAFETY: the checked suffix length bounds every copy. As in the
                // fresh builder, the final inline tag byte is overwritten only
                // with the final byte of complete, initialized UTF-8. Do not read
                // or move the string until its length is published below.
                unsafe {
                    std::ptr::copy_nonoverlapping(text.as_ptr(), output.add(offset), text.len());
                }
                offset += text.len();
            }
        }
        // SAFETY: the original prefix and complete UTF-8 suffix are initialized
        // within the reserved allocation, including a completely full inline one.
        unsafe { value.set_len(length) };
        return Rc::into_raw(owner);
    }

    let mut result = Str::try_with_capacity(length)
        .unwrap_or_else(|_| fail("MemoryError", "string allocation failed"));
    result.push_str(&owner);
    for part in parts {
        // SAFETY: all inputs, including self-borrows, remain rooted and unchanged.
        result.push_str(unsafe { part.as_str() });
    }
    Rc::into_raw(Rc::new(result))
}

/// Appends one disjoint UTF-8 span, retaining unique string storage and growing
/// it geometrically only when the existing capacity cannot hold the suffix.
///
/// # Safety
/// `left` transfers exactly one live, non-null Rc<Str> owner. `data` points to
/// `length` immutable valid UTF-8 bytes disjoint from both the Str object and
/// its character allocation, including spare capacity. The span obeys Rust
/// slice size limits and remains readable until return; it may be null only
/// when `length` is zero. The returned handle owns one reference. Allocation
/// exhaustion is fatal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_append_utf8_owned(
    left: *const Str,
    data: *const u8,
    length: usize,
) -> *const Str {
    if length == 0 {
        return left;
    }
    // SAFETY: the caller transfers this exact live owner, including on copying.
    let mut owner = unsafe { Rc::from_raw(left) };
    if let Some(value) = Rc::get_mut(&mut owner)
        && value.is_heap_allocated()
    {
        let initial = value.len();
        if length <= value.capacity() - initial {
            // SAFETY: the capacity bound proves the total fits both usize and
            // the live allocation. The complete suffix is disjoint and valid
            // UTF-8; its heap destination cannot overwrite the Str's inline tag.
            unsafe {
                let output = value.as_mut_ptr().add(initial);
                std::ptr::copy_nonoverlapping(data, output, length);
                value.set_len(initial + length);
            }
            return Rc::into_raw(owner);
        }
    }
    // SAFETY: the unchanged owner and nonempty disjoint UTF-8 span are transferred.
    unsafe { append_utf8_slow(owner, data, length) }
}

/// Keeps inline/static storage, growth, and shared-owner copying outside the
/// common append into a uniquely owned, already reserved heap buffer.
///
/// # Safety
/// `length` is nonzero and the span obeys `relar_str_append_utf8_owned`'s contract.
#[cold]
#[inline(never)]
unsafe fn append_utf8_slow(mut owner: Rc<Str>, data: *const u8, length: usize) -> *const Str {
    let total = concatenated_length(owner.len(), [length])
        .unwrap_or_else(|| fail("MemoryError", "string is too large"));
    if let Some(value) = Rc::get_mut(&mut owner) {
        if value.capacity() < total {
            value
                .try_reserve(length)
                .unwrap_or_else(|_| fail("MemoryError", "string allocation failed"));
        }
        // SAFETY: capacity covers the checked total and the caller guarantees
        // a disjoint initialized suffix. Do not inspect or move the string after
        // copying its final inline tag byte until the complete length is set.
        unsafe {
            let output = value.spare_capacity_mut().as_mut_ptr().cast::<u8>();
            std::ptr::copy_nonoverlapping(data, output, length);
            value.set_len(total);
        }
        Rc::into_raw(owner)
    } else {
        // SAFETY: the validated nonempty UTF-8 span remains readable and disjoint.
        let suffix =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(data, length)) };
        let mut result = Str::try_with_capacity(total)
            .unwrap_or_else(|_| fail("MemoryError", "string allocation failed"));
        result.push_str(&owner);
        result.push_str(suffix);
        Rc::into_raw(Rc::new(result))
    }
}

/// Checks both arithmetic overflow and Rust's maximum contiguous allocation size.
fn concatenated_length(initial: usize, lengths: impl IntoIterator<Item = usize>) -> Option<usize> {
    lengths
        .into_iter()
        .try_fold(initial, usize::checked_add)
        .filter(|&length| length <= isize::MAX as usize)
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

/// Removes a trailing set of Unicode scalars, or Python whitespace for null.
///
/// # Safety
/// `value` roots a live string and `chars` is either another rooted string or
/// null. The returned string owns an independent reference, including no-ops.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_rstrip(value: *const Str, chars: *const Str) -> *const Str {
    // SAFETY: both immutable strings remain rooted throughout this call.
    let (source, chars) = unsafe { (&*value, chars.as_ref()) };
    let stripped = source.trim_end_matches(|character: char| {
        chars.map_or_else(
            || python_whitespace(character),
            |chars| chars.contains(character),
        )
    });
    if stripped.len() == source.len() {
        // SAFETY: acquiring an independent reference preserves the caller's owner.
        unsafe { Rc::increment_strong_count(value) };
        value
    } else {
        Rc::into_raw(Rc::new(Str::new(stripped)))
    }
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
            let source = Rc::into_raw(Rc::new(Str::new(&padding[..split])));
            let suffix = &parts[1..];
            // SAFETY: source transfers its only owner, and the remaining suffix
            // fragments are disjoint. Empty trailing parts also exercise reading
            // descriptors after the last full-inline byte overwrites its tag.
            let mut reused = unsafe {
                Rc::from_raw(relar_str_concat_parts_owned(
                    source,
                    suffix.as_ptr(),
                    suffix.len(),
                ))
            };
            assert_eq!(Rc::as_ptr(&reused), source);
            drop(owned);
            let expected = format!("{padding}{ending}");
            assert_eq!(result.len(), length);
            assert_eq!(result.as_str(), expected);
            assert_eq!(result.is_heap_allocated(), length > 24);
            assert_eq!(reused.as_str(), expected);
            assert_eq!(reused.is_heap_allocated(), length > 24);
            result.push_str("!");
            Rc::get_mut(&mut reused).unwrap().push_str("!");
            assert_eq!(result.as_str(), format!("{expected}!"));
            assert_eq!(reused.as_str(), result.as_str());
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
    fn indexed_string_parts_preserve_unicode_scratch_bounds_and_input_lifetimes() {
        use std::mem::MaybeUninit;

        for (position, expected) in ["a", "é", "\0", "e", "\u{301}", "🦀"]
            .into_iter()
            .enumerate()
        {
            for at in [position as i64, position as i64 - 6] {
                let source = Rc::new(Str::new("aé\0e\u{301}🦀"));
                let weak = Rc::downgrade(&source);
                let at = Int::small(at).unwrap();
                let mut buffer = [0xcc_u8; 6];
                let scratch = buffer[1..5].as_mut_ptr();
                let mut part = MaybeUninit::<StringPart>::uninit();
                // SAFETY: rooted inputs and disjoint outputs provide a four-byte
                // scratch span and one descriptor; status precedes all output reads.
                let status = unsafe {
                    relar_str_get_part(Rc::as_ptr(&source), at.0, scratch, part.as_mut_ptr())
                };
                assert_eq!(status, 1);
                assert_eq!(exceptions::relar_error_pending(), 0);
                // SAFETY: successful status initializes the entire descriptor.
                let part = unsafe { part.assume_init() };
                assert_eq!(part.data, scratch.cast_const());
                assert_eq!(part.length, expected.len());
                assert_eq!(buffer[0], 0xcc);
                assert!(
                    buffer[1 + expected.len()..]
                        .iter()
                        .all(|byte| *byte == 0xcc)
                );
                drop((source, at));
                assert!(weak.upgrade().is_none());
                // SAFETY: the descriptor borrows caller scratch, so neither
                // released input is needed to concatenate its initialized bytes.
                let result = unsafe { Rc::from_raw(relar_str_concat_parts(&part, 1)) };
                assert_eq!(result.as_str(), expected);
            }
        }
    }

    #[test]
    fn indexed_string_part_errors_preserve_outputs_and_match_string_get() {
        use num_bigint::BigInt;

        let bounds = "string index out of range";
        let overflow = "cannot fit 'int' into an index-sized integer";
        for (text, at, expected) in [
            ("aé\0e\u{301}🦀", BigInt::from(6), bounds),
            ("aé\0e\u{301}🦀", BigInt::from(-7), bounds),
            ("", BigInt::from(0), bounds),
            ("", BigInt::from(-1), bounds),
            ("a", BigInt::from(isize::MIN), bounds),
            ("a", BigInt::from(isize::MAX), bounds),
            ("a", BigInt::from(1) << 400_u32, overflow),
            ("a", -(BigInt::from(1) << 400_u32), overflow),
        ] {
            let source = Rc::new(Str::new(text));
            let at = Int::new(at);
            let mut buffer = [0xcc_u8; 6];
            let mut part = StringPart {
                data: "unchanged".as_ptr(),
                length: "unchanged".len(),
            };
            // SAFETY: all inputs and writable outputs satisfy the normal ABI,
            // despite the language-level index being outside the accepted range.
            let status = unsafe {
                relar_str_get_part(
                    Rc::as_ptr(&source),
                    at.0,
                    buffer[1..5].as_mut_ptr(),
                    &mut part,
                )
            };
            assert_eq!(status, 0);
            assert_eq!(buffer, [0xcc; 6]);
            assert_eq!(part.data, "unchanged".as_ptr());
            assert_eq!(part.length, "unchanged".len());
            assert_eq!(Rc::strong_count(&source), 1);
            assert_eq!(exceptions::relar_error_pending(), 1);
            // SAFETY: taking pending transfers one exception owner; retrieving
            // its message returns one independent string owner, both adopted once.
            unsafe {
                let error = Rc::from_raw(exceptions::relar_error_take());
                assert_eq!(
                    exceptions::relar_exception_matches(
                        Rc::as_ptr(&error),
                        Kind::IndexError as u32
                    ),
                    1,
                );
                let message = Rc::from_raw(exceptions::relar_exception_message(Rc::as_ptr(&error)));
                assert_eq!(message.as_str(), expected);
            }
            assert_eq!(exceptions::relar_error_pending(), 0);
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
    fn owned_concatenation_reuses_unique_owner_and_reserved_heap_storage() {
        let mut text = Str::with_capacity(256);
        text.push_str("a prefix exceeding inline string capacity");
        let source = Rc::new(text);
        let storage = source.as_ptr();
        let capacity = source.capacity();
        let suffix = Rc::new(Str::new("🦀"));
        let literal = "é\0";
        let parts = [
            StringPart {
                data: literal.as_ptr(),
                length: literal.len(),
            },
            StringPart {
                data: Rc::as_ptr(&suffix).cast(),
                length: usize::MAX,
            },
        ];
        let source = Rc::into_raw(source);
        // SAFETY: source transfers its only owner; suffix and literal are disjoint
        // rooted inputs, and the returned owner is adopted exactly once.
        let result = unsafe {
            Rc::from_raw(relar_str_concat_parts_owned(
                source,
                parts.as_ptr(),
                parts.len(),
            ))
        };
        assert_eq!(Rc::as_ptr(&result), source);
        assert_eq!(result.as_ptr(), storage);
        assert_eq!(result.capacity(), capacity);
        assert_eq!(Rc::strong_count(&result), 1);
        assert_eq!(Rc::strong_count(&suffix), 1);
        assert_eq!(
            result.as_str(),
            "a prefix exceeding inline string capacityé\0🦀"
        );
    }

    #[test]
    fn owned_concatenation_grows_geometrically_across_inline_and_heap_storage() {
        let mut source = Rc::new(Str::new(""));
        let original = Rc::as_ptr(&source);
        let literal = "é\0";
        let part = StringPart {
            data: literal.as_ptr(),
            length: literal.len(),
        };
        let mut growths = 0;
        for length in (3..=3072).step_by(3) {
            let capacity = source.capacity();
            // SAFETY: this disjoint literal outlives every call, which consumes
            // the sole owner and returns exactly one replacement owner.
            source = unsafe {
                Rc::from_raw(relar_str_concat_parts_owned(Rc::into_raw(source), &part, 1))
            };
            assert_eq!(Rc::as_ptr(&source), original);
            assert_eq!(source.len(), length);
            assert_eq!(source.is_heap_allocated(), length > 24);
            growths += usize::from(source.capacity() != capacity);
        }
        assert!(
            growths < 32,
            "capacity grew {growths} times for 1024 appends"
        );
        assert_eq!(source.as_str(), "é\0".repeat(1024));
    }

    #[test]
    fn owned_concatenation_preserves_shared_alias_and_consumes_one_owner() {
        for count in [1, 2] {
            let alias = Rc::new(Str::new("α\0🦀"));
            let weak = Rc::downgrade(&alias);
            let source = Rc::into_raw(alias.clone());
            let parts = [
                StringPart {
                    data: ":".as_ptr(),
                    length: 1,
                },
                StringPart {
                    data: source.cast(),
                    length: usize::MAX,
                },
            ];
            // SAFETY: source transfers one of the two references, and alias roots
            // any self-borrowed suffix. Disjoint-only suffixes must also copy.
            let result = unsafe {
                Rc::from_raw(relar_str_concat_parts_owned(source, parts.as_ptr(), count))
            };
            assert_ne!(Rc::as_ptr(&result), source);
            assert_eq!(Rc::strong_count(&alias), 1);
            assert_eq!(alias.as_str(), "α\0🦀");
            drop(alias);
            assert!(weak.upgrade().is_none());
            assert_eq!(
                result.as_str(),
                if count == 1 {
                    "α\0🦀:"
                } else {
                    "α\0🦀:α\0🦀"
                }
            );
        }
    }

    #[test]
    fn owned_concatenation_roots_unowned_self_handles_and_byte_spans() {
        for text in ["α\0🦀", "α\0🦀 with a suffix exceeding inline capacity"] {
            for handle in [false, true] {
                let source = Rc::new(Str::new(text));
                let part = if handle {
                    StringPart {
                        data: Rc::as_ptr(&source).cast(),
                        length: usize::MAX,
                    }
                } else {
                    StringPart {
                        data: source.as_ptr(),
                        length: source.len(),
                    }
                };
                let source = Rc::into_raw(source);
                // SAFETY: the consumed sole owner roots its directly borrowed
                // handle/span. No extra Rc or Weak masks the aliasing case.
                let result =
                    unsafe { Rc::from_raw(relar_str_concat_parts_owned(source, &part, 1)) };
                assert_ne!(Rc::as_ptr(&result), source);
                assert_eq!(result.as_str(), text.repeat(2));
            }
        }

        let mut text = Str::new("a".repeat(64));
        let storage = text.as_ptr();
        text.truncate(8);
        assert_eq!(text.as_ptr(), storage);
        let source = Rc::into_raw(Rc::new(text));
        let part = StringPart {
            // SAFETY: truncation leaves this initialized heap allocation in place.
            data: unsafe { storage.add(32) },
            length: 16,
        };
        // SAFETY: the source owner roots initialized UTF-8 bytes in spare capacity.
        let result = unsafe { Rc::from_raw(relar_str_concat_parts_owned(source, &part, 1)) };
        assert_ne!(Rc::as_ptr(&result), source);
        assert_eq!(result.as_str(), "a".repeat(24));
    }

    #[test]
    fn owned_empty_suffix_preserves_handle_and_shared_ownership() {
        let alias = Rc::new(Str::new("unchanged"));
        for count in [0, 1] {
            let empty = StringPart {
                data: std::ptr::null(),
                length: 0,
            };
            let parts = if count == 0 { std::ptr::null() } else { &empty };
            let source = Rc::into_raw(alias.clone());
            // SAFETY: a null array is allowed only for zero count; a zero-length
            // byte span may be null. Exactly one source owner is transferred.
            let result =
                unsafe { Rc::from_raw(relar_str_concat_parts_owned(source, parts, count)) };
            assert_eq!(Rc::as_ptr(&result), source);
            assert_eq!(Rc::strong_count(&alias), 2);
            assert_eq!(result.as_str(), "unchanged");
        }
        assert_eq!(Rc::strong_count(&alias), 1);
    }

    #[test]
    fn owned_utf8_append_reuses_unique_owner_and_reserved_heap_storage() {
        let mut text = Str::with_capacity(256);
        text.push_str("a prefix exceeding inline string capacity");
        let source = Rc::new(text);
        let storage = source.as_ptr();
        let capacity = source.capacity();
        let source = Rc::into_raw(source);
        let suffix = "é\0🦀";
        // SAFETY: the literal is disjoint, and the only source owner is consumed.
        let result = unsafe {
            Rc::from_raw(relar_str_append_utf8_owned(
                source,
                suffix.as_ptr(),
                suffix.len(),
            ))
        };
        assert_eq!(Rc::as_ptr(&result), source);
        assert_eq!(result.as_ptr(), storage);
        assert_eq!(result.capacity(), capacity);
        assert_eq!(Rc::strong_count(&result), 1);
        assert_eq!(
            result.as_str(),
            "a prefix exceeding inline string capacityé\0🦀"
        );
    }

    #[test]
    fn owned_utf8_append_preserves_shared_and_weak_aliases() {
        let heap_prefix = "a shared prefix exceeding inline string capacity α\0🦀";
        for (text, shared) in [
            ("α\0🦀", false),
            ("α\0🦀", true),
            (heap_prefix, false),
            (heap_prefix, true),
        ] {
            let source = Rc::new(Str::new(text));
            let alias = shared.then(|| source.clone());
            let weak = Rc::downgrade(&source);
            let source = Rc::into_raw(source);
            let suffix = "é!";
            // SAFETY: the literal is disjoint and exactly one owner is transferred.
            let result = unsafe {
                Rc::from_raw(relar_str_append_utf8_owned(
                    source,
                    suffix.as_ptr(),
                    suffix.len(),
                ))
            };
            assert_ne!(Rc::as_ptr(&result), source);
            assert_eq!(Rc::strong_count(&result), 1);
            if let Some(alias) = alias {
                assert_eq!(Rc::strong_count(&alias), 1);
                assert_eq!(alias.as_str(), text);
                drop(alias);
            }
            assert!(weak.upgrade().is_none());
            assert_eq!(result.as_str(), format!("{text}é!"));
        }
    }

    #[test]
    fn owned_utf8_append_initializes_inline_tag_and_heap_boundaries() {
        for (length, ending) in [
            (0, ""),
            (23, "é"),
            (24, "z"),
            (24, "\0"),
            (24, "é"),
            (24, "🦀"),
            (25, "é"),
            (25, "🦀"),
        ] {
            let padding = "a".repeat(length - ending.len());
            let source = Rc::into_raw(Rc::new(Str::new(&padding)));
            // SAFETY: the complete literal suffix is disjoint, including when it
            // fills the final inline tag byte. Exactly one source owner moves.
            let mut result = unsafe {
                Rc::from_raw(relar_str_append_utf8_owned(
                    source,
                    ending.as_ptr(),
                    ending.len(),
                ))
            };
            assert_eq!(Rc::as_ptr(&result), source);
            assert_eq!(result.as_str(), format!("{padding}{ending}"));
            assert_eq!(result.len(), length);
            assert_eq!(result.is_heap_allocated(), length > 24);
            Rc::get_mut(&mut result).unwrap().push_str("!");
            assert_eq!(result.as_str(), format!("{padding}{ending}!"));
        }

        let literal = "a static prefix exceeding inline string capacity";
        let source = Rc::new(Str::const_new(literal));
        assert_eq!(source.as_ptr(), literal.as_ptr());
        assert!(!source.is_heap_allocated());
        let source = Rc::into_raw(source);
        let suffix = "é\0🦀";
        // SAFETY: both static spans are disjoint, and the only source owner is
        // transferred. Mutation must first copy the read-only static storage.
        let result = unsafe {
            Rc::from_raw(relar_str_append_utf8_owned(
                source,
                suffix.as_ptr(),
                suffix.len(),
            ))
        };
        assert_eq!(Rc::as_ptr(&result), source);
        assert_ne!(result.as_ptr(), literal.as_ptr());
        assert!(result.is_heap_allocated());
        assert_eq!(result.as_str(), format!("{literal}{suffix}"));
    }

    #[test]
    fn owned_utf8_empty_append_preserves_shared_owner_without_reading_null() {
        let alias = Rc::new(Str::new("unchanged α\0🦀"));
        let source = Rc::into_raw(alias.clone());
        // SAFETY: an empty suffix may be null; one of two owners is transferred.
        let result =
            unsafe { Rc::from_raw(relar_str_append_utf8_owned(source, std::ptr::null(), 0)) };
        assert_eq!(Rc::as_ptr(&result), source);
        assert_eq!(Rc::strong_count(&alias), 2);
        assert_eq!(result.as_str(), "unchanged α\0🦀");
        drop(result);
        assert_eq!(Rc::strong_count(&alias), 1);
    }

    #[test]
    fn concatenation_length_rejects_allocation_and_arithmetic_overflow() {
        assert_eq!(concatenated_length(0, []), Some(0));
        assert_eq!(concatenated_length(20, [1, 2]), Some(23));
        assert_eq!(
            concatenated_length(isize::MAX as usize, [0]),
            Some(isize::MAX as usize)
        );
        assert_eq!(concatenated_length(isize::MAX as usize, [1]), None);
        assert_eq!(concatenated_length(0, [isize::MAX as usize, 1]), None);
        assert_eq!(concatenated_length(usize::MAX, [1]), None);
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
    fn proven_sequence_reads_borrow_exact_scalar_and_optional_words() {
        use num_bigint::BigInt;

        let text = Rc::new(Str::from("borrowed sequence word"));
        let big = Rc::new(BigInt::from(1_u128 << 100));
        let float = Rc::new(-0.0_f64);
        let text_weak = Rc::downgrade(&text);
        let big_weak = Rc::downgrade(&big);
        let float_weak = Rc::downgrade(&float);
        let cases = [
            (0, vec![0]),
            (1, vec![0, 1]),
            (257, vec![0, 1, 2]),
            (258, vec![0, 1, Rc::as_ptr(&big) as usize]),
            (259, vec![0, Rc::as_ptr(&text) as usize]),
            (263, vec![0, Rc::as_ptr(&float) as usize]),
            (
                7,
                vec![
                    0,
                    (-0.0_f64).to_bits() as usize,
                    f64::NAN.to_bits() as usize,
                ],
            ),
        ];
        for (kind, words) in cases {
            // SAFETY: each descriptor matches its rooted values. All accesses
            // are in bounds and their borrowed words expire before mutation.
            unsafe {
                let sequence = relar_seq_new(kind);
                for &word in &words {
                    relar_seq_append(sequence, word, kind);
                }
                let view = relar_seq_data_readonly(sequence);
                for (index, &word) in words.iter().enumerate() {
                    assert_eq!(relar_seq_borrow_index_proven(sequence, index), word);
                    assert_eq!(view.add(index).read(), word);
                }
                relar_object_release(sequence);
            }
        }
        assert_eq!(Rc::strong_count(&text), 1);
        assert_eq!(Rc::strong_count(&big), 1);
        assert_eq!(Rc::strong_count(&float), 1);
        drop((text, big, float));
        assert!(text_weak.upgrade().is_none());
        assert!(big_weak.upgrade().is_none());
        assert!(float_weak.upgrade().is_none());
        assert_eq!(exceptions::relar_error_pending(), 0);
    }

    #[test]
    fn readonly_sequence_views_keep_empty_and_owned_element_lifetimes() {
        let text = Rc::new(Str::from("rooted until the loop finishes"));
        let weak = Rc::downgrade(&text);
        // SAFETY: views are never read out of bounds and their last uses precede
        // any storage mutation or owner release. Acquired words outlive the view.
        unsafe {
            let sequence = relar_seq_new(3);
            let empty = relar_seq_data_readonly(sequence);
            assert!(!empty.is_null());
            assert_eq!(empty as usize % std::mem::align_of::<usize>(), 0);
            assert_eq!(relar_seq_len(sequence), 1); // Tagged zero; no load.
            relar_seq_append(sequence, Rc::as_ptr(&text) as usize, 3);
            let view = relar_seq_data_readonly(sequence);
            assert_eq!(Rc::strong_count(&text), 2);
            let word = view.read();
            let saved = acquire_rc::<Str>(word);
            assert_eq!(Rc::strong_count(&text), 3);
            relar_object_release(sequence);
            drop(text);
            assert_eq!(saved.as_str(), "rooted until the loop finishes");
            drop(saved);
        }
        assert!(weak.upgrade().is_none());
        assert_eq!(exceptions::relar_error_pending(), 0);
    }

    #[test]
    fn proven_sequence_record_is_acquired_before_its_source_owner_is_removed() {
        use records::{
            RecordDescriptor, relar_record_new, relar_record_release, relar_record_retain,
        };

        let kinds = [3];
        let descriptor = RecordDescriptor {
            length: 1,
            kinds: kinds.as_ptr(),
        };
        let text = Rc::new(Str::from("retained before mutation"));
        let weak = Rc::downgrade(&text);
        // SAFETY: immutable metadata outlives the record. The borrowed read is
        // immediately acquired, matching compiler lowering, before removing
        // the source owner. Every independent record owner is released once.
        unsafe {
            let record = relar_record_new(&descriptor);
            record.cast::<*const Str>().write(Rc::into_raw(text));
            let sequence = relar_seq_new(6);
            relar_seq_append(sequence, record as usize, 6);
            relar_record_release(record);
            let saved = relar_seq_borrow_index_proven(sequence, 0) as *const usize;
            relar_record_retain(saved);
            relar_record_release(relar_seq_pop(sequence) as *const usize);
            relar_object_release(sequence);
            assert_eq!(
                (&*saved.cast::<*const Str>().read()).as_str(),
                "retained before mutation"
            );
            relar_record_release(saved);
        }
        assert!(weak.upgrade().is_none());
        assert_eq!(exceptions::relar_error_pending(), 0);
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
