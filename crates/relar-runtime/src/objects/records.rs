//! Fixed native records with a compact header and compiler-described payload.

use std::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};
use std::cell::Cell;

use super::{fail, words::WordKind};

/// Compiler-emitted immutable metadata; every field occupies one eight-byte word.
/// The descriptor and its kind array must outlive every allocation using them.
#[repr(C)]
pub struct RecordDescriptor {
    pub length: usize,
    pub kinds: *const u32,
}

#[repr(C)]
struct Header {
    references: Cell<usize>,
    descriptor: *const RecordDescriptor,
}

const _: () = assert!(size_of::<Header>() == 16 && align_of::<Header>() == 8);

fn layout(length: usize) -> Layout {
    let size = length
        .checked_mul(size_of::<usize>())
        .and_then(|bytes| bytes.checked_add(size_of::<Header>()))
        .unwrap_or_else(|| fail("MemoryError", "record allocation is too large"));
    Layout::from_size_align(size, align_of::<Header>())
        .unwrap_or_else(|_| fail("MemoryError", "record allocation is too large"))
}

/// Creates a record with all payload words zero. Scalar zero needs no cleanup;
/// reference-bearing zeros remain safe until their fields are initialized.
///
/// # Safety
/// The descriptor and its `length` valid kind entries must remain immutable and
/// live until all records using them are released; generated descriptors are
/// static. Generated code must initialize fields before
/// reading them, preserve each declared representation, and prevent heap cycles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_record_new(descriptor: *const RecordDescriptor) -> *mut usize {
    // SAFETY: the caller provides immutable live metadata for this allocation.
    let descriptor_value = unsafe { &*descriptor };
    let allocation_layout = layout(descriptor_value.length);
    // Validate once before constructing an object whose destructor uses the kinds.
    for index in 0..descriptor_value.length {
        // SAFETY: the descriptor promises this many initialized kind entries.
        WordKind::new(unsafe { descriptor_value.kinds.add(index).read() });
    }
    // SAFETY: layout is nonzero and correctly aligned for the header and words.
    let allocation = unsafe { alloc_zeroed(allocation_layout) };
    if allocation.is_null() {
        handle_alloc_error(allocation_layout);
    }
    // SAFETY: the allocation has room for this header and its declared payload.
    unsafe {
        allocation.cast::<Header>().write(Header {
            references: Cell::new(1),
            descriptor,
        });
        allocation.add(size_of::<Header>()).cast()
    }
}

/// # Safety
/// `payload` is null or a live record payload from `relar_record_new`, used only
/// by its execution thread. No reference into its storage may span mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_record_retain(payload: *const usize) {
    if payload.is_null() {
        return;
    }
    // SAFETY: every payload immediately follows its own live Header.
    let header = unsafe { &*payload.cast::<Header>().sub(1) };
    let next = header
        .references
        .get()
        .checked_add(1)
        .unwrap_or_else(|| fail("SystemError", "record reference count overflow"));
    header.references.set(next);
}

/// Acquires the next private iterator result, reusing its cache only when no
/// earlier yielded tuple still aliases it. A shared cache remains unchanged;
/// the returned fresh record starts with cleanup-safe zero fields instead.
///
/// # Safety
/// `payload` is the live, fully rooted cache record on its execution thread.
/// The caller borrows it without adding a temporary owner before this call.
/// Before publishing the returned owner, initialize every field to its declared
/// representation. A reused record may be mutated only through this result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_record_cached_result(payload: *const usize) -> *mut usize {
    // SAFETY: the caller roots this cache and its immutable descriptor.
    let header = unsafe { &*payload.cast::<Header>().sub(1) };
    if header.references.get() == 1 {
        header.references.set(2);
        payload.cast_mut()
    } else {
        // SAFETY: a fresh record uses the cache's static descriptor; it does not
        // expose or mutate any existing tuple's observable field values.
        unsafe { relar_record_new(header.descriptor) }
    }
}

/// Consumes a record owner, releasing initialized fields only on the final drop.
/// This also handles constructors interrupted after any subset of field writes.
///
/// # Safety
/// `payload` is null or transfers one live record owner on its execution thread.
/// The compiler preserves field kinds, acyclic ownership, and descriptor lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_record_release(payload: *const usize) {
    if payload.is_null() {
        return;
    }
    // SAFETY: the payload follows its live header, and this thread owns access.
    let header = unsafe { &*payload.cast::<Header>().sub(1) };
    let count = header.references.get();
    if count == 0 {
        fail("SystemError", "record owner released twice");
    }
    header.references.set(count - 1);
    if count != 1 {
        return;
    }
    // SAFETY: metadata remains immutable and live even while payloads are released.
    let descriptor = unsafe { &*header.descriptor };
    for index in 0..descriptor.length {
        // SAFETY: each nonzero owning word transfers the field's final reference.
        unsafe {
            let kind = WordKind::new(descriptor.kinds.add(index).read());
            kind.release(kind.read_record_field(payload.add(index)));
        }
    }
    // SAFETY: this is the original allocation and exact allocation layout.
    unsafe {
        dealloc(
            payload.cast::<u8>().sub(size_of::<Header>()).cast_mut(),
            layout(descriptor.length),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::exceptions::{
        Kind, relar_error_set, relar_error_take, relar_exception_new,
    };
    use crate::objects::{relar_object_release, relar_seq_new};
    use crate::{Int, List, Str, relar_list_new};
    use num_bigint::BigInt;
    use std::rc::Rc;

    unsafe fn replace(record: *mut usize, field: usize, word: usize, kind: u32) {
        let kind = WordKind::new(kind);
        // SAFETY: tests provide exact descriptors and rooted values. Match the
        // generated acquire/store/release sequence, including self-assignment.
        unsafe {
            let new = kind.acquire(word);
            let field = record.add(field);
            let old = kind.read_record_field(field);
            let word = new.into_word();
            // Match generated pointer stores, so destruction must preserve their
            // provenance instead of treating every field as stored integer bits.
            if matches!(kind.raw(), 3..=6) {
                field.cast::<*const ()>().write(word as *const ());
            } else {
                field.write(word);
            }
            kind.release(old);
        }
    }

    #[test]
    fn compact_records_reclaim_partial_fields_without_disturbing_pending() {
        let kinds = [0, 1, 2, 3, 4, 5, 6, 258, 259, 260, 261, 262];
        let descriptor = RecordDescriptor {
            length: kinds.len(),
            kinds: kinds.as_ptr(),
        };
        let child_kinds = [3];
        let child_descriptor = RecordDescriptor {
            length: 1,
            kinds: child_kinds.as_ptr(),
        };
        assert_eq!(layout(kinds.len()).size(), 16 + kinds.len() * 8);
        let text = Rc::new(Str::from("partial owned field"));
        let weak_text = Rc::downgrade(&text);
        let integer = Rc::new(BigInt::from(1_u128 << 120));
        let weak_integer = Rc::downgrade(&integer);
        // SAFETY: metadata outlives all allocated records, every word has the
        // declared kind, and the parent/child ownership graph is acyclic.
        unsafe {
            let list = Rc::<List>::from_raw(relar_list_new());
            let weak_list = Rc::downgrade(&list);
            let object = Rc::from_raw(relar_seq_new(2));
            let weak_object = Rc::downgrade(&object);
            let child = relar_record_new(&child_descriptor);
            replace(child, 0, Rc::as_ptr(&text) as usize, 3);
            let record = relar_record_new(&descriptor);
            assert!((0..kinds.len()).all(|index| record.add(index).read() == 0));
            replace(record, 1, 1, 1);
            replace(record, 2, Rc::as_ptr(&integer) as usize, 2);
            replace(record, 3, Rc::as_ptr(&text) as usize, 3);
            replace(record, 4, Rc::as_ptr(&list) as usize, 4);
            replace(record, 5, Rc::as_ptr(&object) as usize, 5);
            replace(record, 6, child as usize, 6);
            // Nullable owning words remain absent; uninitialized regular owning
            // slots on a second record also require safe final cleanup.
            let unfinished = relar_record_new(&descriptor);
            relar_record_release(unfinished);
            relar_record_release(child);
            drop((text, integer, list, object));
            let pending = relar_exception_new(Kind::RuntimeError as u32, std::ptr::null());
            relar_error_set(pending);
            relar_record_release(record);
            assert_eq!(relar_error_take(), pending);
            relar_object_release(pending);
            relar_object_release(pending);
            assert!(weak_list.upgrade().is_none());
            assert!(weak_object.upgrade().is_none());
        }
        assert!(weak_text.upgrade().is_none());
        assert!(weak_integer.upgrade().is_none());
    }

    #[test]
    fn record_aliases_and_sole_owner_replacement_preserve_snapshots() {
        let kinds = [3, 258, 257];
        let descriptor = RecordDescriptor {
            length: 3,
            kinds: kinds.as_ptr(),
        };
        let text = Rc::new(Str::from("snapshot"));
        let weak = Rc::downgrade(&text);
        let word = Rc::as_ptr(&text) as usize;
        // SAFETY: metadata outlives the aliases; the replacement function models
        // exactly the compiler's owned field access protocol.
        unsafe {
            let record = relar_record_new(&descriptor);
            replace(record, 0, word, 3);
            replace(record, 1, Int::small(0).unwrap().into_raw(), 258);
            replace(record, 2, 1, 257);
            drop(text);
            relar_record_retain(record);
            replace(record, 0, word, 3);
            let saved = WordKind::new(3).acquire(WordKind::new(3).read_record_field(record));
            let replacement = Rc::new(Str::from("replacement"));
            let replacement_weak = Rc::downgrade(&replacement);
            replace(record, 0, Rc::as_ptr(&replacement) as usize, 3);
            drop(replacement);
            relar_record_release(record);
            assert_eq!(record.add(1).read(), 1);
            assert_eq!(record.add(2).read(), 1);
            relar_record_release(record);
            assert!(replacement_weak.upgrade().is_none());
            assert_eq!((&*(saved.word as *const Str)).as_str(), "snapshot");
            drop(saved);
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn cached_results_reuse_only_exclusive_records_and_release_their_fields() {
        let kinds = [3, 258];
        let descriptor = RecordDescriptor {
            length: kinds.len(),
            kinds: kinds.as_ptr(),
        };
        let original = Rc::new(Str::from("cached field"));
        let original_weak = Rc::downgrade(&original);
        let replacement = Rc::new(Str::from("independent field"));
        let replacement_weak = Rc::downgrade(&replacement);
        // SAFETY: immutable metadata outlives every record. Each returned owner
        // is released once, and only a returned result mutates record fields.
        unsafe {
            let cache = relar_record_new(&descriptor);
            let exclusive = relar_record_cached_result(cache);
            assert_eq!(exclusive, cache);
            replace(exclusive, 0, Rc::as_ptr(&original) as usize, 3);
            replace(exclusive, 1, Int::small(7).unwrap().into_raw(), 258);
            drop(original);

            let shared = relar_record_cached_result(cache);
            assert_ne!(shared, cache);
            assert_eq!(shared.read(), 0);
            assert_eq!(shared.add(1).read(), 0);
            replace(shared, 0, Rc::as_ptr(&replacement) as usize, 3);
            replace(shared, 1, Int::small(9).unwrap().into_raw(), 258);
            drop(replacement);
            assert_eq!(
                (&*cache.cast::<*const Str>().read()).as_str(),
                "cached field"
            );
            assert_eq!(cache.add(1).read(), Int::small(7).unwrap().into_raw());

            relar_record_release(exclusive);
            let reused = relar_record_cached_result(cache);
            assert_eq!(reused, cache);
            relar_record_release(cache);
            assert!(original_weak.upgrade().is_some());
            relar_record_release(reused);
            assert!(original_weak.upgrade().is_none());
            assert!(replacement_weak.upgrade().is_some());
            relar_record_release(shared);
        }
        assert!(replacement_weak.upgrade().is_none());
    }
}
