//! Single-threaded strong and weak owners for objects passed across the native ABI.

use std::borrow::Borrow;
use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ops::Deref;
use std::ptr::NonNull;

use allocator_api2::alloc::AllocError;
use allocator_api2::boxed::Box;

#[repr(C)]
struct Allocation<T> {
    // The first field is the value pointer exposed to generated code.
    value: UnsafeCell<MaybeUninit<T>>,
    strong: Cell<usize>,
    // Strong owners collectively hold one weak count until the value drops.
    weak: Cell<usize>,
}

#[repr(transparent)]
pub(crate) struct Shared<T>(NonNull<Allocation<T>>);

pub(crate) struct Weak<T>(Option<NonNull<Allocation<T>>>);

fn increment(count: &Cell<usize>) {
    count.set(
        count
            .get()
            .checked_add(1)
            .unwrap_or_else(|| std::process::abort()),
    );
}

fn release_weak<T>(pointer: NonNull<Allocation<T>>) {
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

impl<T> Shared<T> {
    pub(crate) fn try_new(value: T) -> Result<Self, AllocError> {
        // Allocate before suppressing the value's destructor so a refused
        // slot allocation also drops any resources owned by value.
        let mut slot = Box::<Allocation<T>>::try_new_uninit()?;
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

    pub(crate) fn new(value: T) -> Self {
        Self::try_new(value).unwrap_or_else(|_| {
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<Allocation<T>>())
        })
    }

    pub(crate) fn as_ptr(this: &Self) -> *const T {
        this.0.cast::<T>().as_ptr()
    }

    pub(crate) fn into_raw(this: Self) -> *const T {
        let pointer = Self::as_ptr(&this);
        std::mem::forget(this);
        pointer
    }

    /// Reconstitute one strong count transferred by into_raw.
    pub(crate) unsafe fn from_raw(pointer: *const T) -> Self {
        // SAFETY: value is the first field and its wrappers preserve the layout.
        Self(unsafe { NonNull::new_unchecked(pointer.cast_mut().cast()) })
    }

    /// Acquire one strong count while the supplied string word stays rooted.
    pub(crate) unsafe fn acquire_word(word: usize) -> Self {
        // SAFETY: the caller roots this exact native value for the acquisition.
        let owner = ManuallyDrop::new(unsafe { Self::from_raw(word as *const T) });
        Self::clone(&owner)
    }

    pub(crate) unsafe fn increment_strong_count(pointer: *const T) {
        // SAFETY: the caller roots the owner being copied.
        let owner = ManuallyDrop::new(unsafe { Self::from_raw(pointer) });
        std::mem::forget(Self::clone(&owner));
    }

    pub(crate) unsafe fn decrement_strong_count(pointer: *const T) {
        // SAFETY: the caller transfers exactly one live strong count.
        drop(unsafe { Self::from_raw(pointer) });
    }

    pub(crate) fn strong_count(this: &Self) -> usize {
        // SAFETY: this strong owner keeps the allocation live.
        unsafe { this.0.as_ref() }.strong.get()
    }

    pub(crate) fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.0 == other.0
    }

    pub(crate) fn downgrade(this: &Self) -> Weak<T> {
        // SAFETY: this strong owner keeps the allocation live.
        increment(&unsafe { this.0.as_ref() }.weak);
        Weak(Some(this.0))
    }

    pub(crate) fn try_unwrap(this: Self) -> Result<T, Self> {
        // SAFETY: this strong owner keeps the allocation alive.
        let allocation = unsafe { this.0.as_ref() };
        if allocation.strong.get() != 1 {
            return Err(this);
        }
        let this = ManuallyDrop::new(this);
        allocation.strong.set(0);
        // SAFETY: the only strong count is consumed, and weak upgrades now fail.
        let value = unsafe { (*allocation.value.get()).assume_init_read() };
        release_weak(this.0);
        Ok(value)
    }

    pub(crate) fn get_mut(this: &mut Self) -> Option<&mut T> {
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
    pub(crate) fn make_mut(this: &mut Self) -> &mut T
    where
        T: Clone + Default,
    {
        if Self::get_mut(this).is_none() {
            let value = if Self::strong_count(this) == 1 {
                // SAFETY: the sole strong owner has exclusive access. Moving
                // the value leaves an default value for the old slot to drop.
                let slot = unsafe { this.0.as_ref() }.value.get();
                // SAFETY: no other strong owner can access the initialized value.
                std::mem::take(unsafe { (*slot).assume_init_mut() })
            } else {
                (**this).clone()
            };
            *this = Self::new(value);
        }
        Self::get_mut(this).expect("unique new value owner")
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        // SAFETY: self owns a live count in this single-threaded allocation.
        increment(&unsafe { self.0.as_ref() }.strong);
        Self(self.0)
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        // SAFETY: self owns exactly one strong count.
        let allocation = unsafe { self.0.as_ref() };
        let count = allocation.strong.get() - 1;
        allocation.strong.set(count);
        if count == 0 {
            // SAFETY: the value cannot be read or upgraded after strong reaches
            // zero. The implicit weak count roots the allocation during drop.
            let _implicit = Weak(Some(self.0));
            // SAFETY: the implicit weak guard also releases storage on unwind.
            unsafe { (*allocation.value.get()).assume_init_drop() };
        }
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: a strong owner keeps the initialized value alive.
        unsafe { &*Self::as_ptr(self) }
    }
}

impl<T> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T> Borrow<T> for Shared<T> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: Hash> Hash for Shared<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}

impl<T: fmt::Display> fmt::Display for Shared<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_ref(), f)
    }
}

impl<T> AsRef<T> for Shared<T> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: fmt::Debug> fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_ref(), f)
    }
}

impl<T: PartialEq> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl<T: Eq> Eq for Shared<T> {}

impl<T> Default for Weak<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T> Weak<T> {
    /// Compare allocation identity without reading a value whose last owner may have dropped.
    pub(crate) fn as_ptr(&self) -> *const T {
        self.0.map_or(std::ptr::null(), |p| p.cast::<T>().as_ptr())
    }

    pub(crate) fn strong_count(&self) -> usize {
        // SAFETY: a live weak observer roots the counters.
        self.0.map_or(0, |p| unsafe { p.as_ref() }.strong.get())
    }

    pub(crate) fn upgrade(&self) -> Option<Shared<T>> {
        let pointer = self.0?;
        // SAFETY: this weak owner keeps the counters alive even after value drops.
        let strong = &unsafe { pointer.as_ref() }.strong;
        if strong.get() == 0 {
            return None;
        }
        increment(strong);
        Some(Shared(pointer))
    }
}

impl<T> Clone for Weak<T> {
    fn clone(&self) -> Self {
        if let Some(pointer) = self.0 {
            // SAFETY: this weak owner keeps the counter live.
            increment(&unsafe { pointer.as_ref() }.weak);
        }
        Self(self.0)
    }
}

impl<T> Drop for Weak<T> {
    fn drop(&mut self) {
        if let Some(pointer) = self.0 {
            release_weak(pointer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug)]
    struct CountDrop<'a>(&'a Cell<usize>);

    impl Drop for CountDrop<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn unwrap_moves_the_last_value_with_live_observers() {
        let drops = Cell::new(0);
        let owner = Shared::new(CountDrop(&drops));
        let shared = owner.clone();
        let weak = Shared::downgrade(&owner);
        let owner = Shared::try_unwrap(owner).unwrap_err();
        drop(shared);
        let value = Shared::try_unwrap(owner).unwrap();
        assert!(weak.upgrade().is_none());
        drop(weak);
        assert_eq!(drops.get(), 0);
        drop(value);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn a_value_can_release_its_own_weak_observer_during_drop() {
        struct ObservesSelf(RefCell<Weak<ObservesSelf>>);
        impl Drop for ObservesSelf {
            fn drop(&mut self) {
                assert!(self.0.get_mut().upgrade().is_none());
            }
        }

        let owner = Shared::new(ObservesSelf(RefCell::default()));
        *owner.as_ref().0.borrow_mut() = Shared::downgrade(&owner);
        drop(owner);
    }

    #[test]
    fn a_panicking_value_releases_its_implicit_weak_count() {
        struct Panics;
        impl Drop for Panics {
            fn drop(&mut self) {
                panic!("drop probe");
            }
        }

        let owner = Shared::new(Panics);
        let weak = Shared::downgrade(&owner);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(owner)));
        assert!(result.is_err());
        assert!(weak.upgrade().is_none());
    }
}
