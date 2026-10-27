//! Fixed native records with a compact header and compiler-described payload.

use crate::StrOwner;
use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::cell::{Cell, RefCell, UnsafeCell};
use std::rc::Rc;

use super::{Object, fail, words::WordKind};

pub(super) mod strings;
pub(super) mod tuples;
pub(super) mod weak;
mod wrappers;

// Keep the first queue block available even when destruction cannot allocate.
type ReleaseQueue = RefCell<smallvec::SmallVec<[Release; 4]>>;

enum Release {
    Fields(*const usize, usize),
    Record(*const usize),
    Object(crate::ObjectOwner),
    Sequence(WordKind, Vec<usize>),
    Table(Box<dyn super::mappings::ReleaseEntries>, usize),
}

thread_local! {
    // The outer release owns the queue on its stack. This slot has no TLS
    // destructor, so records can also be released during thread teardown.
    static PENDING_RELEASES: Cell<*const ReleaseQueue> = const { Cell::new(std::ptr::null()) };
}

/// Compiler-emitted immutable metadata; every field occupies one eight-byte word.
/// The descriptor and its kind array must outlive every allocation using them.
#[repr(C)]
pub struct RecordDescriptor {
    pub length: usize,
    pub kinds: *const u32,
    /// Null for internal records without user-visible value operations.
    pub semantics: *const RecordSemantics,
}

/// Kinds are zero for a class, one for a tuple, two for a ModuleSpec, and three
/// for a Python function with a wrapper link. Flags select generated equality and representation.
/// All strings are immutable UTF-8 byte slices.
#[repr(C)]
pub struct RecordSemantics {
    pub kind: u32,
    pub flags: u32,
    pub name: *const u8,
    pub name_len: usize,
    pub qualified_name: *const u8,
    pub qualified_name_len: usize,
    pub fields: *const RecordField,
}

impl RecordDescriptor {
    /// Class and tuple records share storage but have distinct Python protocols.
    pub(super) fn class_semantics(&self) -> Option<&RecordSemantics> {
        // SAFETY: generated semantics are immutable and outlive the descriptor.
        unsafe { self.semantics.as_ref() }.filter(|s| s.kind == 0)
    }

    /// Codegen units may own different descriptors for one Python class.
    pub(super) fn same_class(&self, other: &Self) -> bool {
        let Some((left, right)) = self.class_semantics().zip(other.class_semantics()) else {
            return false;
        };
        if std::ptr::eq(self, other) {
            return true;
        }
        if left.qualified_name_len == 0 || left.qualified_name_len != right.qualified_name_len {
            return false;
        }
        // SAFETY: generated class descriptors root nonempty immutable UTF-8 identifiers.
        unsafe {
            std::slice::from_raw_parts(left.qualified_name, left.qualified_name_len)
                == std::slice::from_raw_parts(right.qualified_name, right.qualified_name_len)
        }
    }

    // Bit 3 is local to each semantic kind: classes can use inline value storage;
    // stat tuples instead own extra numeric fields after their visible elements.
    pub(super) fn is_value_struct(&self) -> bool {
        // SAFETY: the compiler emits immutable metadata for the lifetime of the program.
        unsafe {
            self.semantics
                .as_ref()
                .is_some_and(|s| s.kind == 0 && s.flags & 8 != 0)
        }
    }

    /// Only records with a pure TOML hash reserve an extra untraced cache word.
    pub(super) fn has_toml_hash(&self) -> bool {
        // SAFETY: every descriptor used by the runtime owns static metadata or null.
        unsafe { self.semantics.as_ref().is_some_and(|s| s.flags & 4 != 0) }
    }

    pub(super) fn has_time_zone(&self) -> bool {
        // SAFETY: generated record semantics outlive the descriptor.
        unsafe { self.semantics.as_ref() }.is_some_and(|s| s.kind == 1 && s.flags & 16 != 0)
    }

    fn allocated_words(&self) -> usize {
        // These struct-sequence attributes contain only raw numbers. They are
        // neither Python tuple elements nor owners to be visited during release.
        // SAFETY: a non-null semantics pointer refers to the descriptor's permanent metadata.
        let stat_words = if unsafe { self.semantics.as_ref() }
            .is_some_and(|s| s.kind == 1 && s.flags & 8 != 0)
        {
            super::os::stat_result::EXTRA_WORDS
        } else {
            0
        };
        self.length
            .checked_add(
                usize::from(self.has_toml_hash())
                    + stat_words
                    + 2 * usize::from(self.has_time_zone()),
            )
            .unwrap_or_else(|| fail("MemoryError", "record allocation is too large"))
    }
}

#[repr(C)]
pub struct RecordField {
    pub name: *const u8,
    pub name_len: usize,
    /// Bit zero includes the field in equality; bit one includes it in generated
    /// repr; bit two marks a string containing the complete record repr.
    pub flags: u32,
}

#[repr(C)]
struct Header {
    references: Cell<usize>,
    /// Descriptor (untagged), shared block (bit 0), weak anchor (bit 1),
    /// or owned tuple layout (bit 2). An anchor retains the original tagged layout.
    descriptor: Cell<*const RecordDescriptor>,
}

const _: () = assert!(size_of::<Header>() == 16 && align_of::<Header>() == 8);

/// # Safety
/// `payload` is a rooted native record or a live read-only tuple probe.
pub(super) unsafe fn descriptor<'a>(payload: *const usize) -> &'a RecordDescriptor {
    // SAFETY: the immutable descriptor belongs to the live allocation's header.
    unsafe { (*payload.cast::<Header>().sub(1)).descriptor() }
}

/// Copy the inner cached callable's public policy, as update_wrapper does.
/// Capacity used for lookup and eviction belongs to the new wrapper's table.
///
/// # Safety
/// The destination is a newly allocated four-word cache callable; source is
/// null or a rooted native callable (two ordinary words or four cache words).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_callable_copy_cache_policy(
    destination: *mut usize,
    source: *const usize,
    capacity: i64,
    typed: u32,
) {
    // SAFETY: all four fields exist on a cache callable. Its two public
    // policy fields are plain words and do not own references.
    unsafe {
        debug_assert_eq!(descriptor(destination).length, 4);
        if !source.is_null() && descriptor(source).length == 4 {
            destination.add(2).write(*source.add(2));
            destination.add(3).write(*source.add(3));
        } else {
            destination.add(2).write(capacity as usize);
            destination.add(3).write(typed as usize);
        }
    }
}

/// Read a cached callable's public maxsize or typed flag as a dictionary value.
///
/// # Safety
/// The owner roots a four-word cache callable. Its last two words contain
/// an immutable signed capacity and a boolean, in that order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_callable_cache_parameter(
    wrapper: *const usize,
    typed: u32,
) -> *const Object {
    use super::json::readonly::document::Value;
    // SAFETY: generated code verified the wrapper layout.
    let value = unsafe {
        if typed != 0 {
            Value::Bool(*wrapper.add(3) != 0)
        } else {
            let size = *wrapper.add(2) as i64;
            if size < 0 {
                Value::Null
            } else {
                Value::Integer(crate::Int::from_usize(size as usize))
            }
        }
    };
    crate::ObjectOwner::into_raw(value.object())
}

/// Checks the declared identity of a record, including across codegen units.
///
/// # Safety
/// Both arguments refer to live, compiler-described class layouts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_record_has_type(
    payload: *const usize,
    expected: *const RecordDescriptor,
) -> u32 {
    // SAFETY: the caller supplies a rooted class and an immutable class descriptor.
    unsafe { u32::from(descriptor(payload).same_class(&*expected)) }
}

/// Initializes caller-owned scratch for a tuple used only by a read-only lookup.
///
/// # Safety
/// `scratch` contains two header words followed by `descriptor.length` writable
/// words. The caller initializes and roots every field before probing. The
/// payload may be hashed and compared, and must never be retained or released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_tuple_probe(
    scratch: *mut usize,
    descriptor: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: the buffer is word-aligned and has space for this header and tuple.
    unsafe {
        let header = scratch.cast::<Header>();
        header.write(Header {
            references: Cell::new(0),
            descriptor: Cell::new(descriptor),
        });
        header.add(1).cast()
    }
}

impl Header {
    fn tuple_layout(&self) -> Option<*const tuples::TupleLayout> {
        let layout = self.layout_pointer();
        (layout.addr() & tuples::LAYOUT_TAG != 0)
            .then(|| layout.map_addr(|addr| addr & !tuples::LAYOUT_TAG).cast())
    }

    fn block(&self) -> Option<*const Block> {
        let layout = self.layout_pointer();
        if layout.addr() & 1 == 0 {
            return None;
        }
        // Each tagged pointer owns a live block. Removing its tag
        // recovers the original aligned pointer and its allocation provenance.
        Some(layout.map_addr(|addr| addr - 1).cast::<Block>())
    }

    fn descriptor(&self) -> &RecordDescriptor {
        // SAFETY: a tagged header owns its Rc until its last record owner drops.
        let layout = self.layout_pointer();
        let descriptor = if layout.addr() & 1 == 0 {
            layout.map_addr(|addr| addr & !tuples::LAYOUT_TAG)
        } else {
            // SAFETY: the untagged block is rooted for this record's lifetime.
            unsafe { (*layout.map_addr(|addr| addr - 1).cast::<Block>()).descriptor }
        };
        // SAFETY: both forms carry the original immutable compiler metadata.
        unsafe { &*descriptor }
    }
}

fn layout(length: usize) -> Layout {
    let size = length
        .checked_mul(size_of::<usize>())
        .and_then(|bytes| bytes.checked_add(size_of::<Header>()))
        .unwrap_or_else(|| fail("MemoryError", "record allocation is too large"));
    Layout::from_size_align(size, align_of::<Header>())
        .unwrap_or_else(|_| fail("MemoryError", "record allocation is too large"))
}

// Each surviving record pins at most 512 bytes of slots and one small Rc header.
// Other records in the block release their field owners at their own last drop.
const BLOCK_WORDS: usize = 64;

#[repr(C)]
struct Block {
    descriptor: *const RecordDescriptor,
    slots: UnsafeCell<[usize; BLOCK_WORDS]>,
}

/// Hands out stable, independently owned records while decoding a typed array.
/// Each full block can outlive the decoder and every other block.
pub(super) struct RecordBatch<'a> {
    descriptor: &'a RecordDescriptor,
    words: usize,
    block: Option<Rc<Block>>,
    next: usize,
}

impl<'a> RecordBatch<'a> {
    /// Wide records with fewer than two slots per block use individual allocations.
    pub(super) fn new(descriptor: &'a RecordDescriptor) -> Option<Self> {
        let words = layout(descriptor.allocated_words()).size() / size_of::<usize>();
        (words <= BLOCK_WORDS / 2).then_some(Self {
            descriptor,
            words,
            block: None,
            next: BLOCK_WORDS,
        })
    }

    pub(super) fn accepts(&self, descriptor: *const RecordDescriptor) -> bool {
        std::ptr::eq(self.descriptor, descriptor)
    }

    /// # Safety
    /// The descriptor must describe valid kinds and outlive all returned record owners.
    pub(super) unsafe fn allocate(&mut self) -> *mut usize {
        if self.next + self.words > BLOCK_WORDS {
            self.block = Some(Rc::new(Block {
                descriptor: self.descriptor,
                slots: UnsafeCell::new([0; BLOCK_WORDS]),
            }));
            self.next = 0;
        }
        let block = Rc::into_raw(Rc::clone(self.block.as_ref().unwrap()));
        // SAFETY: the block's distinct, never-reused slots are aligned for headers
        // and payloads. This record owns the Rc stored in its tagged header.
        unsafe {
            let slot = (*block)
                .slots
                .get()
                .cast::<usize>()
                .add(self.next)
                .cast::<Header>();
            self.next += self.words;
            slot.write(Header {
                references: Cell::new(1),
                descriptor: Cell::new(block.cast::<RecordDescriptor>().map_addr(|addr| addr | 1)),
            });
            slot.add(1).cast()
        }
    }
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
pub unsafe extern "C" fn imre_record_new(descriptor: *const RecordDescriptor) -> *mut usize {
    // SAFETY: the caller supplies a live descriptor and its immutable field kinds.
    let record = unsafe { allocate_record(descriptor) };
    if record.is_null() {
        // SAFETY: the descriptor remains live after a failed allocation.
        handle_alloc_error(layout(unsafe { (*descriptor).allocated_words() }));
    }
    record
}

/// Returns null with MemoryError pending when the record allocation fails.
///
/// # Safety
/// The descriptor satisfies the same lifetime and field requirements as `imre_record_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_record_try_new(descriptor: *const RecordDescriptor) -> *mut usize {
    // SAFETY: the caller supplies a live descriptor and its immutable field kinds.
    let record = unsafe { allocate_record(descriptor) };
    if record.is_null() {
        super::exceptions::record_allocation_failed();
    }
    record
}

/// Returns an owned reference to a module's unique class constructor.
///
/// # Safety
/// The executing module exclusively owns the zero-initialized slot. The layout
/// describes an unbound callable (adapter, null context); all three addresses
/// remain live until the entry releases its globals.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_record_class_factory(
    slot: *mut *mut usize,
    layout: *const RecordDescriptor,
    adapter: *const (),
) -> *mut usize {
    // SAFETY: the caller owns the slot and the descriptor's zeroed callable
    // payload. The slot adopts the initial owner; the caller receives another.
    unsafe {
        let mut class = *slot;
        if class.is_null() {
            class = imre_record_try_new(layout);
            if class.is_null() {
                return class;
            }
            class.write(adapter as usize);
            *slot = class;
        }
        imre_record_retain(class);
        class
    }
}

/// Shallow-copies a plain class instance without evaluating Python code.
///
/// # Safety
/// The caller roots a live record throughout the call. Its immutable descriptor
/// describes all public fields and any cached-property storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_record_shallow_copy(source: *const usize) -> *mut usize {
    // SAFETY: the source descriptor outlives these allocations. The allocation
    // helper reports MemoryError before we acquire any field references.
    unsafe {
        let layout = descriptor(source);
        let copy = imre_record_try_new(layout);
        if copy.is_null() {
            return copy;
        }
        for i in 0..layout.length {
            let kind = WordKind::new(layout.kinds.add(i).read());
            // A cold cached property may carry a zero, uninitialized owner.
            kind.retain(kind.read_record_field(source.add(i)));
        }
        // Byte-copy the original scalar and pointer representations, including
        // their provenance. Private hash-cache storage starts cold in the copy.
        std::ptr::copy_nonoverlapping(source, copy, layout.length);
        copy
    }
}

/// Returns a record with zeroed fields, or null on allocation failure.
///
/// # Safety
/// The immutable descriptor and all field kinds must outlive the returned record.
pub(super) unsafe fn allocate_record(descriptor: *const RecordDescriptor) -> *mut usize {
    // SAFETY: the caller provides immutable live metadata for this allocation.
    let descriptor_value = unsafe { &*descriptor };
    let allocation_layout = layout(descriptor_value.allocated_words());
    // Validate once before constructing an object whose destructor uses the kinds.
    for index in 0..descriptor_value.length {
        // SAFETY: the descriptor promises this many initialized kind entries.
        WordKind::new(unsafe { descriptor_value.kinds.add(index).read() });
    }
    // SAFETY: layout is nonzero and correctly aligned for the header and words.
    let allocation = unsafe { alloc(allocation_layout) };
    if allocation.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the allocation has room for this header and its declared payload.
    unsafe {
        allocation.cast::<Header>().write(Header {
            references: Cell::new(1),
            descriptor: Cell::new(descriptor),
        });
        // Only the payload needs zeroing for partial-field cleanup. The ordinary
        // allocation can reuse a freed small record without clearing its header.
        let payload = allocation.add(size_of::<Header>()).cast::<usize>();
        payload.write_bytes(0, descriptor_value.allocated_words());
        payload
    }
}

/// # Safety
/// `payload` is null or a live native record payload, used only
/// by its execution thread. No reference into its storage may span mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_record_retain(payload: *const usize) {
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
pub unsafe extern "C" fn imre_record_cached_result(payload: *const usize) -> *mut usize {
    // SAFETY: the caller borrows its rooted cache without a temporary owner.
    unsafe { cached_result(payload, imre_record_new) }
}

/// Returns a retained cache or a fresh record, raising MemoryError on allocation failure.
///
/// # Safety
/// The cache and returned record have the same ownership and initialization
/// requirements as `imre_record_cached_result`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_record_try_cached_result(payload: *const usize) -> *mut usize {
    // SAFETY: the caller borrows its rooted cache without a temporary owner.
    unsafe { cached_result(payload, imre_record_try_new) }
}

unsafe fn cached_result(
    payload: *const usize,
    allocate: unsafe extern "C" fn(*const RecordDescriptor) -> *mut usize,
) -> *mut usize {
    // SAFETY: the caller roots this cache and its immutable descriptor.
    let header = unsafe { &*payload.cast::<Header>().sub(1) };
    if header.references.get() == 1 && !header.has_weak_anchor() {
        header.references.set(2);
        payload.cast_mut()
    } else {
        // SAFETY: a fresh record uses the cache's static descriptor; it does not
        // expose or mutate any existing tuple's observable field values.
        unsafe {
            let next = allocate(header.descriptor());
            if let Some(layout) = header.tuple_layout()
                && !next.is_null()
            {
                Rc::increment_strong_count(layout);
                // The new record shares its immutable layout, not the cache's identity.
                (*next.cast::<Header>().sub(1))
                    .descriptor
                    .set(header.layout_pointer());
            }
            next
        }
    }
}

/// Consumes a record owner, releasing initialized fields only on the final drop.
/// This also handles constructors interrupted after any subset of field writes.
///
/// # Safety
/// `payload` is null or transfers one live record owner on its execution thread.
/// The compiler preserves field kinds, acyclic ownership, and descriptor lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_record_release(payload: *const usize) {
    if !payload.is_null() {
        release(Release::Record(payload));
    }
}

/// A container can start an acyclic release traversal without a record root.
pub(super) fn release_object_word(object: crate::ObjectOwner) {
    release(Release::Object(object));
}

/// Transfers the sequence's existing heap buffer and all of its owned words.
pub(super) fn release_sequence_words(kind: WordKind, values: Vec<usize>) {
    release(Release::Sequence(kind, values));
}

pub(super) fn release_table(table: Box<dyn super::mappings::ReleaseEntries>) {
    release(Release::Table(table, 0));
}

fn release(owner: Release) {
    PENDING_RELEASES.with(|slot| {
        // SAFETY: a non-null pointer belongs to the enclosing release on this
        // thread. The C ABI cannot unwind, and the outer call clears the slot
        // before returning. No queue borrow spans a field release.
        if let Some(pending) = unsafe { slot.get().as_ref() } {
            pending.borrow_mut().push(owner);
            return;
        }
        if let Release::Object(object) = &owner
            && crate::ObjectOwner::strong_count(object) > 1
        {
            return;
        }
        let pending = ReleaseQueue::default();
        slot.set(&pending);
        let mut current = owner;
        loop {
            let siblings = pending.borrow().len();
            match current {
                // SAFETY: each entry transfers one live owner of this record.
                Release::Record(record) => unsafe { release_record_owner(record, &pending) },
                // SAFETY: an unfinished frame owns the standalone allocation and remaining fields.
                Release::Fields(record, next) => unsafe {
                    release_standalone_fields(record, next, &pending);
                },
                Release::Object(object) => super::release_object_owner(object),
                Release::Table(mut table, mut next) => {
                    if table.release_next(&mut next) {
                        pending.borrow_mut().push(Release::Table(table, next));
                    }
                }
                Release::Sequence(kind, mut values) => {
                    if let Some(word) = values.pop() {
                        // SAFETY: the sequence transferred this independently owned word.
                        unsafe { kind.release(word) };
                    }
                    if !values.is_empty() {
                        // Visit the child before resuming the remaining words.
                        pending.borrow_mut().push(Release::Sequence(kind, values));
                    }
                }
            }
            let mut pending = pending.borrow_mut();
            // Children were enqueued in field or container order. Finish this
            // subtree in that order before resuming the outstanding siblings.
            pending[siblings..].reverse();
            let Some(next) = pending.pop() else { break };
            current = next;
        }
        slot.set(std::ptr::null());
    });
}

/// # Safety
/// The caller transfers one live record owner on its execution thread.
unsafe fn release_record_owner(payload: *const usize, pending: &ReleaseQueue) {
    // SAFETY: the payload follows its live header, and this thread owns access.
    let header = unsafe { &*payload.cast::<Header>().sub(1) };
    let count = header.references.get();
    if count == 0 {
        fail("SystemError", "record owner released twice");
    }
    header.references.set(count - 1);
    if count != 1 {
        if count == 2 && wrappers::is_function(header.descriptor()) {
            // SAFETY: the remaining owner roots this marked function. Every
            // traversed edge independently owns its target on this thread.
            unsafe { wrappers::release_unowned_cycle(payload) };
        }
        return;
    }
    header.drop_weak_anchor();
    // SAFETY: metadata remains immutable and live even while payloads are released.
    let descriptor = header.descriptor();
    if descriptor.has_time_zone() {
        // SAFETY: the last owner releases both optional attributes of this time tuple.
        unsafe { super::time::release_extras(payload) };
    }
    if let Some(block) = header.block() {
        // SAFETY: no live owner can reach this record now. Other records in
        // the block, and an active cursor, keep their own storage alive.
        unsafe {
            release_fields::<true>(payload, descriptor, 0..descriptor.length);
            Rc::decrement_strong_count(block);
        }
    } else {
        // SAFETY: the final owner transfers this standalone allocation and all its fields.
        unsafe { release_standalone_fields(payload, 0, pending) };
    }
}

unsafe fn release_standalone_fields(
    payload: *const usize,
    mut next: usize,
    pending: &ReleaseQueue,
) {
    // SAFETY: this record owns its descriptor until its final field is released.
    // Keep the borrow local so it ends before the last tuple layout owner drops.
    let descriptor = unsafe { descriptor(payload) };
    let (siblings, room) = {
        let queue = pending.borrow();
        (queue.len(), queue.capacity() - queue.len())
    };
    while next < descriptor.length {
        // Leave room for the continuation when this batch cannot finish the record.
        let batch = if descriptor.length - next <= room {
            room
        } else {
            room.saturating_sub(1)
        };
        let end = (next + batch.clamp(1, 32)).min(descriptor.length);
        // SAFETY: this frame owns the untouched fields starting at next.
        unsafe { release_fields::<false>(payload, descriptor, next..end) };
        next = end;
        if next < descriptor.length && pending.borrow().len() > siblings {
            pending.borrow_mut().push(Release::Fields(payload, next));
            return;
        }
    }
    // SAFETY: every field has transferred its owner and the constructor used this layout.
    unsafe {
        let size = size_of::<Header>() + descriptor.allocated_words() * size_of::<usize>();
        let layout = Layout::from_size_align_unchecked(size, align_of::<Header>());
        let header = payload.cast::<Header>().sub(1);
        // A suspended field-release frame still needs its tuple descriptor.
        // Transfer its owner only after the last field has been visited.
        let tuple_layout = (*header).tuple_layout();
        dealloc(header.cast_mut().cast(), layout);
        if let Some(tuple_layout) = tuple_layout {
            Rc::decrement_strong_count(tuple_layout);
        }
    }
}

/// The caller transfers the final record's fields with their original descriptor.
unsafe fn release_fields<const PACKED: bool>(
    payload: *const usize,
    descriptor: &RecordDescriptor,
    fields: std::ops::Range<usize>,
) {
    // Dataclass fields release in definition order. Tuples release their last
    // element first. Both kinds use the same native record layout.
    // SAFETY: semantic metadata, when present, lives as long as the descriptor.
    let is_tuple = unsafe { descriptor.semantics.as_ref() }.is_some_and(|value| value.kind == 1);
    for field in fields {
        let index = if is_tuple {
            descriptor.length - 1 - field
        } else {
            field
        };
        // SAFETY: each nonzero owning word transfers the field's final reference.
        unsafe {
            let kind = WordKind::new(descriptor.kinds.add(index).read());
            let word = kind.read_record_field(payload.add(index));
            if PACKED && kind.raw() & !super::words::NULLABLE == 3 && word != 0 {
                strings::release(StrOwner::from_raw(word as *const crate::Str));
            } else {
                kind.release(word);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::exceptions::{Kind, imre_error_set, imre_error_take, imre_exception_new};
    use crate::objects::{imre_object_release, imre_seq_new};
    use crate::shared::Shared;
    use crate::{Int, Str, imre_list_new};
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
    fn class_inline_storage_and_stat_tail_use_their_own_semantic_kind() {
        fn layout(kind: u32) -> (bool, usize) {
            let semantics = RecordSemantics {
                kind,
                flags: 8,
                name: std::ptr::null(),
                name_len: 0,
                qualified_name: std::ptr::null(),
                qualified_name_len: 0,
                fields: std::ptr::null(),
            };
            let descriptor = RecordDescriptor {
                length: 10,
                kinds: std::ptr::null(),
                semantics: &semantics,
            };
            (descriptor.is_value_struct(), descriptor.allocated_words())
        }
        assert_eq!(layout(0), (true, 10));
        assert_eq!(
            layout(1),
            (false, 10 + super::super::os::stat_result::EXTRA_WORDS)
        );
    }

    #[test]
    fn deep_object_only_lists_release_every_owner_on_a_small_stack() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let mut root = crate::objects::Words::new(5).into_object();
                let leaf = crate::ObjectOwner::downgrade(&root);
                for _ in 0..20_000 {
                    let mut parent = crate::objects::Words::new(5);
                    parent
                        .values
                        .push(crate::ObjectOwner::into_raw(root) as usize);
                    root = parent.into_object();
                }
                drop(root);
                assert!(leaf.upgrade().is_none());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn deep_record_chains_release_iteratively_and_preserve_shared_children() {
        let record_kind = [6];
        let descriptor = RecordDescriptor {
            length: 1,
            kinds: record_kind.as_ptr(),
            semantics: std::ptr::null(),
        };
        let string_kind = [3];
        let leaf_descriptor = RecordDescriptor {
            length: 1,
            kinds: string_kind.as_ptr(),
            semantics: std::ptr::null(),
        };
        let text = StrOwner::new(Str::from("shared leaf"));
        // SAFETY: each new field receives one live owner of its declared kind.
        // Both descriptors outlive every release, including the saved leaf.
        unsafe {
            let leaf = imre_record_new(&leaf_descriptor);
            leaf.write(StrOwner::into_raw(text.clone()) as usize);
            imre_record_retain(leaf);
            let mut root = leaf;
            for _ in 0..50_000 {
                let parent = imre_record_new(&descriptor);
                parent.write(root as usize);
                root = parent;
            }
            imre_record_release(root);
            assert_eq!(StrOwner::strong_count(&text), 2);
            imre_record_release(leaf);
            assert_eq!(StrOwner::strong_count(&text), 1);
        }
    }

    #[test]
    fn records_can_be_released_from_thread_local_destructors() {
        struct ReleaseOnExit;
        impl Drop for ReleaseOnExit {
            fn drop(&mut self) {
                let kinds = [0];
                let descriptor = RecordDescriptor {
                    length: kinds.len(),
                    kinds: kinds.as_ptr(),
                    semantics: std::ptr::null(),
                };
                // SAFETY: the descriptor outlives the sole owner.
                unsafe { imre_record_release(imre_record_new(&descriptor)) };
            }
        }
        thread_local! { static ON_EXIT: ReleaseOnExit = const { ReleaseOnExit }; }
        std::thread::spawn(|| {
            ON_EXIT.with(|_| {});
            drop(ReleaseOnExit);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn compact_records_reclaim_partial_fields_without_disturbing_pending() {
        let kinds = [0, 1, 2, 3, 4, 5, 6, 258, 259, 260, 261, 262];
        let descriptor = RecordDescriptor {
            length: kinds.len(),
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        let child_kinds = [3];
        let child_descriptor = RecordDescriptor {
            length: 1,
            kinds: child_kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        assert_eq!(layout(kinds.len()).size(), 16 + kinds.len() * 8);
        let text = StrOwner::new(Str::from("partial owned field"));
        let weak_text = StrOwner::downgrade(&text);
        let integer = Shared::new(BigInt::from(1_u128 << 120));
        let weak_integer = Shared::downgrade(&integer);
        // SAFETY: metadata outlives all allocated records, every word has the
        // declared kind, and the parent/child ownership graph is acyclic.
        unsafe {
            let list = crate::ListOwner::from_raw(imre_list_new());
            let weak_list = crate::ListOwner::downgrade(&list);
            let object = crate::ObjectOwner::from_raw(imre_seq_new(2));
            let weak_object = crate::ObjectOwner::downgrade(&object);
            let child = imre_record_new(&child_descriptor);
            replace(child, 0, StrOwner::as_ptr(&text) as usize, 3);
            let record = imre_record_new(&descriptor);
            assert!((0..kinds.len()).all(|index| record.add(index).read() == 0));
            replace(record, 1, 1, 1);
            replace(record, 2, Shared::as_ptr(&integer) as usize, 2);
            replace(record, 3, StrOwner::as_ptr(&text) as usize, 3);
            replace(record, 4, crate::ListOwner::as_ptr(&list) as usize, 4);
            replace(record, 5, crate::ObjectOwner::as_ptr(&object) as usize, 5);
            replace(record, 6, child as usize, 6);
            // Nullable owning words remain absent; uninitialized regular owning
            // slots on a second record also require safe final cleanup.
            let unfinished = imre_record_new(&descriptor);
            imre_record_release(unfinished);
            imre_record_release(child);
            drop((text, integer, list, object));
            let pending = imre_exception_new(Kind::RuntimeError as u32, std::ptr::null());
            imre_error_set(pending);
            imre_record_release(record);
            assert_eq!(imre_error_take(), pending);
            imre_object_release(pending);
            imre_object_release(pending);
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
            semantics: std::ptr::null(),
        };
        let text = StrOwner::new(Str::from("snapshot"));
        let weak = StrOwner::downgrade(&text);
        let word = StrOwner::as_ptr(&text) as usize;
        // SAFETY: metadata outlives the aliases; the replacement function models
        // exactly the compiler's owned field access protocol.
        unsafe {
            let record = imre_record_new(&descriptor);
            replace(record, 0, word, 3);
            replace(record, 1, Int::small(0).unwrap().into_raw(), 258);
            replace(record, 2, 1, 257);
            drop(text);
            imre_record_retain(record);
            replace(record, 0, word, 3);
            let saved = WordKind::new(3).acquire(WordKind::new(3).read_record_field(record));
            let replacement = StrOwner::new(Str::from("replacement"));
            let replacement_weak = StrOwner::downgrade(&replacement);
            replace(record, 0, StrOwner::as_ptr(&replacement) as usize, 3);
            drop(replacement);
            imre_record_release(record);
            assert_eq!(record.add(1).read(), 1);
            assert_eq!(record.add(2).read(), 1);
            imre_record_release(record);
            assert!(replacement_weak.upgrade().is_none());
            assert_eq!((&*(saved.word as *const Str)).as_str(), "snapshot");
            drop(saved);
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn wide_record_releases_every_initialized_field() {
        let kinds = [5; 65];
        let layout = RecordDescriptor {
            length: kinds.len(),
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        let leaf = crate::objects::Words::new(2).into_object();
        let weak = crate::ObjectOwner::downgrade(&leaf);
        // SAFETY: the descriptor outlives the record and populated fields retain a live object.
        unsafe {
            let row = imre_record_try_new(&layout);
            assert!(!row.is_null());
            for field in (0..kinds.len()).step_by(2) {
                replace(row, field, crate::ObjectOwner::as_ptr(&leaf) as usize, 5);
            }
            drop(leaf);
            imre_record_release(row);
        }
        assert_eq!(weak.strong_count(), 0);
    }

    #[test]
    fn escaped_array_record_keeps_only_its_own_fields_and_block_alive() {
        let kinds = [3, 3];
        let layout = RecordDescriptor {
            length: kinds.len(),
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        let mut batch = RecordBatch::new(&layout).unwrap();
        let mut rows = crate::objects::words::Words::new(6);
        let mut blocks = Vec::new();
        let mut fields = Vec::new();
        // SAFETY: layout outlives the array and its escaped owner; each first
        // field takes the declared string kind and second fields stay zero.
        unsafe {
            for _ in 0..100 {
                let row = batch.allocate();
                let block = Rc::downgrade(batch.block.as_ref().unwrap());
                if blocks.last().is_none_or(|previous| !block.ptr_eq(previous)) {
                    blocks.push(block);
                }
                let field = StrOwner::new(Str::new("owned field"));
                fields.push(StrOwner::downgrade(&field));
                replace(row, 0, StrOwner::as_ptr(&field) as usize, 3);
                rows.values.push(row as usize);
            }
            assert!(blocks.len() > 1 && blocks.len() < fields.len());
            let escaped = rows.values[20] as *mut usize;
            imre_record_retain(escaped);
            drop(batch);
            drop(rows);
            assert_eq!(
                blocks
                    .iter()
                    .filter(|block| block.strong_count() > 0)
                    .count(),
                1
            );
            assert!(std::ptr::eq(descriptor(escaped), &layout));
            assert!(
                fields
                    .iter()
                    .enumerate()
                    .all(|(index, field)| (field.strong_count() > 0) == (index == 20))
            );

            let replacement = StrOwner::new(Str::new("replacement"));
            replace(escaped, 0, StrOwner::as_ptr(&replacement) as usize, 3);
            assert_eq!(fields[20].strong_count(), 0);
            imre_record_release(escaped);
            assert_eq!(StrOwner::strong_count(&replacement), 1);
            assert!(blocks.iter().all(|block| block.strong_count() == 0));
        }
    }

    #[test]
    fn cached_results_reuse_only_exclusive_records_and_release_their_fields() {
        let kinds = [3, 258];
        let descriptor = RecordDescriptor {
            length: kinds.len(),
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        let original = StrOwner::new(Str::from("cached field"));
        let original_weak = StrOwner::downgrade(&original);
        let replacement = StrOwner::new(Str::from("independent field"));
        let replacement_weak = StrOwner::downgrade(&replacement);
        // SAFETY: immutable metadata outlives every record. Each returned owner
        // is released once, and only a returned result mutates record fields.
        unsafe {
            let cache = imre_record_new(&descriptor);
            let exclusive = imre_record_cached_result(cache);
            assert_eq!(exclusive, cache);
            replace(exclusive, 0, StrOwner::as_ptr(&original) as usize, 3);
            replace(exclusive, 1, Int::small(7).unwrap().into_raw(), 258);
            drop(original);

            let shared = imre_record_cached_result(cache);
            assert_ne!(shared, cache);
            assert_eq!(shared.read(), 0);
            assert_eq!(shared.add(1).read(), 0);
            replace(shared, 0, StrOwner::as_ptr(&replacement) as usize, 3);
            replace(shared, 1, Int::small(9).unwrap().into_raw(), 258);
            drop(replacement);
            assert_eq!(
                (&*cache.cast::<*const Str>().read()).as_str(),
                "cached field"
            );
            assert_eq!(cache.add(1).read(), Int::small(7).unwrap().into_raw());

            imre_record_release(exclusive);
            let reused = imre_record_cached_result(cache);
            assert_eq!(reused, cache);
            imre_record_release(cache);
            assert!(original_weak.upgrade().is_some());
            imre_record_release(reused);
            assert!(original_weak.upgrade().is_none());
            assert!(replacement_weak.upgrade().is_some());
            imre_record_release(shared);
        }
        assert!(replacement_weak.upgrade().is_none());
    }
}
