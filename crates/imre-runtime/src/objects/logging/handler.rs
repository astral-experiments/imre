//! Named handlers use weak record references, independently of logger ownership.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::super::records::weak::WeakRecord;
use super::Str;

struct Entry {
    handler: WeakRecord,
    _notify: Rc<dyn Fn()>,
}

thread_local! { static HANDLERS: RefCell<HashMap<Rc<str>, Entry>> = RefCell::default(); }

/// Update the weak name map before the caller stores its new name. CPython
/// removes the old key even when a different handler has since claimed it.
///
/// # Safety
/// The caller roots a live record and the optional native string words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_handler_rename(handler: *const usize, old: usize, new: usize) {
    // SAFETY: nonzero optional strings are rooted native string pointers.
    let (old, new) = unsafe { ((old as *const Str).as_ref(), (new as *const Str).as_ref()) };
    HANDLERS.with_borrow_mut(|names| {
        if let Some(old) = old {
            names.remove(old.as_str());
        }
        if let Some(new) = new.filter(|s| !s.as_str().is_empty()) {
            // SAFETY: the method receiver stays live until this call returns.
            let handler = unsafe { WeakRecord::new(handler) };
            let key = Rc::<str>::from(new.as_str());
            let expired = key.clone();
            let notify: Rc<dyn Fn()> = Rc::new(move || {
                // During process teardown the whole registry may already be gone.
                let _ = HANDLERS.try_with(|names| {
                    let mut names = names.borrow_mut();
                    if names
                        .get(&expired)
                        .is_some_and(|entry| !entry.handler.is_live())
                    {
                        names.remove(&expired);
                    }
                });
            });
            handler.notify_on_drop(&notify);
            names.insert(
                key,
                Entry {
                    handler,
                    _notify: notify,
                },
            );
        }
    });
}

/// Return an independent owner of a live handler, or the absent optional word.
///
/// # Safety
/// The caller roots the exact name string through the lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_handler_lookup(name: *const Str) -> usize {
    // SAFETY: the caller owns name for this operation.
    let name = unsafe { &*name }.as_str();
    HANDLERS.with_borrow(|names| {
        names
            .get(name)
            .and_then(|entry| entry.handler.upgrade())
            .map_or(0, |record| record as usize)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StrOwner;
    use crate::objects::records::{RecordDescriptor, imre_record_new, imre_record_release};

    #[test]
    fn final_release_only_deletes_the_current_weak_binding() {
        let layout = RecordDescriptor {
            length: 0,
            kinds: std::ptr::null(),
            semantics: std::ptr::null(),
        };
        let name = StrOwner::new(Str::from("replacement"));
        let key = StrOwner::as_ptr(&name) as usize;
        // SAFETY: the layout and name live through every use and both owners
        // of the second record are released separately.
        unsafe {
            let first = imre_record_new(&layout);
            let second = imre_record_new(&layout);
            imre_log_handler_rename(first, 0, key);
            imre_log_handler_rename(second, 0, key);
            imre_record_release(first);
            let owned = imre_log_handler_lookup(StrOwner::as_ptr(&name));
            assert_eq!(owned, second as usize);
            imre_record_release(second);
            assert!(HANDLERS.with_borrow(|names| names.contains_key("replacement")));
            imre_record_release(owned as *const usize);
            // No later lookup or insertion is required to remove the dead key.
            assert!(!HANDLERS.with_borrow(|names| names.contains_key("replacement")));
        }
    }
}
