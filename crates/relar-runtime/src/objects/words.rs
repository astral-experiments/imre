//! Ownership for concrete, compiler-described machine words.
//!
//! Stored containers carry one kind per homogeneous buffer, or one static kind
//! per record field. Only temporary owners carry their own kind. Bit 256 marks
//! a nullable value; optional booleans use 0=None, 1=False, and 2=True.
//! Bare float words hold all binary64 bits; nullable floats instead own an Rc<f64>.

use smallvec::SmallVec;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use super::{Bytes, Int, List, Object, Str, fail, records, unions};

pub(super) const NULLABLE: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WordKind(u32);

impl WordKind {
    pub(super) fn new(kind: u32) -> Self {
        if kind & !NULLABLE > 10 {
            fail("SystemError", "invalid concrete word representation");
        }
        Self(kind)
    }

    pub(super) fn raw(self) -> u32 {
        self.0
    }

    fn base(self) -> u32 {
        self.0 & !NULLABLE
    }

    /// Reads the compiler's concrete field storage without reinterpreting pointer
    /// bytes as an integer. Nonnullable reference fields use LLVM pointer stores;
    /// the explicit cast exposes their provenance before word-based reclamation.
    /// Nullable references already use integer words with an explicit ptrtoint.
    ///
    /// # Safety
    /// `field` is an aligned, readable record slot stored using this descriptor's
    /// LLVM representation, or its initial all-zero state.
    pub(super) unsafe fn read_record_field(self, field: *const usize) -> usize {
        if matches!(self.0, 3..=6 | 8) {
            // SAFETY: this exact descriptor denotes a pointer-typed field.
            unsafe { field.cast::<*const ()>().read() as usize }
        } else if self.0 == 7 {
            // SAFETY: nonnullable float fields use LLVM double storage.
            unsafe { field.cast::<f64>().read().to_bits() as usize }
        } else {
            // SAFETY: scalar, Int, and nullable fields use machine-word storage.
            unsafe { field.read() }
        }
    }

    /// Validates scalar and null encodings; handle provenance remains an ABI
    /// precondition, as it cannot be established from an address alone.
    fn validate(self, word: usize) {
        let nullable = self.0 & NULLABLE != 0;
        let valid = match self.base() {
            0 => word == 0,
            1 => word <= if nullable { 2 } else { 1 },
            2..=6 | 8..=10 => word != 0 || nullable,
            7 => true,
            _ => unreachable!(),
        };
        if !valid {
            fail("SystemError", "invalid concrete word value");
        }
    }

    /// Acquires a statically typed borrowed value before a container can replace
    /// its last owner. The returned temporary releases on every early return.
    pub(super) unsafe fn acquire(self, word: usize) -> OwnedWord {
        self.validate(word);
        // SAFETY: the caller roots a value with this exact representation.
        unsafe { self.retain(word) };
        OwnedWord { word, kind: self }
    }

    /// # Safety
    /// `word` uses this exact scalar representation or is a rooted handle of
    /// this kind. Zero is also the safe uninitialized state of owning fields.
    pub(super) unsafe fn retain(self, word: usize) {
        if word == 0 {
            return;
        }
        // SAFETY: the caller supplies each handle's exact allocation kind.
        unsafe {
            match self.base() {
                0 | 1 | 10 => {}
                2 => {
                    if word & 1 == 0 {
                        Rc::increment_strong_count(word as *const num_bigint::BigInt);
                    }
                }
                3 => Rc::increment_strong_count(word as *const Str),
                4 => Rc::increment_strong_count(word as *const List),
                5 => Rc::increment_strong_count(word as *const Object),
                6 => records::relar_record_retain(word as *const usize),
                7 if self.0 & NULLABLE != 0 => Rc::increment_strong_count(word as *const f64),
                7 => {}
                8 => Rc::increment_strong_count(word as *const Bytes),
                9 => unions::relar_union_retain(word),
                _ => unreachable!(),
            }
        }
    }

    /// # Safety
    /// Consumes one reference of this kind, or accepts zero for an absent or
    /// uninitialized value. Cleanup never installs a pending language error.
    pub(super) unsafe fn release(self, word: usize) {
        if word == 0 {
            return;
        }
        // SAFETY: the caller transfers the exact owned reference selected here.
        unsafe {
            match self.base() {
                0 | 1 | 10 => {}
                2 => drop(Int(word)),
                3 => Rc::decrement_strong_count(word as *const Str),
                4 => crate::relar_list_release(word as *const List),
                5 => Rc::decrement_strong_count(word as *const Object),
                6 => records::relar_record_release(word as *const usize),
                7 if self.0 & NULLABLE != 0 => Rc::decrement_strong_count(word as *const f64),
                7 => {}
                8 => Rc::decrement_strong_count(word as *const Bytes),
                9 => unions::relar_union_release(word),
                _ => unreachable!(),
            }
        }
    }
}

/// An acquired temporary; persistent buffers store only its transferred word.
pub(super) struct OwnedWord {
    pub(super) word: usize,
    pub(super) kind: WordKind,
}

impl OwnedWord {
    pub(super) fn into_word(self) -> usize {
        ManuallyDrop::new(self).word
    }

    /// # Safety
    /// Transfers an already-owned, correctly typed word into this temporary.
    pub(super) unsafe fn from_owned(word: usize, kind: WordKind) -> Self {
        Self { word, kind }
    }
}

impl Drop for OwnedWord {
    fn drop(&mut self) {
        // SAFETY: this temporary owns exactly the transferred reference.
        unsafe { self.kind.release(self.word) };
    }
}

/// A homogeneous buffer; the descriptor is shared by all element words.
/// Three words fit inline; spilling preserves their existing owned references.
pub(super) struct Words {
    pub(super) kind: WordKind,
    pub(super) values: SmallVec<[usize; 3]>,
}

impl Words {
    pub(super) fn new(kind: u32) -> Self {
        Self {
            kind: WordKind::new(kind),
            values: SmallVec::new(),
        }
    }

    /// Checks the private compiler/runtime contract before transferring a value.
    pub(super) fn check_kind(&self, kind: u32) {
        if self.kind != WordKind::new(kind) {
            fail("SystemError", "sequence element representation changed");
        }
    }
}

impl Drop for Words {
    fn drop(&mut self) {
        for word in self.values.drain(..) {
            // SAFETY: each buffer word owns its declared value independently.
            unsafe { self.kind.release(word) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn inline_words_fit_the_existing_object_layout() {
        assert_eq!(std::mem::size_of::<Words>(), 40);
        assert_eq!(std::mem::size_of::<Object>(), 48);
    }

    #[test]
    fn sequence_spill_preserves_aliases_views_and_transferred_owners() {
        use super::super::{
            relar_seq_append, relar_seq_data_readonly, relar_seq_len, relar_seq_new, relar_seq_pop,
        };

        // SAFETY: every handle stays rooted, readonly views end before mutation,
        // and each new or popped owner is adopted exactly once.
        unsafe {
            let child = Rc::from_raw(relar_seq_new(1));
            let weak = Rc::downgrade(&child);
            let word = Rc::as_ptr(&child) as usize;
            let sequence = Rc::from_raw(relar_seq_new(5));
            let alias = Rc::clone(&sequence);
            for _ in 0..3 {
                relar_seq_append(Rc::as_ptr(&sequence), word, 5);
            }
            assert_eq!(Rc::strong_count(&child), 4);
            assert!(
                !sequence
                    .0
                    .with(|storage| storage.sequence().values.spilled())
            );
            {
                let view = relar_seq_data_readonly(Rc::as_ptr(&sequence));
                assert_eq!(std::slice::from_raw_parts(view, 3), &[word; 3]);
            }

            relar_seq_append(Rc::as_ptr(&alias), word, 5);
            assert_eq!(Rc::strong_count(&child), 5);
            assert!(
                sequence
                    .0
                    .with(|storage| storage.sequence().values.spilled())
            );
            assert_eq!(
                Int(relar_seq_len(Rc::as_ptr(&sequence))).as_small(),
                Some(4)
            );
            {
                let view = relar_seq_data_readonly(Rc::as_ptr(&alias));
                assert_eq!(std::slice::from_raw_parts(view, 4), &[word; 4]);
            }

            let popped = Rc::from_raw(relar_seq_pop(Rc::as_ptr(&alias)) as *const Object);
            assert!(Rc::ptr_eq(&child, &popped));
            assert_eq!(Rc::strong_count(&child), 5);
            assert_eq!(
                Int(relar_seq_len(Rc::as_ptr(&sequence))).as_small(),
                Some(3)
            );
            drop(child);
            drop(sequence);
            assert_eq!(Rc::strong_count(&popped), 4);
            drop(alias);
            assert_eq!(Rc::strong_count(&popped), 1);
            drop(popped);
            assert!(weak.upgrade().is_none());
        }
    }

    #[test]
    fn float_words_distinguish_unboxed_bits_from_optional_owners() {
        let bare = WordKind::new(7);
        let nullable = WordKind::new(NULLABLE | 7);
        for bits in [0, 1_u64 << 63, 0x7ff8_0000_0000_0042, u64::MAX] {
            let value = f64::from_bits(bits);
            // SAFETY: bare descriptors accept all f64 bits and read double fields.
            unsafe {
                let owner = bare.acquire(bits as usize);
                assert_eq!(owner.word, bits as usize);
                assert_eq!(
                    bare.read_record_field(std::ptr::from_ref(&value).cast()),
                    bits as usize
                );
            }
            let boxed = Rc::new(value);
            let word = Rc::as_ptr(&boxed) as usize;
            // SAFETY: boxed roots this exact optional-float allocation.
            let owner = unsafe { nullable.acquire(word) };
            assert_eq!(Rc::strong_count(&boxed), 2);
            drop(owner);
            assert_eq!(Rc::strong_count(&boxed), 1);
        }
        // SAFETY: zero denotes absence for the nullable descriptor.
        drop(unsafe { nullable.acquire(0) });
    }
}
