//! Immutable tuple records whose arity is known only at run time. Their
//! headers own the field layout just as a packed record header owns its block.

use super::*;
use crate::objects::{Error, Int, Kind, OwnedWord};

/// Start an unpublished display; its element kinds can differ and its length may grow.
#[unsafe(no_mangle)]
pub extern "C" fn imre_tuple_buffer_new() -> *const Object {
    use crate::objects::{Storage, StorageCell};
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::TupleBuffer(
        Vec::new(),
    )))))
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
    let result = TupleBuilder::new(items.len()).and_then(|mut tuple| {
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

const SEMANTICS: RecordSemantics = RecordSemantics {
    kind: 1,
    flags: 3,
    name: std::ptr::null(),
    name_len: 0,
    qualified_name: std::ptr::null(),
    qualified_name_len: 0,
    fields: std::ptr::null(),
};

/// Reserve the native tuple before asking the input for any item. While the
/// tuple is private only its initialized prefix participates in cleanup.
pub(in crate::objects) struct TupleBuilder {
    base: std::ptr::NonNull<Header>,
    allocation: Layout,
    metadata: Option<Rc<TupleLayout>>,
}

impl TupleBuilder {
    pub(in crate::objects) fn new(capacity: usize) -> Result<Self, Error> {
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

    pub(in crate::objects) fn len(&self) -> usize {
        self.metadata.as_ref().unwrap().descriptor.length
    }

    pub(in crate::objects) fn push(&mut self, item: OwnedWord) {
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

    pub(in crate::objects) fn finish(mut self) -> Result<OwnedWord, Error> {
        let meta = Rc::get_mut(self.metadata.as_mut().unwrap()).unwrap();
        let exact = layout(meta.descriptor.length);
        if exact != self.allocation {
            // CPython only resizes after a nonempty, non-strict short batch.
            // On failure the original allocation and its prefix still belong to self.
            // SAFETY: both layouts use the header alignment and exact is nonempty.
            let base = unsafe {
                std::alloc::realloc(self.base.as_ptr().cast(), self.allocation, exact.size())
            };
            self.base = std::ptr::NonNull::new(base.cast()).ok_or_else(Error::allocation)?;
            self.allocation = exact;
            meta.kinds.shrink_to_fit();
            meta.descriptor.kinds = meta.kinds.as_ptr();
        }
        let descriptor = Rc::into_raw(self.metadata.take().unwrap())
            .cast::<RecordDescriptor>()
            .map_addr(|addr| addr | 2);
        // SAFETY: every slot described by the now immutable metadata is initialized.
        // The ordinary final-record release owns both the allocation and its metadata.
        unsafe {
            self.base.as_ptr().write(Header {
                references: Cell::new(1),
                descriptor,
            });
            Ok(OwnedWord::from_owned(
                self.base.as_ptr().add(1) as usize,
                WordKind::new(6),
            ))
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
    fn wide_nested_tuple_releases_its_descriptor_after_the_last_field() {
        let text = Rc::new(crate::Str::from("shared"));
        let mut outer = TupleBuilder::new(96).ok().unwrap();
        for _ in 0..96 {
            let mut child = TupleBuilder::new(1).ok().unwrap();
            // SAFETY: the shared string roots this exact representation.
            child.push(unsafe { WordKind::new(3).acquire(Rc::as_ptr(&text) as usize) });
            outer.push(child.finish().ok().unwrap());
        }
        let tuple = outer.finish().ok().unwrap();
        // SAFETY: the published tuple owns its dynamic layout. Acquire a
        // temporary reference to create the observer without consuming its owner.
        let layout = unsafe {
            let header = &*(tuple.word as *const usize).cast::<Header>().sub(1);
            let layout = header.tuple_layout().unwrap();
            Rc::increment_strong_count(layout);
            let owner = Rc::from_raw(layout);
            Rc::downgrade(&owner)
        };
        assert_eq!(Rc::strong_count(&text), 97);
        drop(tuple);
        assert_eq!(Rc::strong_count(&text), 1);
        assert!(layout.upgrade().is_none());
    }
}
