//! A closed str/bytes sum reuses the alignment bit of its exact Rc payload.
//! Zero is reserved for absent optionals and partially initialized storage.

use std::rc::Rc;

use super::{Bytes, Str};

const _: () = assert!(align_of::<Str>() >= 2 && align_of::<Bytes>() >= 2);

/// # Safety
/// `value` is zero or a rooted Rc<Str> pointer (tag 0) or Rc<Bytes> pointer
/// (tag 1). Pointer addresses were exposed before adding the low tag bit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_union_retain(value: usize) {
    if value == 0 {
        return;
    }
    // SAFETY: clearing the tag restores the exact rooted allocation's address.
    unsafe {
        if value & 1 == 0 {
            Rc::increment_strong_count(value as *const Str);
        } else {
            Rc::increment_strong_count((value & !1) as *const Bytes);
        }
    }
}

/// # Safety
/// Transfers one owner with the same encoding as `relar_union_retain`, or zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_union_release(value: usize) {
    if value == 0 {
        return;
    }
    // SAFETY: the caller transfers the owner selected by the immutable tag.
    unsafe {
        if value & 1 == 0 {
            Rc::decrement_strong_count(value as *const Str);
        } else {
            Rc::decrement_strong_count((value & !1) as *const Bytes);
        }
    }
}

/// # Safety
/// Borrows the exact rooted payload encoded by `relar_union_retain`; zero is false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_union_truthy(value: usize) -> u8 {
    if value == 0 {
        return 0;
    }
    // SAFETY: neither exact immutable payload escapes the caller's root.
    unsafe {
        u8::from(if value & 1 == 0 {
            !(*(value as *const Str)).is_empty()
        } else {
            !(*((value & !1) as *const Bytes)).is_empty()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::words::WordKind;

    #[test]
    fn tagged_members_keep_exact_owners_and_truth_values() {
        let text = Rc::new(Str::from("text"));
        let bytes = Rc::new(Bytes::new());
        let text_word = Rc::as_ptr(&text) as usize;
        let bytes_word = Rc::as_ptr(&bytes) as usize | 1;
        // SAFETY: both locals root their exact tagged payloads throughout.
        unsafe {
            assert_eq!(relar_union_truthy(text_word), 1);
            assert_eq!(relar_union_truthy(bytes_word), 0);
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
