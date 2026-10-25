//! Native string owners retain the address used by generated code.

use crate::Str;
use crate::shared::{Shared, Weak};

pub(crate) type StrOwner = Shared<Str>;
pub(crate) type WeakStr = Weak<Str>;

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
