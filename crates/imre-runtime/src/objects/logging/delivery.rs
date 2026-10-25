//! A logger owns one live list, and the streams it writes retain their original identities.
use std::rc::Rc;

use super::super::{Int, Object, Storage, Str, finish, object};
use super::{LOGGERS, name};

/// Write the whole string. Text streams return a character count; codec writers return None.
///
/// # Safety
/// Both the writable stream and the exact string stay live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_output_write(stream: *const Object, text: *const Str) -> usize {
    // SAFETY: these sealed streams cannot call user code while mutably borrowed.
    let text = unsafe { &*text };
    let result = unsafe {
        object(stream).0.with_mut(|s| match s {
            Storage::CodecWriter(writer) => writer.write(text).map(|()| 0),
            Storage::File(file) => file
                .write(text)
                .map(|()| Int::from_usize(text.chars().count()).into_raw()),
            _ => unreachable!("checked text output stream"),
        })
    };
    finish(result, 0)
}

/// # Safety
/// The caller roots a sealed, writable text stream for this synchronous flush.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_output_flush(stream: *const Object) {
    // SAFETY: these concrete stream operations never reenter the borrowed stream.
    let result = unsafe {
        object(stream).0.with_mut(|s| match s {
            Storage::CodecWriter(writer) => writer.flush(),
            Storage::File(file) => file.flush(),
            _ => unreachable!("checked text output stream"),
        })
    };
    finish(result, ());
}

/// # Safety
/// The caller roots a registered logger for the lookup.
unsafe fn handler_list(logger: *const Object) -> Rc<Object> {
    // SAFETY: the supplied object remains live for its exact name read.
    let key = unsafe { name(logger) };
    LOGGERS.with_borrow(|logs| logs.entries[key.as_str()].handlers.clone())
}

/// # Safety
/// The caller roots the logger; it receives an owned reference to the live list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_handlers(logger: *const Object) -> *const Object {
    // SAFETY: the caller supplies a live logger.
    Rc::into_raw(unsafe { handler_list(logger) })
}

/// # Safety
/// Both the exact logger and the handler record are live through this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_add_handler(logger: *const Object, handler: *const usize) {
    // SAFETY: fixed handler identity comparisons do not call back into either object.
    unsafe {
        handler_list(logger).0.with_mut(|s| {
            let words = s.sequence_mut();
            let handler = handler as usize;
            if !words.values.contains(&handler) {
                words.values.push(words.kind.acquire(handler).into_word());
                words.update_header();
            }
        })
    }
}

/// # Safety
/// Both the exact logger and the handler record are live through removal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_remove_handler(logger: *const Object, handler: *const usize) {
    // SAFETY: the caller retains its own record owner while the list releases its copy.
    unsafe {
        handler_list(logger).0.with_mut(|s| {
            let words = s.sequence_mut();
            if let Some(index) = words
                .values
                .iter()
                .position(|&word| word == handler as usize)
            {
                let removed = words.values.remove(index);
                words.update_header();
                words.kind.release(removed);
            }
        })
    }
}
