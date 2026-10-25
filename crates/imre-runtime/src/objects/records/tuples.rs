//! Immutable tuple records whose arity is known only at run time. Their
//! headers own the field layout just as a packed record header owns its block.

use super::*;
use crate::objects::{Error, Int, Kind, OwnedWord};

/// Start an unpublished display; its element kinds can differ and its length may grow.
#[unsafe(no_mangle)]
pub extern "C" fn imre_tuple_buffer_new() -> *const Object {
    use crate::objects::{Storage, StorageCell};
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::TupleBuffer(Vec::new()),
    ))))
}

/// # Safety
/// The caller roots the private buffer and the value of the exact supplied kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_tuple_buffer_push(
    buffer: *const Object,
    kind: u32,
    word: usize,
) -> u32 {
    use crate::objects::{Storage, object};
    // SAFETY: only the lowering of a tuple display can access this private buffer.
    unsafe {
        object(buffer).0.with_mut(|storage| {
            let Storage::TupleBuffer(items) = storage else {
                unreachable!()
            };
            if items.try_reserve(1).is_err() {
                return Error::allocation().returning(0);
            }
            items.push(WordKind::new(kind).acquire(word));
            1
        })
    }
}

/// # Safety
/// The caller roots this private display buffer until the immutable result owns its fields.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_tuple_buffer_finish(buffer: *const Object) -> *const usize {
    use crate::objects::{Storage, object};
    // SAFETY: the buffer is live and inaccessible to any Python callback.
    let items = unsafe {
        object(buffer).0.with_mut(|storage| {
            let Storage::TupleBuffer(items) = storage else {
                unreachable!()
            };
            std::mem::take(items)
        })
    };
    let result = TupleBuilder::new(items.len()).map(|mut tuple| {
        for item in items {
            tuple.push(item);
        }
        tuple.finish()
    });
    match result {
        Ok(tuple) => tuple.into_word() as *const usize,
        Err(error) => error.returning(std::ptr::null()),
    }
}

/// The header's tagged Rc points here; the descriptor must be the first field.
#[repr(C)]
pub(super) struct TupleLayout {
    descriptor: RecordDescriptor,
    kinds: Vec<u32>,
}

pub(super) const LAYOUT_TAG: usize = 4;
const _: () =
    assert!(align_of::<RecordDescriptor>() > LAYOUT_TAG && align_of::<TupleLayout>() > LAYOUT_TAG);

const SEMANTICS: RecordSemantics = RecordSemantics {
    kind: 1,
    flags: 3,
    name: std::ptr::null(),
    name_len: 0,
    qualified_name: std::ptr::null(),
    qualified_name_len: 0,
    fields: std::ptr::null(),
};

/// Allocate the final field block after the display finishes iterating. Only the
/// initialized prefix participates in cleanup until all fields are published.
struct TupleBuilder {
    base: std::ptr::NonNull<Header>,
    allocation: Layout,
    metadata: Option<Rc<TupleLayout>>,
}

impl TupleBuilder {
    fn new(capacity: usize) -> Result<Self, Error> {
        let allocation = Layout::array::<usize>(capacity)
            .and_then(|words| Layout::new::<Header>().extend(words))
            .map(|(layout, _)| layout)
            .map_err(|_| Error::allocation())?;
        let mut kinds = Vec::new();
        kinds
            .try_reserve_exact(capacity)
            .map_err(|_| Error::allocation())?;
        let metadata = Rc::new(TupleLayout {
            descriptor: RecordDescriptor {
                length: 0,
                kinds: kinds.as_ptr(),
                semantics: &SEMANTICS,
            },
            kinds,
        });
        // SAFETY: the layout includes the nonempty header and room for capacity words.
        let base = std::ptr::NonNull::new(unsafe { alloc(allocation) }.cast::<Header>())
            .ok_or_else(Error::allocation)?;
        Ok(Self {
            base,
            allocation,
            metadata: Some(metadata),
        })
    }

    fn len(&self) -> usize {
        self.metadata.as_ref().unwrap().descriptor.length
    }

    fn push(&mut self, item: OwnedWord) {
        let i = self.len();
        let meta = Rc::get_mut(self.metadata.as_mut().unwrap()).unwrap();
        assert!(i < (self.allocation.size() - size_of::<Header>()) / size_of::<usize>());
        meta.kinds.push(item.kind.raw());
        // SAFETY: the reserved, aligned slot at i is private and uninitialized.
        unsafe {
            item.initialize_record_field(self.base.as_ptr().add(1).cast::<usize>().add(i));
        }
        meta.descriptor.length += 1;
    }

    fn finish(mut self) -> OwnedWord {
        assert_eq!(self.allocation, layout(self.len()));
        let descriptor = Rc::into_raw(self.metadata.take().unwrap())
            .cast::<RecordDescriptor>()
            .map_addr(|addr| addr | LAYOUT_TAG);
        // SAFETY: every slot described by the now immutable metadata is initialized.
        // The ordinary final-record release owns both the allocation and its metadata.
        unsafe {
            self.base.as_ptr().write(Header {
                references: Cell::new(1),
                descriptor: Cell::new(descriptor),
            });
            OwnedWord::from_owned(self.base.as_ptr().add(1) as usize, WordKind::new(6))
        }
    }
}

impl Drop for TupleBuilder {
    fn drop(&mut self) {
        if let Some(meta) = &self.metadata {
            // SAFETY: before publication we own the allocation and exactly the
            // initialized prefix. An exception releases it in tuple order.
            unsafe {
                release_fields::<false>(
                    self.base.as_ptr().add(1).cast(),
                    &meta.descriptor,
                    0..meta.descriptor.length,
                );
                dealloc(self.base.as_ptr().cast(), self.allocation);
            }
        }
    }
}

/// # Safety
/// The caller roots an initialized native tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_tuple_len(tuple: *const usize) -> usize {
    // SAFETY: no tuple mutation or destruction can overlap the length read.
    Int::from_usize(unsafe { descriptor(tuple) }.length).into_raw()
}

/// # Safety
/// Tuple and index remain rooted. Return one owned word of the statically proven kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_tuple_get(tuple: *const usize, index: usize) -> usize {
    // SAFETY: both input roots are valid throughout the lookup.
    unsafe {
        let desc = descriptor(tuple);
        let Ok(i) = crate::decode_index(&Int::borrow(index)) else {
            return Error::new(
                Kind::IndexError,
                "cannot fit 'int' into an index-sized integer",
            )
            .returning(0);
        };
        let Ok(i) = crate::index(i, desc.length) else {
            return Error::new(Kind::IndexError, "tuple index out of range").returning(0);
        };
        let kind = WordKind::new(*desc.kinds.add(i));
        kind.acquire(kind.read_record_field(tuple.add(i)))
            .into_word()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_observed_tuple_cache_transfers_only_its_layout_to_a_fresh_result() {
        let kind = WordKind::new(2);
        let zero = Int::small(0).unwrap().into_raw();
        let first = Int::small(17).unwrap().into_raw();
        let second = Int::small(23).unwrap().into_raw();
        let mut builder = TupleBuilder::new(1).ok().unwrap();
        // SAFETY: the integers are immediate; every record owner is released once.
        unsafe {
            builder.push(kind.acquire(first));
            let original = builder.finish();
            let payload = original.word as *const usize;
            let observer = weak::WeakRecord::new(payload);
            assert_eq!(imre_tuple_get(payload, zero), first);
            let copy = imre_record_try_cached_result(payload);
            assert!(!copy.is_null());
            assert_ne!(payload, copy.cast_const());
            kind.acquire(second).initialize_record_field(copy);
            assert_eq!(imre_tuple_get(payload, zero), first);
            drop(original);
            assert!(!observer.is_live());
            assert!(observer.upgrade().is_none());
            assert_eq!(imre_tuple_get(copy, zero), second);
            assert_eq!(imre_tuple_len(copy), Int::small(1).unwrap().into_raw());
            imre_record_release(copy);
        }
    }

    #[test]
    fn discarding_an_unfinished_display_releases_its_captured_values() {
        let text = crate::StrOwner::new(crate::Str::from("earlier field"));
        // SAFETY: the new display owns one reference; text roots the exact
        // input representation until each successful push retains it.
        unsafe {
            let buffer = crate::ObjectOwner::from_raw(imre_tuple_buffer_new());
            for _ in 0..3 {
                assert_eq!(
                    imre_tuple_buffer_push(
                        crate::ObjectOwner::as_ptr(&buffer),
                        3,
                        crate::StrOwner::as_ptr(&text) as usize
                    ),
                    1
                );
            }
            assert_eq!(crate::StrOwner::strong_count(&text), 4);
            drop(buffer);
        }
        assert_eq!(crate::StrOwner::strong_count(&text), 1);
    }

    #[test]
    fn wide_nested_tuple_releases_its_descriptor_after_the_last_field() {
        let text = crate::StrOwner::new(crate::Str::from("shared"));
        let mut outer = TupleBuilder::new(96).ok().unwrap();
        for _ in 0..96 {
            let mut child = TupleBuilder::new(1).ok().unwrap();
            // SAFETY: the shared string roots this exact representation.
            child
                .push(unsafe { WordKind::new(3).acquire(crate::StrOwner::as_ptr(&text) as usize) });
            outer.push(child.finish());
        }
        let tuple = outer.finish();
        // SAFETY: the published tuple owns its dynamic layout. Acquire a
        // temporary reference to create the observer without consuming its owner.
        let layout = unsafe {
            let header = &*(tuple.word as *const usize).cast::<Header>().sub(1);
            let layout = header.tuple_layout().unwrap();
            Rc::increment_strong_count(layout);
            let owner = Rc::from_raw(layout);
            Rc::downgrade(&owner)
        };
        assert_eq!(crate::StrOwner::strong_count(&text), 97);
        drop(tuple);
        assert_eq!(crate::StrOwner::strong_count(&text), 1);
        assert!(layout.upgrade().is_none());
    }
}
