//! Weak observers of fixed records. Unobserved records keep their two-word header.
use super::{Header, RecordDescriptor, imre_record_retain};
use crate::objects::{Object, Storage, object};
use crate::{Error, Kind, ObjectOwner, StorageCell, finish};
use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::rc::{Rc, Weak};

const ANCHOR: usize = 2;

struct Anchor {
    layout: *const RecordDescriptor,
    payload: *const usize,
    observers: RefCell<Vec<Weak<dyn Fn()>>>,
    proxy: RefCell<crate::shared::Weak<Object>>,
}

impl Drop for Anchor {
    fn drop(&mut self) {
        // Notify live observers newest first, after upgrading the record has
        // become impossible. Dropping an observer before its record cancels it.
        for observer in self.observers.get_mut().drain(..).rev() {
            if let Some(notify) = observer.upgrade() {
                notify();
            }
        }
    }
}

/// Observe a record without extending its lifetime or exposing a reusable address.
#[derive(Clone)]
pub(in crate::objects) struct WeakRecord(Weak<Anchor>);

impl WeakRecord {
    /// Install a single anchor shared by all weak observers of this record.
    ///
    /// # Safety
    /// `payload` is an owned live record, not a borrowed scratch tuple probe.
    pub(in crate::objects) unsafe fn new(payload: *const usize) -> Self {
        // SAFETY: the caller's owner keeps this header live for the operation.
        let header = unsafe { &*payload.cast::<Header>().sub(1) };
        let layout = header.descriptor.get();
        if header.has_weak_anchor() {
            // SAFETY: borrow the header's one strong anchor without consuming it.
            let anchor = ManuallyDrop::new(unsafe { Rc::from_raw(anchor_pointer(layout)) });
            return Self(Rc::downgrade(&anchor));
        }
        let anchor = Rc::new(Anchor {
            layout,
            payload,
            observers: RefCell::default(),
            proxy: RefCell::default(),
        });
        let observer = Self(Rc::downgrade(&anchor));
        header.descriptor.set(
            Rc::into_raw(anchor)
                .cast::<RecordDescriptor>()
                .map_addr(|addr| addr | ANCHOR),
        );
        observer
    }

    /// Acquire one record owner, or return None once the last owner has dropped.
    pub(in crate::objects) fn upgrade(&self) -> Option<*const usize> {
        let anchor = self.0.upgrade()?;
        // SAFETY: only the live header owns this anchor; records stay on their execution thread.
        unsafe { imre_record_retain(anchor.payload) };
        Some(anchor.payload)
    }

    /// Check liveness without acquiring an owner.
    pub(in crate::objects) fn is_live(&self) -> bool {
        self.0.strong_count() != 0
    }

    /// Keep a weak subscription; the caller owns the notification until it
    /// drops the dictionary entry, including on replacement of the same key.
    pub(in crate::objects) fn notify_on_drop(&self, notify: &Rc<dyn Fn()>) {
        if let Some(anchor) = self.0.upgrade() {
            let mut observers = anchor.observers.borrow_mut();
            observers.retain(|observer| observer.strong_count() != 0);
            observers.push(Rc::downgrade(notify));
        }
    }
}

/// Return the shared callback-free proxy, without making the proxy own its target.
///
/// # Safety
/// `payload` points to a rooted Python class record throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_weak_proxy_new(
    payload: *const usize,
    callable: bool,
) -> *const Object {
    // SAFETY: the caller roots the referent while its weak anchor is installed.
    let target = unsafe { WeakRecord::new(payload) };
    let anchor = target.0.upgrade().expect("rooted referent");
    if let Some(cached) = anchor.proxy.borrow().upgrade() {
        return ObjectOwner::into_raw(cached);
    }
    let handle = ObjectOwner::try_new(Object(StorageCell::new(Storage::WeakProxy(
        target, callable,
    ))))
    .map_err(|_| Error::memory_error());
    finish(
        handle.map(|handle| {
            *anchor.proxy.borrow_mut() = ObjectOwner::downgrade(&handle);
            ObjectOwner::into_raw(handle)
        }),
        std::ptr::null(),
    )
}

/// Own the referent for one operation, raising `ReferenceError` after final release.
///
/// # Safety
/// `proxy` is a rooted weak proxy created by `imre_weak_proxy_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_weak_proxy_target(proxy: *const Object) -> *const usize {
    // SAFETY: the borrow only upgrades the weak record; it cannot invoke Python.
    let target = unsafe {
        object(proxy).0.with(|storage| match storage {
            Storage::WeakProxy(target, _) => target.upgrade(),
            _ => unreachable!("expected a weak proxy"),
        })
    };
    finish(
        target.ok_or_else(|| {
            Error::new(
                Kind::ReferenceError,
                "weakly-referenced object no longer exists",
            )
        }),
        std::ptr::null(),
    )
}

/// The Python type of a proxy remains observable after its referent has expired.
///
/// # Safety
/// `proxy` is a rooted native weak proxy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_weak_proxy_callable(proxy: *const Object) -> bool {
    // SAFETY: reads one immutable flag without upgrading or mutating the referent.
    unsafe {
        object(proxy).0.with(|storage| match storage {
            Storage::WeakProxy(_, callable) => *callable,
            _ => unreachable!("expected a weak proxy"),
        })
    }
}

/// Clear the weak-anchor tag while retaining the pointer's provenance.
fn anchor_pointer(tagged: *const RecordDescriptor) -> *const Anchor {
    tagged.map_addr(|addr| addr & !ANCHOR).cast()
}

impl Header {
    /// An observer keeps the identity of a cached result externally visible.
    pub(super) fn has_weak_anchor(&self) -> bool {
        self.descriptor.get().addr() & ANCHOR != 0
    }

    /// Recover the original layout, including block and tuple tags on an observed record.
    pub(super) fn layout_pointer(&self) -> *const RecordDescriptor {
        let layout = self.descriptor.get();
        if layout.addr() & ANCHOR == 0 {
            layout
        } else {
            // SAFETY: the header owns the anchor while its tag is present.
            unsafe { (*anchor_pointer(layout)).layout }
        }
    }

    /// Invalidate observers before releasing fields or returning the allocation.
    pub(super) fn drop_weak_anchor(&self) {
        let layout = self.descriptor.get();
        if layout.addr() & ANCHOR != 0 {
            // SAFETY: final release consumes the header's single strong anchor.
            let anchor = unsafe { Rc::from_raw(anchor_pointer(layout)) };
            self.descriptor.set(anchor.layout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::records::{
        RecordBatch, descriptor, imre_record_cached_result, imre_record_new, imre_record_release,
    };

    #[test]
    fn proxy_cache_is_weak_in_both_directions() {
        let layout = RecordDescriptor {
            length: 0,
            kinds: std::ptr::null(),
            semantics: std::ptr::null(),
        };
        // SAFETY: each raw ownership transfer is balanced below; the descriptor outlives the record.
        unsafe {
            let target = imre_record_new(&layout);
            let first = ObjectOwner::from_raw(imre_weak_proxy_new(target, true));
            let alias = ObjectOwner::from_raw(imre_weak_proxy_new(target, true));
            assert_eq!(ObjectOwner::as_ptr(&first), ObjectOwner::as_ptr(&alias));
            let prior = ObjectOwner::downgrade(&first);
            drop((first, alias));
            assert!(prior.upgrade().is_none());
            let proxy = ObjectOwner::from_raw(imre_weak_proxy_new(target, true));
            let temporary = imre_weak_proxy_target(ObjectOwner::as_ptr(&proxy));
            assert_eq!(target.cast_const(), temporary);
            imre_record_release(target);
            assert!(imre_weak_proxy_callable(ObjectOwner::as_ptr(&proxy)));
            imre_record_release(temporary);
            assert!(imre_weak_proxy_target(ObjectOwner::as_ptr(&proxy)).is_null());
            let error = ObjectOwner::from_raw(crate::objects::exceptions::imre_error_take());
            assert_eq!(
                crate::objects::exceptions::kind_of(error.as_ref()),
                Kind::ReferenceError
            );
        }
    }

    #[test]
    fn observers_track_identity_through_retain_cache_and_final_release() {
        let layout = RecordDescriptor {
            length: 0,
            kinds: std::ptr::null(),
            semantics: std::ptr::null(),
        };
        for packed in [false, true] {
            let mut batch = RecordBatch::new(&layout).unwrap();
            // SAFETY: the zero-field layout outlives all owners; each returned owner is released once.
            unsafe {
                let original = if packed {
                    batch.allocate()
                } else {
                    imre_record_new(&layout)
                };
                let first = WeakRecord::new(original);
                let second = WeakRecord::new(original);
                assert!(std::ptr::eq(descriptor(original), &layout));
                let next = imre_record_cached_result(original);
                assert_ne!(original, next);
                let retained = second.upgrade().unwrap();
                assert_eq!(retained, original.cast_const());
                imre_record_release(original);
                drop(batch);
                assert!(first.is_live());
                imre_record_release(retained);
                assert!(first.upgrade().is_none());
                assert!(!second.is_live());
                imre_record_release(next);
            }
        }
    }
}
