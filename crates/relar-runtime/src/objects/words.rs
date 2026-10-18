//! Ownership for concrete, compiler-described machine words.
//!
//! Stored containers carry one kind per homogeneous buffer, or one static kind
//! per record field. Only temporary owners carry their own kind. Bit 256 marks
//! a nullable value; optional booleans use 0=None, 1=False, and 2=True.

use std::mem::ManuallyDrop;
use std::rc::Rc;

use super::{Int, List, Object, Str, fail, records};

pub(super) const NULLABLE: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WordKind(u32);

impl WordKind {
    pub(super) fn new(kind: u32) -> Self {
        if kind & !NULLABLE > 6 {
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
        if matches!(self.0, 3..=6) {
            // SAFETY: this exact descriptor denotes a pointer-typed field.
            unsafe { field.cast::<*const ()>().read() as usize }
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
            2..=6 => word != 0 || nullable,
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
    /// `word` is zero or a rooted handle of this kind. Zero is also the safe
    /// uninitialized state of reference-bearing record fields.
    pub(super) unsafe fn retain(self, word: usize) {
        if word == 0 {
            return;
        }
        // SAFETY: the caller supplies each handle's exact allocation kind.
        unsafe {
            match self.base() {
                0 | 1 => {}
                2 => {
                    if word & 1 == 0 {
                        Rc::increment_strong_count(word as *const num_bigint::BigInt);
                    }
                }
                3 => Rc::increment_strong_count(word as *const Str),
                4 => Rc::increment_strong_count(word as *const List),
                5 => Rc::increment_strong_count(word as *const Object),
                6 => records::relar_record_retain(word as *const usize),
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
                0 | 1 => {}
                2 => drop(Int(word)),
                3 => Rc::decrement_strong_count(word as *const Str),
                4 => Rc::decrement_strong_count(word as *const List),
                5 => Rc::decrement_strong_count(word as *const Object),
                6 => records::relar_record_release(word as *const usize),
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
pub(super) struct Words {
    pub(super) kind: WordKind,
    pub(super) values: Vec<usize>,
}

impl Words {
    pub(super) fn new(kind: u32) -> Self {
        Self {
            kind: WordKind::new(kind),
            values: Vec::new(),
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
