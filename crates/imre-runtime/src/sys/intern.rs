//! A canonical live string for each interned spelling in the native interpreter.
use crate::Str;
use crate::StrOwner;
use hashbrown::HashTable;
use std::cell::RefCell;
use std::hash::{BuildHasher, RandomState};
use std::mem::ManuallyDrop;

#[derive(Default)]
struct Interned {
    hash: RandomState,
    strings: HashTable<(u64, crate::string_owner::WeakStr)>,
    until_sweep: usize,
}

impl Interned {
    fn try_intern(&mut self, value: &crate::StrOwner) -> Option<crate::StrOwner> {
        if let Some(singleton) = crate::objects::short_string(value) {
            return Some(singleton);
        }
        let hash = self.hash.hash_one(value.as_str());
        if let Some(existing) = self
            .strings
            .iter_hash(hash)
            .filter(|(key, _)| *key == hash)
            .filter_map(|(_, weak)| weak.upgrade())
            .find(|existing| existing.as_str() == value.as_str())
        {
            return Some(existing);
        }
        // Weak owners retain no text after its last caller releases it. Sweep
        // periodically even for a stream of unique spellings; doubling the
        // interval with the live set keeps insertion amortized constant time.
        if self.until_sweep == 0 {
            self.strings.retain(|(_, weak)| weak.strong_count() != 0);
            if self.strings.len().max(64) < self.strings.capacity() / 4 {
                let mut smaller = HashTable::new();
                if smaller
                    .try_reserve(
                        self.strings.len(),
                        |entry: &(u64, crate::string_owner::WeakStr)| entry.0,
                    )
                    .is_ok()
                {
                    for entry in self.strings.drain() {
                        smaller.insert_unique(entry.0, entry, |entry| entry.0);
                    }
                    self.strings = smaller;
                }
            }
            self.until_sweep = self.strings.len().max(64);
        }
        self.until_sweep -= 1;
        // CPython leaves the input unchanged if the interning dictionary
        // cannot grow. The string is valid and remains uninterned.
        if self.strings.try_reserve(1, |entry| entry.0).is_err() {
            return None;
        }
        self.strings
            .insert_unique(hash, (hash, StrOwner::downgrade(value)), |(key, _)| *key);
        Some(StrOwner::clone(value))
    }

    fn contains(&self, value: &Str) -> bool {
        if let Some(singleton) = crate::objects::short_string(value) {
            return std::ptr::eq(StrOwner::as_ptr(&singleton), value);
        }
        self.strings
            .find(self.hash.hash_one(value.as_str()), |(_, weak)| {
                std::ptr::eq(weak.as_ptr(), value)
            })
            .is_some()
    }
}

thread_local! {
    // Native string handles are shared by this interpreter thread.
    static INTERNED: RefCell<Interned> = RefCell::default();
}

/// Share a canonical native owner between typed and runtime-loaded Python.
pub(crate) fn intern_owner(value: &StrOwner) -> StrOwner {
    try_intern_owner(value).unwrap_or_else(|| StrOwner::clone(value))
}

/// Report when the weak table could not register a new spelling.
pub(crate) fn try_intern_owner(value: &StrOwner) -> Option<StrOwner> {
    INTERNED.with_borrow_mut(|table| table.try_intern(value))
}

/// Keep module metadata in the same table as interned literals and sys.intern.
pub(crate) fn canonical(name: &'static str) -> StrOwner {
    intern_owner(&StrOwner::new(Str::const_new(name)))
}

/// # Safety
/// The caller roots one live native string; the return transfers a separate owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sys_intern(value: *const Str) -> *const Str {
    // SAFETY: borrow the caller's owner without consuming its strong reference.
    let value = ManuallyDrop::new(unsafe { StrOwner::from_raw(value) });
    StrOwner::into_raw(intern_owner(&value))
}

/// # Safety
/// The caller roots the exact string whose interning status is queried.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sys_is_interned(value: *const Str) -> u32 {
    // SAFETY: the immutable string lives for the entire lookup.
    u32::from(INTERNED.with_borrow(|table| table.contains(unsafe { &*value })))
}

/// # Safety
/// The compiler supplies a readable UTF-8 literal with program lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_interned_literal(data: *const u8, len: usize) -> *const Str {
    // SAFETY: the literal's backing bytes remain live as long as the program.
    let literal = StrOwner::new(Str::const_new(unsafe { crate::text(data, len) }));
    StrOwner::into_raw(intern_owner(&literal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_keeps_identity_without_keeping_dead_strings_alive() {
        let mut table = Interned::default();
        let first = StrOwner::new(Str::new("same spelling"));
        let second = StrOwner::new((*first).clone());
        let weak = StrOwner::downgrade(&first);
        assert!(StrOwner::ptr_eq(&first, &table.try_intern(&first).unwrap()));
        assert!(StrOwner::ptr_eq(&first, &table.try_intern(&second).unwrap()));
        assert!(table.contains(&first));
        assert!(!table.contains(&second));
        drop(first);
        assert!(weak.upgrade().is_none());
        assert!(StrOwner::ptr_eq(&second, &table.try_intern(&second).unwrap()));
        for i in 0..2048 {
            table.try_intern(&StrOwner::new(Str::new(i.to_string()))).unwrap();
        }
        assert!(table.strings.len() < 70);
        assert!(table.contains(&second));
    }
}
