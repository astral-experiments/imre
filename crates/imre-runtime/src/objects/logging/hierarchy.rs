//! Operations on the immediate children in the process-wide logger registry.

use super::{LOGGERS, Object, state};
use crate::objects::mappings::{imre_set_new, set_add};

/// Returns a fresh set. A logger attached across missing ancestors is not an
/// immediate child until the missing parent is materialized.
///
/// # Safety
/// The caller roots the exact native logger throughout enumeration.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_children(logger: *const Object) -> *const Object {
    // SAFETY: the registry and caller root the receiver and each inserted key.
    let parent = unsafe { state(logger) };
    let depth = |s: &str| {
        if s == "root" {
            0
        } else {
            1 + s.bytes().filter(|b| *b == b'.').count()
        }
    };
    let child_depth = depth(&parent.name) + 1;
    let result = LOGGERS.with_borrow(|logs| {
        // SAFETY: the constructor returns one independently owned object.
        let children = unsafe { crate::ObjectOwner::from_raw(imre_set_new(5)) };
        if parent.standalone {
            return Ok(crate::ObjectOwner::into_raw(children));
        }
        for (key, entry) in &logs.entries {
            if depth(key) == child_depth && logs.parent_name(key) == Some(parent.name.as_str()) {
                // SAFETY: logger handles are identity keys of representation kind 5.
                unsafe {
                    set_add(
                        crate::ObjectOwner::as_ptr(&children),
                        crate::ObjectOwner::as_ptr(&entry.handle) as usize,
                    )
                }?;
            }
        }
        Ok(crate::ObjectOwner::into_raw(children))
    });
    crate::finish(result, std::ptr::null())
}
