//! Logger enablement caches are public, live dictionaries with manager-wide invalidation.

use super::*;
use crate::objects::mappings::{dict_set, imre_dict_clear, imre_dict_new, lookup};

/// Allocate the same integer-keyed boolean cache that Python stores on each logger.
pub(super) fn new() -> Rc<Object> {
    // SAFETY: the dictionary constructor transfers its new strong reference.
    unsafe { Rc::from_raw(imre_dict_new(2, 1)) }
}

impl Registry {
    /// Keep previously returned dictionary objects live and empty their contents in place.
    pub(super) fn clear_enabled(&self) {
        for entry in self.entries.values() {
            // SAFETY: entries root separate dictionaries containing only integers and booleans.
            unsafe { imre_dict_clear(Rc::as_ptr(&entry.enabled)) }
        }
    }

    /// Test the manager limit first. The disabled flag has already bypassed this cache.
    pub(super) fn enabled(&self, name: &str, level: &Int) -> Result<bool> {
        let cache = Rc::as_ptr(&self.entries[name].enabled);
        // SAFETY: the registry owns the map, and the exact integer key is rooted by the caller.
        if let Some(hit) = unsafe { lookup(cache, level.0) } {
            return Ok(hit.into_word() != 0);
        }
        let threshold = self.effective(name);
        let enabled = match (
            level.as_small(),
            self.disable.as_small(),
            threshold.as_small(),
        ) {
            (Some(level), Some(disable), Some(threshold)) => level > disable && level >= threshold,
            _ => level.big() > self.disable.big() && level.big() >= threshold.big(),
        };
        // SAFETY: pure integer keys and boolean values cannot re-enter the registry.
        unsafe { dict_set(cache, level.0, usize::from(enabled)) }?;
        Ok(enabled)
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
    LOGGERS.with_borrow(|logs| Rc::into_raw(Rc::clone(&logs.entries[name.as_str()].enabled)))
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
