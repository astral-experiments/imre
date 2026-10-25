//! Ownership for concrete, compiler-described machine words.
//!
//! Stored containers carry one kind per homogeneous buffer, or one static kind
//! per record field. Only temporary owners carry their own kind. Bit 256 marks
//! a nullable value; optional booleans use 0=None, 1=False, and 2=True.
//! Bare float words hold all binary64 bits; nullable floats instead own an Rc<f64>.

use smallvec::SmallVec;
use std::cell::Cell;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use super::{Bytes, Int, List, Object, Str, deferred_diff::Deferred, fail, records, unions};

pub(super) const NULLABLE: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub(super) struct WordKind(u16);

impl WordKind {
    // Private to Words. A deferred list stores a plan instead of element words.
    const DEFERRED: Self = Self(u16::MAX);

    pub(super) fn new(kind: u32) -> Self {
        if kind & !NULLABLE > 10 {
            fail("SystemError", "invalid concrete word representation");
        }
        Self(kind as u16)
    }

    pub(super) fn raw(self) -> u32 {
        u32::from(self.0)
    }

    pub(super) fn base(self) -> u32 {
        self.raw() & !NULLABLE
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
        let nullable = self.raw() & NULLABLE != 0;
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
                6 => records::imre_record_retain(word as *const usize),
                7 if self.raw() & NULLABLE != 0 => Rc::increment_strong_count(word as *const f64),
                7 => {}
                8 => Rc::increment_strong_count(word as *const Bytes),
                9 => unions::imre_union_retain(word),
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
                4 => crate::imre_list_release(word as *const List),
                5 => records::release_object_word(Rc::from_raw(word as *const Object)),
                6 => records::imre_record_release(word as *const usize),
                7 if self.raw() & NULLABLE != 0 => Rc::decrement_strong_count(word as *const f64),
                7 => {}
                8 => Rc::decrement_strong_count(word as *const Bytes),
                9 => unions::imre_union_release(word),
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
    /// Transfers this owner into a fresh record field using the LLVM slot type.
    ///
    /// # Safety
    /// The destination is an uninitialized slot of this exact descriptor.
    pub(super) unsafe fn initialize_record_field(self, field: *mut usize) {
        let kind = self.kind.raw();
        let word = self.into_word();
        // SAFETY: this is the inverse of WordKind::read_record_field.
        unsafe {
            match kind {
                3..=6 | 8 => field.cast::<*const ()>().write(word as *const ()),
                7 => field.cast::<f64>().write(f64::from_bits(word as u64)),
                _ => field.write(word),
            }
        }
    }

    pub(super) fn into_word(self) -> usize {
        ManuallyDrop::new(self).word
    }

    /// # Safety
    /// Transfers an already-owned, correctly typed word into this temporary.
    pub(super) unsafe fn from_owned(word: usize, kind: WordKind) -> Self {
        Self { word, kind }
    }
}

impl Clone for OwnedWord {
    fn clone(&self) -> Self {
        // SAFETY: this owner keeps the initialized word live throughout acquisition.
        unsafe { self.kind.acquire(self.word) }
    }
}

impl Drop for OwnedWord {
    fn drop(&mut self) {
        // SAFETY: this temporary owns exactly the transferred reference.
        unsafe { self.kind.release(self.word) };
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StringOrder {
    Unknown,
    Scanned,
    Ascending,
    Unsorted,
}

/// A homogeneous buffer; the descriptor is shared by all element words.
/// Three words fit inline; spilling preserves their existing owned references.
#[repr(C)]
pub(super) struct Words {
    // Twice the saturated length, plus one when the buffer lives inside the handle.
    header: u32,
    pub(super) kind: WordKind,
    // Fits beside the nine-bit word kind without growing a sequence handle.
    string_order: Cell<StringOrder>,
    pub(super) values: SmallVec<[usize; 3]>,
}

impl Words {
    pub(super) fn new(kind: u32) -> Self {
        Self {
            header: 1,
            kind: WordKind::new(kind),
            string_order: Cell::new(StringOrder::Unknown),
            values: SmallVec::new(),
        }
    }

    /// An inline buffer can never have a saturated length. Reserve its unused
    /// header value for an owned plan, keeping the ordinary sequence layout.
    pub(super) fn deferred(plan: Deferred) -> Self {
        let mut words = Self::new(3);
        words.header = u32::MAX;
        words.kind = WordKind::DEFERRED;
        words.values.push(Box::into_raw(Box::new(plan)) as usize);
        words
    }

    pub(super) fn plan(&self) -> Option<&Deferred> {
        if self.kind == WordKind::DEFERRED {
            debug_assert_eq!(self.header, u32::MAX);
            debug_assert_eq!(self.values.len(), 1);
            // SAFETY: only deferred() installs this sentinel and owned box.
            Some(unsafe { &*(self.values[0] as *const Deferred) })
        } else {
            None
        }
    }

    pub(super) fn resolved(&self) -> &Self {
        self.plan().map_or(self, Deferred::ordered)
    }

    /// A mutating alias gets its own ordinary buffer. Copies share only the
    /// immutable snapshot and retain independent list identities and contents.
    pub(super) fn resolve_mut(&mut self) {
        if let Some(plan) = self.plan() {
            *self = plan.ordered().copy_strings();
        }
    }

    pub(super) fn copy_strings(&self) -> Self {
        debug_assert!(self.plan().is_none());
        self.check_kind(3);
        let mut result = Self::new(3);
        result.values.reserve(self.values.len());
        for &word in &self.values {
            // SAFETY: the source independently owns this exact string.
            result
                .values
                .push(unsafe { self.kind.acquire(word) }.into_word());
        }
        result.update_header();
        result
    }

    /// Borrows each exact string together with its independently owned word.
    pub(super) fn strings(&self) -> impl Iterator<Item = (usize, &Str)> {
        self.check_kind(3);
        self.values.iter().map(|&word| {
            // SAFETY: the exact descriptor proves each element is a rooted string.
            (word, unsafe { &*(word as *const Str) })
        })
    }

    /// Deferred words live in their plan even when their resolved list is
    /// small. Its inline bit must never address the plan pointer as an element.
    pub(super) fn resolved_header(&self) -> u32 {
        self.plan().map_or(self.header, |plan| {
            Self::header_for(plan.ordered().values.len(), false)
        })
    }

    /// An empty inline buffer normally has header 1. Publishing length zero
    /// without its inline flag lets a later append/pop remain observable to sort.
    pub(super) fn sorting_empty(kind: u32) -> Self {
        let mut empty = Self::new(kind);
        empty.header = 0;
        empty
    }

    pub(super) fn is_sorting_empty(&self) -> bool {
        self.header == 0 && !self.values.spilled()
    }

    /// Publish a fully initialized buffer after bulk initialization by a decoder or splitter.
    pub(super) fn into_object(mut self) -> Rc<Object> {
        if self.kind != WordKind::DEFERRED {
            self.update_header();
        }
        Rc::new(Object(super::StorageCell::new(super::Storage::Sequence(
            self,
        ))))
    }

    /// A saturated header defers to the full-width buffer length in the runtime.
    pub(super) fn update_header(&mut self) {
        debug_assert_ne!(self.kind, WordKind::DEFERRED);
        self.header = Self::header_for(self.values.len(), !self.values.spilled());
    }

    fn header_for(length: usize, inline: bool) -> u32 {
        let length = length.min(i32::MAX as usize) as u32;
        (length << 1) | u32::from(inline)
    }

    /// Checks the private compiler/runtime contract before transferring a value.
    pub(super) fn check_kind(&self, kind: u32) {
        if self.kind != WordKind::new(kind) {
            fail("SystemError", "sequence element representation changed");
        }
    }

    /// Every mutation of a published sequence invalidates its ordering proof.
    pub(super) fn forget_order(&mut self) {
        *self.string_order.get_mut() = StringOrder::Unknown;
    }

    /// Checks ascending order once for a resolved string sequence. A mutable
    /// borrow clears the proof before exposing its element words.
    pub(super) fn strings_ascending(&self) -> bool {
        match self.string_order.get() {
            StringOrder::Ascending => true,
            StringOrder::Unsorted => false,
            StringOrder::Unknown | StringOrder::Scanned => {
                let sorted = self.strings().map(|(_, text)| text).is_sorted();
                self.string_order.set(if sorted {
                    StringOrder::Ascending
                } else {
                    StringOrder::Unsorted
                });
                sorted
            }
        }
    }

    /// Tests exact string membership, checking for ascending order only after
    /// a previous search read a substantial part of the list.
    ///
    /// # Safety
    /// The caller roots every word as a non-null, immutable string throughout.
    pub(super) unsafe fn contains_string(&self, needle: &Str) -> bool {
        debug_assert_eq!(self.kind.raw(), 3);
        // SAFETY: the list independently roots every exact string being compared.
        let text = |word: usize| unsafe { &*(word as *const Str) };
        let scan = || {
            self.values.iter().position(|&word| {
                let item = text(word);
                std::ptr::eq(item, needle) || item == needle
            })
        };
        if self.values.len() < 64 {
            return scan().is_some();
        }
        if self.string_order.get() == StringOrder::Unknown {
            let hit = scan();
            if hit.unwrap_or(self.values.len()) >= self.values.len() / 2 {
                self.string_order.set(StringOrder::Scanned);
            }
            return hit.is_some();
        }
        if !self.strings_ascending() {
            return scan().is_some();
        }
        self.values
            .binary_search_by(|&word| text(word).cmp(needle))
            .is_ok()
    }
}

#[repr(C)]
pub struct SequenceLayout {
    header: usize,
    inline_words: usize,
}

/// Generated main calls this once before running user code. Returning the
/// sequence offsets keeps Rust's enum and inline-buffer layout private.
#[unsafe(no_mangle)]
pub extern "C" fn imre_seq_layout() -> SequenceLayout {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    crate::run_start::wait_for_cleanup();

    let mut sample = Words::new(0);
    sample.values.push(0);
    // One non-owning word fits inline and needs no destructor.
    let storage = ManuallyDrop::new(super::Storage::Sequence(sample));
    let super::Storage::Sequence(words) = &*storage else {
        unreachable!()
    };
    let base = std::ptr::from_ref(&*storage).addr();
    SequenceLayout {
        header: std::ptr::from_ref(&words.header).addr() - base,
        inline_words: words.values.as_ptr().addr() - base,
    }
}

impl Drop for Words {
    fn drop(&mut self) {
        if self.kind == WordKind::DEFERRED {
            // SAFETY: the deferred header owns exactly this one boxed plan.
            drop(unsafe { Box::from_raw(self.values[0] as *mut Deferred) });
            return;
        }
        for word in self.values.drain(..).rev() {
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
        assert_eq!(Words::header_for(0, false), 0);
        assert_eq!(
            Words::header_for(i32::MAX as usize - 1, false),
            u32::MAX - 3
        );
        assert_eq!(Words::header_for(i32::MAX as usize, false), u32::MAX - 1);
        assert_eq!(Words::header_for(usize::MAX, false), u32::MAX - 1);
    }

    #[test]
    fn sequence_spill_preserves_aliases_views_and_transferred_owners() {
        use super::super::{
            imre_seq_append, imre_seq_data_readonly, imre_seq_len, imre_seq_new, imre_seq_pop,
        };

        // SAFETY: every handle stays rooted, readonly views end before mutation,
        // and each new or popped owner is adopted exactly once.
        unsafe {
            let child = Rc::from_raw(imre_seq_new(1));
            let weak = Rc::downgrade(&child);
            let word = Rc::as_ptr(&child) as usize;
            let sequence = Rc::from_raw(imre_seq_new(5));
            let alias = Rc::clone(&sequence);
            let layout = imre_seq_layout();
            let cached = Rc::as_ptr(&sequence).byte_add(layout.header).cast::<u32>();
            assert_eq!(cached.read(), 1);
            for _ in 0..3 {
                imre_seq_append(Rc::as_ptr(&sequence), word, 5);
            }
            assert_eq!(cached.read(), 7);
            assert_eq!(Rc::strong_count(&child), 4);
            assert!(
                !sequence
                    .0
                    .with(|storage| storage.sequence().values.spilled())
            );
            {
                let view = imre_seq_data_readonly(Rc::as_ptr(&sequence));
                assert_eq!(
                    view,
                    Rc::as_ptr(&sequence).byte_add(layout.inline_words).cast()
                );
                assert_eq!(std::slice::from_raw_parts(view, 3), &[word; 3]);
            }

            imre_seq_append(Rc::as_ptr(&alias), word, 5);
            assert_eq!(cached.read(), 8);
            assert_eq!(Rc::strong_count(&child), 5);
            assert!(
                sequence
                    .0
                    .with(|storage| storage.sequence().values.spilled())
            );
            assert_eq!(Int(imre_seq_len(Rc::as_ptr(&sequence))).as_small(), Some(4));
            {
                let view = imre_seq_data_readonly(Rc::as_ptr(&alias));
                assert_eq!(std::slice::from_raw_parts(view, 4), &[word; 4]);
            }

            let popped = Rc::from_raw(imre_seq_pop(Rc::as_ptr(&alias)) as *const Object);
            assert_eq!(cached.read(), 6); // Popping preserves spilled storage.
            assert!(Rc::ptr_eq(&child, &popped));
            assert_eq!(Rc::strong_count(&child), 5);
            assert_eq!(Int(imre_seq_len(Rc::as_ptr(&sequence))).as_small(), Some(3));
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
    fn deferred_header_never_exposes_the_plan_as_an_inline_string() {
        use super::super::deferred_diff::{
            imre_ndiff_deferred, imre_seq_resolve_header, imre_str_list_copy,
        };
        use super::super::{imre_seq_append, imre_seq_data_readonly};

        let held = Rc::new(Str::new("rooted"));
        let weak = Rc::downgrade(&held);
        let input = Words::new(3).into_object();
        // SAFETY: all handles remain rooted through their last access. The
        // buffer pointer is read before either the list or a copy is changed.
        unsafe {
            imre_seq_append(Rc::as_ptr(&input), Rc::as_ptr(&held) as usize, 3);
            let diff = Rc::from_raw(imre_ndiff_deferred(Rc::as_ptr(&input), Rc::as_ptr(&input)));
            let copy = Rc::from_raw(imre_str_list_copy(Rc::as_ptr(&diff)));
            drop(held);
            drop(input);
            let layout = imre_seq_layout();
            let handle = Rc::as_ptr(&diff);
            assert_eq!(
                handle.byte_add(layout.header).cast::<u32>().read(),
                u32::MAX
            );
            assert_eq!(imre_seq_resolve_header(handle), 2);
            let data = imre_seq_data_readonly(handle);
            assert_ne!(data, handle.byte_add(layout.inline_words).cast());
            assert_eq!((&*(data.read() as *const Str)).as_str(), "  rooted");
            drop(diff);
            assert!(weak.upgrade().is_some());
            drop(copy);
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
