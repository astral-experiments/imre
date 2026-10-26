//! Per-instance replacements for Popen.poll live on the Python owner thread.
use super::*;
use crate::objects::exceptions::imre_error_pending;
use crate::objects::words::NULLABLE;

#[derive(Clone)]
pub(super) struct Override {
    callable: OwnedWord,
    returns_none: bool,
}

/// The layout emitted for an ordinary native callable, including bound methods.
#[repr(C)]
struct Callable<T> {
    call: unsafe extern "C" fn(*const ()) -> T,
    context: *const (),
}

impl Override {
    /// Invoke outside both the Popen storage borrow and the child mutex: user
    /// code can change returncode, delete this override, or raise an exception.
    pub(super) fn invoke(&self) -> usize {
        // SAFETY: the compiler checked the zero-argument signature on assignment.
        // None functions return i8; int and optional int functions return an owned
        // integer word. The retained callable roots its context throughout the call.
        unsafe {
            if self.returns_none {
                let callback = &*(self.callable.word as *const Callable<u8>);
                (callback.call)(callback.context);
                0
            } else {
                let callback = &*(self.callable.word as *const Callable<usize>);
                (callback.call)(callback.context)
            }
        }
    }
}

/// # Safety
/// The owner thread roots both arguments. The callable has the zero-argument
/// return ABI specified by returns_none (i8 None or a possibly-null integer).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_set_poll(
    handle: *const Object,
    callable: *const (),
    returns_none: bool,
) {
    // SAFETY: acquire the callable before replacing its previous owner.
    let callable = unsafe { WordKind::new(6).acquire(callable as usize) };
    let replacement = Override {
        callable,
        returns_none,
    };
    // SAFETY: release the old callback only after ending the storage borrow.
    drop(unsafe { with_process(handle, |p| p.poll_override.replace(replacement)) });
}

/// # Safety
/// The caller roots this Popen on its owner thread; deleting an inherited
/// method without an instance override raises AttributeError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_del_poll(handle: *const Object) {
    // SAFETY: take ownership before dropping the callback and its context.
    if unsafe { with_process(handle, |p| p.poll_override.take()) }.is_none() {
        Error::new(
            Kind::AttributeError,
            "'Popen' object has no attribute 'poll'",
        )
        .returning(());
    }
}

/// # Safety
/// The owner thread roots this Popen and signal throughout the callback. A
/// false result selects the ordinary synchronized poll-and-signal operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_signal_override(handle: *const Object, signal: usize) -> bool {
    // SAFETY: no reference into Popen escapes the storage borrow.
    let Some(callback) = (unsafe { with_process(handle, |p| p.poll_override.clone()) }) else {
        return false;
    };
    let word = callback.invoke();
    // SAFETY: send_signal discards the callback's owned int or None.
    unsafe { WordKind::new(2 | NULLABLE).release(word) };
    if imre_error_pending() == 0 {
        // SAFETY: the caller continues to own handle and signal. The callback
        // may have changed returncode, which signal_after_poll reads under lock.
        let result =
            unsafe { (*imre_popen_control(handle)).signal_after_poll(|| signal_number(signal)) };
        finish(result, ());
    }
    true
}
