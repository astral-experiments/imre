//! A logger owns one live list, and the streams it writes retain their original identities.
use crate::{ObjectOwner, StrOwner};

use super::super::{Error, Int, Object, Storage, Str, finish, object};
use super::state;

/// Write the whole string. Text streams return a character count; codec writers return None.
///
/// # Safety
/// Both the writable stream and the exact string stay live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_output_write(stream: *const Object, text: *const Str) -> usize {
    // SAFETY: the caller keeps the exact string alive for this write.
    let text = unsafe { &*text };
    // SAFETY: the caller roots the stream, and these sealed writes cannot reenter it.
    let result = unsafe {
        object(stream).0.with_mut(|s| match s {
            Storage::CodecWriter(writer) => writer
                .write(text, || {
                    // The codec retains the exact input only when constructing an encoding error.
                    Ok(StrOwner::acquire_word(text as *const Str as usize))
                })
                .map(|()| 0),
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

/// Return the underlying file for close, closed and descriptor operations.
/// A codec writer delegates these attributes to its wrapped binary stream.
///
/// # Safety
/// The caller roots a writable stream and receives its own reference to the file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_output_file(stream: *const Object) -> *const Object {
    // SAFETY: both alternatives retain a live file without calling back into the stream.
    let file = unsafe {
        object(stream).0.with(|s| match s {
            Storage::CodecWriter(writer) => writer.stream.clone(),
            Storage::File(_) => ObjectOwner::acquire_word(stream as usize),
            _ => unreachable!("checked text output stream"),
        })
    };
    ObjectOwner::into_raw(file)
}

/// # Safety
/// The caller roots the exact logger during the list read.
unsafe fn handler_list(logger: *const Object) -> ObjectOwner {
    // SAFETY: the caller retains the owner while the list acquires a reference.
    unsafe { state(logger) }.handlers.borrow().clone()
}

/// Allocate an empty list before replacing a live handler list.
pub(super) fn empty_handlers() -> Result<ObjectOwner, Error> {
    let words = super::super::Words::new(6);
    ObjectOwner::try_new(Object(super::super::StorageCell::new(words.into_storage())))
        .map_err(|_| Error::memory_error())
}

/// Remove configured handlers in place so held list references observe the change.
pub(super) fn remove_configured_handlers(logger: &super::Logger) {
    let list = logger.handlers.borrow().clone();
    // SAFETY: the retained list stays live through clear, outside the logger cell borrow.
    unsafe { super::super::imre_seq_clear(ObjectOwner::as_ptr(&list)) };
}

/// # Safety
/// The caller roots the logger; it receives an owned reference to the live list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_handlers(logger: *const Object) -> *const Object {
    // SAFETY: the caller supplies a live logger.
    ObjectOwner::into_raw(unsafe { handler_list(logger) })
}

/// # Safety
/// Both the exact logger and the handler record are live through this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_add_handler(logger: *const Object, handler: *const usize) {
    // SAFETY: fixed handler identity comparisons do not call back into either object.
    let result = unsafe {
        handler_list(logger).0.with_mut(|s| {
            let words = s.concrete_sequence_mut();
            let handler = handler as usize;
            if !words.values.contains(&handler) {
                words
                    .values
                    .try_reserve(1)
                    .map_err(|_| Error::allocation())?;
                words.values.push(words.kind.acquire(handler).into_word());
                words.update_header();
            }
            Ok(())
        })
    };
    finish(result, ());
}

/// # Safety
/// Both the exact logger and the handler record are live through removal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_remove_handler(logger: *const Object, handler: *const usize) {
    // SAFETY: the caller holds the logger and the handler during the identity search.
    let detached = unsafe {
        handler_list(logger).0.with_mut(|s| {
            let words = s.concrete_sequence_mut();
            let index = words
                .values
                .iter()
                .position(|&word| word == handler as usize)?;
            let word = words.values.remove(index);
            words.update_header();
            Some((words.kind, word))
        })
    };
    if let Some((kind, word)) = detached {
        // SAFETY: removal transferred one owned record. The list borrow has ended.
        unsafe { kind.release(word) };
    }
}
