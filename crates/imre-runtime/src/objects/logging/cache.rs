//! Logger enablement caches are public, live dictionaries with manager-wide invalidation.

use super::*;
use crate::ObjectOwner;
use crate::objects::mappings::{dict_set, imre_dict_clear, imre_dict_new, lookup};

/// The fast path and the public attribute use one ordered dictionary.
pub(super) struct EnabledCache(ObjectOwner);

impl EnabledCache {
    pub(super) fn new() -> Self {
        // SAFETY: the constructor transfers its fresh dict[int, bool] reference.
        Self(unsafe { ObjectOwner::from_raw(imre_dict_new(2, 1)) })
    }
    pub(super) fn get(&self, IntKey(level): &IntKey) -> Option<bool> {
        // SAFETY: the map and the exact integer key are rooted.
        unsafe { lookup(ObjectOwner::as_ptr(&self.0), level.0) }.map(|value| value.word != 0)
    }
    pub(super) fn insert(&self, IntKey(level): IntKey, enabled: bool) -> Result<()> {
        // SAFETY: the map acquires the key and stores a boolean word.
        unsafe { dict_set(ObjectOwner::as_ptr(&self.0), level.0, usize::from(enabled)) }
    }
    pub(super) fn clear(&self) {
        // SAFETY: the cache stays rooted while its old keys are discarded.
        unsafe { imre_dict_clear(ObjectOwner::as_ptr(&self.0)) }
    }
}

/// # Safety
/// The exact integer level stays rooted until the manager stores its own reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_disable(level: usize) {
    // SAFETY: the borrowed integer is cloned before returning to the caller.
    let value = (*unsafe { Int::borrow(level) }).clone();
    LOGGERS.with_borrow_mut(|logs| {
        logs.disable = value;
        logs.clear_enabled();
    });
}

/// # Safety
/// The exact logger is rooted; the caller receives an owner of its live cache.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_enabled_cache(logger: *const Object) -> *const Object {
    // SAFETY: the caller roots the logger while its name acquires an owner.
    let name = unsafe { name(logger) };
    LOGGERS.with_borrow(|logs| ObjectOwner::into_raw(logs.entries[name.as_str()].enabled.0.clone()))
}

/// # Safety
/// The error description is Python's repr of the invalid level, rooted by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_invalid_level(description: *const Str) -> usize {
    // SAFETY: formatting borrows the live description only during this call.
    let description = unsafe { &*description };
    Error::new(
        Kind::TypeError,
        format!(
            "Level not an integer or a valid string: {}",
            description.as_str()
        ),
    )
    .returning(0)
}
