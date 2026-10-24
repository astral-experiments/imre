//! Closed text unions reuse two alignment bits of their exact Rc payload.
//! Zero is reserved for absent optionals and partially initialized storage.

use super::words::WordKind;
use super::{Bytes, Object, Str, object};

const _: () = assert!(
    align_of::<Str>() >= 8
        && align_of::<Bytes>() >= 8
        && align_of::<Object>() >= 8
        && align_of::<usize>() >= 8
);

/// Recovers the original address and the runtime descriptor for its owner.
pub(super) fn member(value: usize) -> (usize, WordKind) {
    let kind = match value & 7 {
        0 => 3,                                      // str
        1 => 8,                                      // bytes
        2 => 5,                                      // list[str]
        3 => 5,                                      // exception
        4 => 6,                                      // fixed record
        6 => 5,                                      // pathlib.Path
        5 => return (value >> 3, WordKind::new(10)), // ordinary Enum tag
        _ => unreachable!("invalid text union tag"),
    };
    (value & !7, WordKind::new(kind))
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
    if value == 0 {
        return 0;
    }
    let (word, _) = member(value);
    // SAFETY: the caller roots the selected member and no user code runs here.
    unsafe {
        u32::from(match value & 7 {
            0 => !(*(word as *const Str)).is_empty(),
            1 => !(*(word as *const Bytes)).is_empty(),
            2 => object(word as *const Object)
                .0
                .with(|s| !s.sequence().values.is_empty()),
            _ => true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn tagged_members_keep_exact_owners_and_truth_values() {
        let text = Rc::new(Str::from("text"));
        let bytes = Rc::new(Bytes::new());
        let text_word = Rc::as_ptr(&text) as usize;
        let bytes_word = Rc::as_ptr(&bytes) as usize | 1;
        // SAFETY: both locals root their exact tagged payloads throughout.
        unsafe {
            assert_eq!(imre_union_truthy(text_word), 1);
            assert_eq!(imre_union_truthy(bytes_word), 0);
            let text_owner = WordKind::new(9).acquire(text_word);
            let bytes_owner = WordKind::new(9).acquire(bytes_word);
            assert_eq!(Rc::strong_count(&text), 2);
            assert_eq!(Rc::strong_count(&bytes), 2);
            drop(text_owner);
            drop(bytes_owner);
            assert_eq!(Rc::strong_count(&text), 1);
            assert_eq!(Rc::strong_count(&bytes), 1);
            drop(WordKind::new(256 | 9).acquire(0));
        }
    }
}
