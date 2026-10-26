//! Reuse the last owner of a long string field between decoded record arrays.

use crate::Str;
use crate::StrOwner;
use std::cell::RefCell;

const STEP: usize = 32;
const CLASSES: usize = 8;
const PER_CLASS: usize = 128;
const IDLE_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct Pool {
    bins: [Vec<StrOwner>; CLASSES],
    capacity: usize,
}

thread_local! {
    static POOL: RefCell<Pool> = RefCell::default();
}

pub(in crate::objects) fn copy(value: &str) -> StrOwner {
    if !(STEP + 1..=STEP * CLASSES).contains(&value.len()) {
        return StrOwner::new(Str::new(value));
    }
    let bin = (value.len() - 1) / STEP;
    let capacity = (bin + 1) * STEP;
    let cached = POOL
        .try_with(|pool| {
            let mut pool = pool.try_borrow_mut().ok()?;
            let owner = pool.bins[bin].pop()?;
            pool.capacity -= capacity;
            Some(owner)
        })
        .ok()
        .flatten();
    let mut owner = cached.unwrap_or_else(|| StrOwner::new(Str::with_capacity(capacity)));
    StrOwner::get_mut(&mut owner)
        .expect("cached string has no observers")
        .push_str(value);
    owner
}

pub(super) fn release(mut owner: StrOwner) {
    let capacity = owner.capacity();
    if owner.len() <= STEP
        || !(STEP + 1..=STEP * CLASSES).contains(&capacity)
        || !capacity.is_multiple_of(STEP)
    {
        return;
    }
    // A retained field or a Weak observer still identifies the old allocation.
    let Some(value) = StrOwner::get_mut(&mut owner) else {
        return;
    };
    value.clear();
    let _ = POOL.try_with(|pool| {
        let Ok(mut pool) = pool.try_borrow_mut() else {
            return;
        };
        let bin = capacity / STEP - 1;
        if pool.bins[bin].len() < PER_CLASS && pool.capacity + capacity <= IDLE_BYTES {
            pool.capacity += capacity;
            pool.bins[bin].push(owner);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_reuse_respects_strong_and_weak_observers() {
        POOL.with(|pool| *pool.borrow_mut() = Pool::default());
        let text = "é\0".repeat(20);
        let owner = copy(&text);
        let alias = StrOwner::clone(&owner);
        let weak = StrOwner::downgrade(&owner);
        release(owner);
        assert_eq!(weak.upgrade().unwrap().as_str(), text);
        release(alias);
        assert!(weak.upgrade().is_none());
        POOL.with(|pool| assert_eq!(pool.borrow().capacity, 0));

        let owner = copy(&text);
        let address = StrOwner::as_ptr(&owner);
        release(owner);
        let replacement = "🦀".repeat(15);
        let owner = copy(&replacement);
        assert_eq!(StrOwner::as_ptr(&owner), address);
        assert_eq!(owner.as_str(), replacement);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn idle_payloads_obey_size_and_capacity_limits() {
        POOL.with(|pool| *pool.borrow_mut() = Pool::default());
        let mut live = Vec::new();
        for bin in (2..=CLASSES).rev() {
            let text = "x".repeat(bin * STEP);
            live.extend((0..PER_CLASS * 2).map(|_| copy(&text)));
        }
        for owner in live {
            release(owner);
        }
        POOL.with(|pool| {
            let pool = pool.borrow();
            assert_eq!(pool.capacity, IDLE_BYTES);
            assert!(pool.bins.iter().all(|bin| bin.len() <= PER_CLASS));
            assert_eq!(
                pool.capacity,
                pool.bins
                    .iter()
                    .flatten()
                    .map(|s| s.capacity())
                    .sum::<usize>()
            );
        });
        release(copy("short"));
        release(copy(&"x".repeat(CLASSES * STEP + 1)));
        POOL.with(|pool| assert_eq!(pool.borrow().capacity, IDLE_BYTES));
    }
}
