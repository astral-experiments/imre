//! Opaque, single-threaded Python.h references. Tagged native values preserve
//! list identity and allow PyList_GetItem to borrow the actual stored integer.

use super::{ModuleDef, Py, extensions_error};
use crate::objects::records::{imre_record_release, imre_record_retain};
use crate::{
    Bytes, Int, Kind, List, imre_int_release, imre_int_retain, imre_list_release, imre_list_retain,
};
use num_bigint::BigInt;
use smallvec::SmallVec;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

const BYTES: usize = 2;
const LIST: usize = 4;
const BIG_INT: usize = 6;
const MASK: usize = 7;
const _: () = assert!(
    align_of::<BigInt>() >= 8
        && align_of::<List>() >= 8
        && align_of::<Bytes>() >= 8
        && align_of::<Boxed>() >= 8
);

pub(super) enum Boxed {
    Module(*const ModuleDef),
    Tuple(SmallVec<[Owned; 2]>),
    IntCallback(*const usize),
}

thread_local! {
    // Aliases to an existing callable must compare equal as PyObject pointers
    // while C owns it. Weak interning never prolongs the receiver's lifetime.
    static CALLBACKS: RefCell<HashMap<usize, Weak<Boxed>>> = RefCell::default();
    static EMPTY_TUPLE: Cell<Option<Rc<Boxed>>> = const { Cell::new(None) };
}

/// A previous unobserved argument tuple can provide this call's allocation.
pub(super) fn argument_tuple(values: SmallVec<[Owned; 2]>) -> Rc<Boxed> {
    if let Some(mut tuple) = EMPTY_TUPLE.take() {
        let Some(Boxed::Tuple(storage)) = Rc::get_mut(&mut tuple) else {
            unreachable!("only unique empty argument tuples enter the cache");
        };
        *storage = values;
        tuple
    } else {
        Rc::new(Boxed::Tuple(values))
    }
}

/// A tuple retained or returned by C keeps its original arguments and identity.
pub(super) fn release_arguments(mut tuple: Rc<Boxed>) {
    if let Some(Boxed::Tuple(values)) = Rc::get_mut(&mut tuple) {
        // Release all values and any spilled argument buffer before caching
        // an empty allocation. Drops may reenter; no TLS borrow spans them.
        drop(std::mem::take(values));
        drop(EMPTY_TUPLE.replace(Some(tuple)));
    }
}

pub(super) fn clear_tuple_cache() {
    drop(EMPTY_TUPLE.take());
}

impl Drop for Boxed {
    fn drop(&mut self) {
        match self {
            Self::IntCallback(record) => {
                let _ = CALLBACKS.try_with(|cache| cache.borrow_mut().remove(&(*record as usize)));
                // SAFETY: this handle owns one callable record reference.
                unsafe { imre_record_release(*record) };
            }
            Self::Module(def) => {
                // SAFETY: C keeps its module definition and function pointers
                // alive until shutdown. Only the final owning reference gets here.
                if let Some(free) = unsafe { (**def).free } {
                    // SAFETY: the C hook receives its still-live module pointer.
                    unsafe { free(std::ptr::from_mut(self).cast()) };
                }
            }
            Self::Tuple(_) => {}
        }
    }
}

#[repr(transparent)]
pub(super) struct Owned(Py);

impl Owned {
    pub(super) fn new(object: Boxed) -> Self {
        Self(Py(Rc::into_raw(Rc::new(object)).cast_mut().cast()))
    }

    pub(super) fn integer(value: Int) -> Self {
        Self(Py::integer(value.into_raw()))
    }

    pub(super) fn try_bytes(value: Bytes) -> Option<Self> {
        let ptr = crate::BytesOwner::into_raw(crate::BytesOwner::try_new(value).ok()?)
            .cast_mut()
            .cast::<std::ffi::c_void>();
        Some(Self(Py(ptr.map_addr(|address| address | BYTES))))
    }

    pub(super) fn as_py(&self) -> Py {
        self.0
    }

    /// Transfers a reference returned by an extension to Rust cleanup.
    pub(super) unsafe fn take(value: Py) -> Self {
        Self(value)
    }

    pub(super) fn into_py(self) -> Py {
        std::mem::ManuallyDrop::new(self).0
    }

    /// The compiler supplies a live word of the indicated exact native type.
    pub(super) unsafe fn from_native(kind: u8, word: usize) -> Option<Self> {
        let value = match kind {
            0 => Py::NONE,
            1 => Py::integer(word),
            2 => Py::bits(word | BYTES),
            3 => Py::bits(word | LIST),
            4 => {
                let object = CALLBACKS.with_borrow_mut(|cache| {
                    if let Some(existing) = cache.get(&word).and_then(Weak::upgrade) {
                        return existing;
                    }
                    // SAFETY: the exact callable word points to a rooted native record.
                    unsafe { imre_record_retain(word as *const usize) };
                    let object = Rc::new(Boxed::IntCallback(word as *const usize));
                    cache.insert(word, Rc::downgrade(&object));
                    object
                });
                return Some(Self(Py(Rc::into_raw(object).cast_mut().cast())));
            }
            _ => {
                return extensions_error(
                    Kind::SystemError,
                    "invalid extension parameter type",
                    None,
                );
            }
        };
        // SAFETY: the caller's native value outlives this acquisition.
        unsafe { value.retain() };
        Some(Self(value))
    }

    /// Returns an independently owned value in the calling module's native ABI.
    pub(super) fn native(&self, kind: u32) -> Option<usize> {
        let value = self.0;
        // SAFETY: self roots this particular handle throughout inspection/acquisition.
        unsafe {
            match kind {
                0 if value == Py::NONE => Some(0),
                1 if let Some(word) = value.int_word() => {
                    imre_int_retain(word);
                    Some(word)
                }
                2 if let Some(bytes) = value.bytes() => {
                    crate::retain_bytes(bytes);
                    Some(bytes as usize)
                }
                3 if let Some(list) = value.list() => {
                    imre_list_retain(list);
                    Some(list as usize)
                }
                4 if let Some(Boxed::IntCallback(record)) = value.boxed() => {
                    imre_record_retain(*record);
                    Some(*record as usize)
                }
                _ => extensions_error(
                    Kind::TypeError,
                    "extension result does not match its declared type",
                    None,
                ),
            }
        }
    }
}

impl Clone for Owned {
    fn clone(&self) -> Self {
        // SAFETY: self owns the source reference.
        unsafe { self.0.retain() };
        Self(self.0)
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: exactly one owned reference is being consumed.
        unsafe { self.0.release() }
    }
}

impl Py {
    // All non-null pointers with this tag carry an aligned crate::BytesOwner address.
    pub(super) const NONE: Self = Self(std::ptr::without_provenance_mut(BYTES));

    pub(super) fn bits(word: usize) -> Self {
        Self(std::ptr::with_exposed_provenance_mut(word))
    }

    pub(super) fn integer(word: usize) -> Self {
        Self::bits(if word & 1 == 1 { word } else { word | BIG_INT })
    }

    pub(super) fn int_word(self) -> Option<usize> {
        let word = self.0.addr();
        if word & 1 == 1 {
            Some(word)
        } else if word & MASK == BIG_INT {
            Some(word & !MASK)
        } else {
            None
        }
    }

    pub(super) fn bytes(self) -> Option<*const Bytes> {
        (self != Self::NONE && self.0.addr() & MASK == BYTES).then(|| {
            self.0
                .map_addr(|address| address & !MASK)
                .cast_const()
                .cast()
        })
    }

    pub(super) fn list(self) -> Option<*const List> {
        (self.0.addr() & MASK == LIST).then(|| {
            self.0
                .map_addr(|address| address & !MASK)
                .cast_const()
                .cast()
        })
    }

    /// Caller roots this opaque PyObject for the entire returned reference.
    pub(super) unsafe fn boxed<'a>(self) -> Option<&'a Boxed> {
        if self.0.is_null() || self.0.addr() & MASK != 0 || self.exception_kind().is_some() {
            return None;
        }
        // SAFETY: all remaining valid untagged pointers are Rc<Boxed> payloads.
        Some(unsafe { &*self.0.cast::<Boxed>() })
    }

    pub(super) unsafe fn retain(self) {
        if let Some(word) = self.int_word() {
            // SAFETY: the caller roots the integer through this acquisition.
            unsafe { imre_int_retain(word) };
        } else if let Some(bytes) = self.bytes() {
            // SAFETY: the caller roots the immutable byte allocation.
            unsafe { crate::retain_bytes(bytes) };
        } else if let Some(list) = self.list() {
            // SAFETY: this tag carries the original list, with no copied backing storage.
            unsafe { imre_list_retain(list) };
        } else if !self.0.is_null() && self != Self::NONE && self.exception_kind().is_none() {
            // SAFETY: all remaining valid handles are independently counted boxed objects.
            unsafe { Rc::increment_strong_count(self.0.cast::<Boxed>()) };
        }
    }

    pub(super) unsafe fn release(self) {
        if let Some(word) = self.int_word() {
            // SAFETY: the caller transfers one integer reference.
            unsafe { imre_int_release(word) };
        } else if let Some(bytes) = self.bytes() {
            // SAFETY: the caller transfers one immutable bytes reference.
            unsafe { crate::release_bytes(bytes) };
        } else if let Some(list) = self.list() {
            // SAFETY: the caller transfers a reference to this exact native list.
            unsafe { imre_list_release(list) };
        } else if !self.0.is_null() && self != Self::NONE && self.exception_kind().is_none() {
            // SAFETY: the caller transfers one boxed-object reference.
            unsafe { Rc::decrement_strong_count(self.0.cast::<Boxed>()) };
        }
    }
}
