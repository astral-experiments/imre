//! The default display hook owns the last displayed object in the builtins namespace.
use super::words::WordKind;
use super::{Error, Kind, Object, Storage, StorageCell, acquire_rc, exceptions, values};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static LAST: RefCell<Option<Rc<Object>>> = const { RefCell::new(None) };
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
        Rc::into_raw,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_builtins_import_last() -> *const Object {
    LAST.with_borrow(Clone::clone).map_or_else(
        || {
            Error::new(
                Kind::ImportError,
                "cannot import name '_' from 'builtins' (unknown location)",
            )
            .returning(std::ptr::null())
        },
        Rc::into_raw,
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
unsafe fn snapshot(kind: u32, word: usize) -> Rc<Object> {
    if kind == VIEW {
        // SAFETY: the caller roots this snapshot until ownership has been acquired.
        unsafe { acquire_rc(word) }
    } else {
        // SAFETY: the compiler supplies a rooted word with this concrete representation.
        let value = unsafe { WordKind::new(kind).acquire(word) };
        Rc::new(Object(StorageCell::new(Storage::ValueView(value))))
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
    // SAFETY: view retains the concrete word throughout formatting.
    let (kind, word) = unsafe { exceptions::value_word(Rc::as_ptr(&view)) };
    if kind.raw() == 0 {
        return;
    }
    // SAFETY: (0, 0) is the native representation of None.
    unsafe { imre_builtins_set_last(0, 0) };
    let mut rendered = String::new();
    // SAFETY: the compiler accepts only values with a native repr.
    unsafe { values::write_repr(&mut rendered, word, kind) };
    if crate::sys::write_display_line(&rendered) {
        drop(LAST.replace(Some(view)));
    }
}
