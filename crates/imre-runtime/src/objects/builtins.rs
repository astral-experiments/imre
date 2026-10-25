//! The default display hook owns the last displayed object in the builtins namespace.
use super::{Error, Kind, Object, boxed_values, exceptions};
use crate::StrOwner;
use std::cell::RefCell;

thread_local! {
    static LAST: RefCell<Option<crate::ObjectOwner>> = const { RefCell::new(None) };
}

// Unlike a concrete word kind, this argument already owns its runtime descriptor.
const VIEW: u32 = u32::MAX;

/// Obtain a retained snapshot of the namespace binding, independently of later writes.
#[unsafe(no_mangle)]
pub extern "C" fn imre_builtins_last() -> *const Object {
    LAST.with_borrow(|slot| slot.clone()).map_or_else(
        || {
            Error::new(
                Kind::AttributeError,
                "module 'builtins' has no attribute '_'",
            )
            .returning(std::ptr::null())
        },
        crate::ObjectOwner::into_raw,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_builtins_has_last() -> u32 {
    LAST.with_borrow(|slot| u32::from(slot.is_some()))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_builtins_del_last() {
    if LAST.take().is_none() {
        Error::new(Kind::AttributeError, "'module' object has no attribute '_'").raise();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_builtins_clear() {
    drop(LAST.take());
}

/// # Safety
/// A concrete argument has exactly its stated layout; a VIEW points to a live snapshot.
unsafe fn snapshot(kind: u32, word: usize) -> crate::ObjectOwner {
    if kind == VIEW {
        // SAFETY: the caller roots this snapshot until ownership has been acquired.
        unsafe { crate::ObjectOwner::acquire_word(word) }
    } else {
        // SAFETY: the compiler supplies a rooted word with this concrete representation.
        unsafe { crate::ObjectOwner::from_raw(boxed_values::imre_box_value(kind, word)) }
    }
}

/// # Safety
/// The compiler roots the borrowed word with the stated concrete kind or VIEW.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_builtins_set_last(kind: u32, word: usize) {
    // SAFETY: acquire the incoming owner before releasing the displaced value.
    let value = unsafe { snapshot(kind, word) };
    drop(LAST.replace(Some(value)));
}

/// Run the original sys display hook. A failed write leaves the recursion guard at None.
///
/// # Safety
/// The caller roots a supported display value through the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sys_displayhook(kind: u32, word: usize) {
    // SAFETY: make an independent owner before touching the namespace binding.
    let view = unsafe { snapshot(kind, word) };
    // SAFETY: the view roots any concrete value or dynamic module member.
    if unsafe { exceptions::imre_system_exit_code_kind(crate::ObjectOwner::as_ptr(&view)) } == 0 {
        return;
    }
    // SAFETY: (0, 0) is the native representation of None.
    unsafe { imre_builtins_set_last(0, 0) };
    // SAFETY: repr returns an owned string or NULL with a pending Python error.
    let rendered =
        unsafe { exceptions::imre_exception_field_repr(crate::ObjectOwner::as_ptr(&view)) };
    if rendered.is_null() {
        return;
    }
    // SAFETY: the runtime returned one owned reference.
    let rendered = unsafe { StrOwner::from_raw(rendered) };
    if crate::sys::write_display_line(&rendered) {
        drop(LAST.replace(Some(view)));
    }
}
