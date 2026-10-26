//! Closed unions use three alignment bits of their exact heap payload.
//! Zero is reserved for absent optionals and partially initialized storage.
//! Words one and two represent the bool singletons; other nonzero words carry a member tag.

use super::words::WordKind;
use super::{Bytes, Int, Object, Str, object};
use crate::shared::Shared;
use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};

const _: () = assert!(
    align_of::<Str>() >= 8
        && align_of::<Bytes>() >= 8
        && align_of::<Object>() >= 8
        && align_of::<usize>() >= 8
);

/// Recovers the original address and the runtime descriptor for its owner.
pub(super) fn member(value: usize) -> (usize, WordKind) {
    // No live allocation has a null address. These otherwise unused tagged
    // words encode Python's two bool singletons, just like nullable bool.
    if matches!(value, 1 | 2) {
        return (value - 1, WordKind::new(1));
    }
    let kind = match value & 7 {
        0 => 3,                                      // str
        1 => 8,                                      // bytes
        2 => 5,                                      // list[str] or frozenset[str]
        3 => 5,                                      // always-true Object (exception or LogRecord)
        4 => 6,                                      // fixed record
        6 => 5,                                      // pathlib.Path
        5 => return (value >> 3, WordKind::new(10)), // ordinary Enum tag
        7 => 2, // aligned Shared<BigInt>, even for an ordinary immediate integer
        _ => unreachable!(),
    };
    (value & !7, WordKind::new(kind))
}

/// An immediate integer already uses its low bit. Give it aligned storage
/// inside the union; an existing large integer transfers its allocation.
pub(super) fn integer(value: Int) -> usize {
    let word = match value.as_small() {
        Some(small) => Shared::into_raw(Shared::new(BigInt::from(small))) as usize,
        None => value.into_raw(),
    };
    word | 7
}

/// # Safety
/// Takes ownership of an exact integer word, or zero for an absent optional.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_union_from_int(word: usize) -> usize {
    if word == 0 { 0 } else { integer(Int(word)) }
}

/// # Safety
/// Takes ownership of a present bool or integer member; restores the canonical
/// immediate representation before it is used as an integer or dictionary key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_union_into_int(word: usize) -> usize {
    if matches!(word, 1 | 2) {
        return Int::small(i64::from(word == 2)).unwrap().into_raw();
    }
    let value = Int(word & !7);
    value
        .big()
        .to_i64()
        .and_then(Int::small)
        .unwrap_or(value)
        .into_raw()
}

/// # Safety
/// `value` is zero or a rooted, correctly tagged text union member.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_union_retain(value: usize) {
    let (word, kind) = member(value);
    // SAFETY: removing the tag restores the exact rooted allocation's address.
    unsafe { kind.retain(word) }
}

/// # Safety
/// Transfers one owner with the same encoding as `imre_union_retain`, or zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_union_release(value: usize) {
    let (word, kind) = member(value);
    // SAFETY: the caller transfers the owner selected by the immutable tag.
    unsafe { kind.release(word) }
}

/// # Safety
/// Borrows the exact rooted union member. Zero is false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_union_truthy(value: usize) -> u32 {
    if value <= 2 {
        return u32::from(value == 2);
    }
    let (word, _) = member(value);
    // SAFETY: the caller roots the selected member and no user code runs here.
    unsafe {
        u32::from(match value & 7 {
            0 => !(*(word as *const Str)).is_empty(),
            1 => !(*(word as *const Bytes)).is_empty(),
            2 => object(word as *const Object).0.with(|s| match s {
                super::Storage::Set(set) => !set.is_empty(),
                _ => crate::finish(s.sequence().map(|words| !words.values.is_empty()), false),
            }),
            7 => !(*(word as *const BigInt)).is_zero(),
            _ => true,
        })
    }
}

/// Reads the list or tuple length without copying its elements.
///
/// # Safety
/// The caller roots a tagged str list (2) or a tuple of strs (4).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_iterable_len(value: usize) -> usize {
    let (word, _) = member(value);
    if value & 7 == 2 {
        // SAFETY: the original list is rooted throughout this length read.
        return unsafe { super::imre_seq_len(word as *const Object) };
    }
    debug_assert_eq!(value & 7, 4);
    // SAFETY: the tuple owns a live descriptor with its fixed, bounded arity.
    (unsafe { super::records::descriptor(word as *const usize).length } << 1) | 1
}

/// Returns the original list for live iteration, or copies immutable tuple field references.
/// The tuple itself remains available wherever the Iterable is stored or formatted.
///
/// # Safety
/// The caller roots an exact list of strings (tag 2) or a tuple containing only strings (tag 4).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_iterable_list(value: usize) -> *const Object {
    let (word, _) = member(value);
    if value & 7 == 2 {
        // SAFETY: the tagged value roots this live sequence handle.
        return crate::ObjectOwner::into_raw(unsafe { crate::ObjectOwner::acquire_word(word) });
    }
    debug_assert_eq!(value & 7, 4);
    let fields = word as *const usize;
    // SAFETY: the descriptor describes the rooted tuple payload and all its initialized str fields.
    let entries =
        unsafe { std::slice::from_raw_parts(fields, super::records::descriptor(fields).length) };
    let mut words = super::Words::new(3);
    if words.values.try_reserve_exact(entries.len()).is_err() {
        return super::finish(Err(super::Error::allocation()), std::ptr::null());
    }
    words.values.extend(entries.iter().map(|&entry| {
        // SAFETY: the immutable tuple keeps this str alive while taking the list's own reference.
        unsafe { WordKind::new(3).acquire(entry) }.into_word()
    }));
    crate::ObjectOwner::into_raw(words.into_object())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StrOwner;

    #[test]
    fn tagged_members_keep_exact_owners_and_truth_values() {
        let text = StrOwner::new(Str::from("text"));
        let bytes = crate::BytesOwner::new(Bytes::new());
        let text_word = StrOwner::as_ptr(&text) as usize;
        let bytes_word = crate::BytesOwner::as_ptr(&bytes) as usize | 1;
        // SAFETY: both locals root their exact tagged payloads throughout.
        unsafe {
            assert_eq!(imre_union_truthy(text_word), 1);
            assert_eq!(imre_union_truthy(bytes_word), 0);
            let text_owner = WordKind::new(9).acquire(text_word);
            let bytes_owner = WordKind::new(9).acquire(bytes_word);
            assert_eq!(StrOwner::strong_count(&text), 2);
            assert_eq!(crate::BytesOwner::strong_count(&bytes), 2);
            drop(text_owner);
            drop(bytes_owner);
            assert_eq!(StrOwner::strong_count(&text), 1);
            assert_eq!(crate::BytesOwner::strong_count(&bytes), 1);
            drop(WordKind::new(256 | 9).acquire(0));
        }
    }
}
