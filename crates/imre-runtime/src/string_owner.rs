//! Single-threaded strong and weak owners for strings passed across the native ABI.

use std::borrow::Borrow;
use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ops::Deref;
use std::ptr::NonNull;

use allocator_api2::alloc::AllocError;
use allocator_api2::boxed::Box;

use crate::Str;

#[repr(C)]
struct Allocation {
    // The first field is the string pointer exposed to generated code.
    value: UnsafeCell<MaybeUninit<Str>>,
    strong: Cell<usize>,
    // Strong owners collectively hold one weak count until the string drops.
    weak: Cell<usize>,
}

#[repr(transparent)]
pub(crate) struct StrOwner(NonNull<Allocation>);

#[derive(Default)]
pub(crate) struct WeakStr(Option<NonNull<Allocation>>);

fn increment(count: &Cell<usize>) {
    count.set(
        count
            .get()
            .checked_add(1)
            .unwrap_or_else(|| std::process::abort()),
    );
}

fn release_weak(pointer: NonNull<Allocation>) {
    // SAFETY: the caller transfers a live weak count, including the implicit one.
    let allocation = unsafe { pointer.as_ref() };
    let count = allocation.weak.get() - 1;
    allocation.weak.set(count);
    if count == 0 {
        // SAFETY: the last strong owner already dropped value. The final weak
        // owner releases the original Box with its exact type and allocator.
        drop(unsafe { Box::from_raw(pointer.as_ptr()) });
    }
}

impl StrOwner {
    pub(crate) fn try_new(value: Str) -> Result<Self, AllocError> {
        // Allocate before suppressing the string's destructor so a refused
        // slot allocation also drops any heap buffer owned by value.
        let mut slot = Box::<Allocation>::try_new_uninit()?;
        slot.write(Allocation {
            value: UnsafeCell::new(MaybeUninit::new(value)),
            strong: Cell::new(1),
            weak: Cell::new(1),
        });
        // SAFETY: write initialized every field above.
        Ok(Self(NonNull::from(Box::leak(unsafe {
            slot.assume_init()
        }))))
    }

    pub(crate) fn new(value: Str) -> Self {
        Self::try_new(value).unwrap_or_else(|_| {
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<Allocation>())
        })
    }

    pub(crate) fn as_ptr(this: &Self) -> *const Str {
        this.0.cast::<Str>().as_ptr()
    }

    pub(crate) fn into_raw(this: Self) -> *const Str {
        let pointer = Self::as_ptr(&this);
        std::mem::forget(this);
        pointer
    }

    /// Reconstitute one strong count transferred by into_raw.
    pub(crate) unsafe fn from_raw(pointer: *const Str) -> Self {
        // SAFETY: value is the first field and its wrappers preserve the layout.
        Self(unsafe { NonNull::new_unchecked(pointer.cast_mut().cast()) })
    }

    /// Acquire one strong count while the supplied string word stays rooted.
    pub(crate) unsafe fn acquire_word(word: usize) -> Self {
        // SAFETY: the caller roots this exact native string for the acquisition.
        let owner = ManuallyDrop::new(unsafe { Self::from_raw(word as *const Str) });
        Self::clone(&owner)
    }

    pub(crate) unsafe fn increment_strong_count(pointer: *const Str) {
        // SAFETY: the caller roots the owner being copied.
        let owner = ManuallyDrop::new(unsafe { Self::from_raw(pointer) });
        std::mem::forget(Self::clone(&owner));
    }

    pub(crate) unsafe fn decrement_strong_count(pointer: *const Str) {
        // SAFETY: the caller transfers exactly one live strong count.
        drop(unsafe { Self::from_raw(pointer) });
    }

    #[cfg(test)]
    pub(crate) fn strong_count(this: &Self) -> usize {
        // SAFETY: this strong owner keeps the allocation live.
        unsafe { this.0.as_ref() }.strong.get()
    }

    #[cfg(test)]
    pub(crate) fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.0 == other.0
    }

    pub(crate) fn downgrade(this: &Self) -> WeakStr {
        // SAFETY: this strong owner keeps the allocation live.
        increment(&unsafe { this.0.as_ref() }.weak);
        WeakStr(Some(this.0))
    }

    pub(crate) fn get_mut(this: &mut Self) -> Option<&mut Str> {
        // SAFETY: this owner keeps the counters live.
        let allocation = unsafe { this.0.as_ref() };
        if allocation.strong.get() == 1 && allocation.weak.get() == 1 {
            // SAFETY: there are no other owners or observers that could upgrade.
            Some(unsafe { (*allocation.value.get()).assume_init_mut() })
        } else {
            None
        }
    }

    #[cfg(test)]
    pub(crate) fn make_mut(this: &mut Self) -> &mut Str {
        if Self::get_mut(this).is_none() {
            let value = if Self::strong_count(this) == 1 {
                // SAFETY: the sole strong owner has exclusive access. Moving
                // the value leaves an empty string for the old slot to drop.
                let slot = unsafe { this.0.as_ref() }.value.get();
                // SAFETY: no other strong owner can access the initialized string.
                std::mem::take(unsafe { (*slot).assume_init_mut() })
            } else {
                (**this).clone()
            };
            *this = Self::new(value);
        }
        Self::get_mut(this).expect("unique new string owner")
    }
}

impl Clone for StrOwner {
    fn clone(&self) -> Self {
        // SAFETY: self owns a live count in this single-threaded allocation.
        increment(&unsafe { self.0.as_ref() }.strong);
        Self(self.0)
    }
}

impl Drop for StrOwner {
    fn drop(&mut self) {
        // SAFETY: self owns exactly one strong count.
        let allocation = unsafe { self.0.as_ref() };
        let count = allocation.strong.get() - 1;
        allocation.strong.set(count);
        if count == 0 {
            // SAFETY: the value cannot be read or upgraded after strong reaches
            // zero. The implicit weak count roots the allocation during drop.
            unsafe { (*allocation.value.get()).assume_init_drop() };
            release_weak(self.0);
        }
    }
}

impl Deref for StrOwner {
    type Target = Str;

    fn deref(&self) -> &Str {
        // SAFETY: a strong owner keeps the initialized string alive.
        unsafe { &*Self::as_ptr(self) }
    }
}

impl From<Str> for StrOwner {
    fn from(value: Str) -> Self {
        Self::new(value)
    }
}

impl Borrow<Str> for StrOwner {
    fn borrow(&self) -> &Str {
        self
    }
}

impl Hash for StrOwner {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}

impl fmt::Display for StrOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_ref(), f)
    }
}

impl AsRef<Str> for StrOwner {
    fn as_ref(&self) -> &Str {
        self
    }
}

impl fmt::Debug for StrOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_ref(), f)
    }
}

impl PartialEq for StrOwner {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl Eq for StrOwner {}

impl WeakStr {
    #[cfg(test)]
    pub(crate) fn strong_count(&self) -> usize {
        // SAFETY: a live weak observer roots the counters.
        self.0.map_or(0, |p| unsafe { p.as_ref() }.strong.get())
    }

    pub(crate) fn upgrade(&self) -> Option<StrOwner> {
        let pointer = self.0?;
        // SAFETY: this weak owner keeps the counters alive even after value drops.
        let strong = &unsafe { pointer.as_ref() }.strong;
        if strong.get() == 0 {
            return None;
        }
        increment(strong);
        Some(StrOwner(pointer))
    }
}

impl Clone for WeakStr {
    fn clone(&self) -> Self {
        if let Some(pointer) = self.0 {
            // SAFETY: this weak owner keeps the counter live.
            increment(&unsafe { pointer.as_ref() }.weak);
        }
        Self(self.0)
    }
}

impl Drop for WeakStr {
    fn drop(&mut self) {
        if let Some(pointer) = self.0 {
            release_weak(pointer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_references_and_weak_observers_share_one_string() {
        let mut owner =
            StrOwner::try_new(Str::new("a native string longer than the inline buffer")).unwrap();
        let observer = StrOwner::downgrade(&owner);
        let pointer = StrOwner::into_raw(owner.clone());
        assert!(StrOwner::get_mut(&mut owner).is_none());
        // SAFETY: the transferred count roots the raw pointer throughout.
        unsafe {
            StrOwner::increment_strong_count(pointer);
            drop(StrOwner::from_raw(pointer));
            assert_eq!(StrOwner::strong_count(&owner), 2);
            drop(owner);
            let upgraded = observer.upgrade().unwrap();
            assert_eq!(StrOwner::as_ptr(&upgraded), pointer);
            StrOwner::decrement_strong_count(pointer);
            assert_eq!(StrOwner::strong_count(&upgraded), 1);
            drop(upgraded);
        }
        assert!(observer.upgrade().is_none());
        assert!(observer.clone().upgrade().is_none());
    }

    #[test]
    fn copy_on_write_dissociates_observers_and_keeps_other_owners() {
        let mut owner = StrOwner::new(Str::new("a native string longer than the inline buffer"));
        let observer = StrOwner::downgrade(&owner);
        StrOwner::make_mut(&mut owner).push('a');
        assert!(observer.upgrade().is_none());
        let shared = owner.clone();
        StrOwner::make_mut(&mut owner).push('b');
        assert_eq!(owner.len(), shared.len() + 1);
        assert!(!StrOwner::ptr_eq(&owner, &shared));
    }

    #[test]
    fn mutation_waits_for_the_last_observer() {
        let mut owner = StrOwner::new(Str::new("a"));
        let observer = StrOwner::downgrade(&owner);
        let last = observer.clone();
        drop(observer);
        assert!(StrOwner::get_mut(&mut owner).is_none());
        drop(last);
        StrOwner::get_mut(&mut owner).unwrap().push('b');
        assert_eq!(owner.as_str(), "ab");
    }
}
