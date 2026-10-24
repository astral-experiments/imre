//! C-main argument capture with one lazy, shared Python-visible list.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_char};
use std::rc::Rc;

use super::exceptions::{Error, Kind, Result, finish};
use super::{Object, Str, Words, fail};

#[derive(Clone, Copy)]
struct Arguments {
    count: usize,
    values: *const *const c_char,
}

thread_local! {
    static ARGUMENTS: Cell<Option<Arguments>> = const { Cell::new(None) };
    static ARGV: RefCell<Option<Rc<Object>>> = const { RefCell::new(None) };
}

/// Captures argv without decoding or allocating. Generated main calls this only
/// when its closed program can access sys.argv.
///
/// # Safety
/// Call once before argv access on this execution thread. `argc` is nonnegative;
/// all `argc` entries must remain readable, non-null, NUL-terminated C strings
/// until `imre_args_clear`. A later activation may initialize after clearing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_args_init(argc: i32, argv: *const *const c_char) {
    if argc < 0 || (argc != 0 && argv.is_null()) || ARGUMENTS.get().is_some() {
        fail("SystemError", "invalid argument initialization");
    }
    ARGUMENTS.set(Some(Arguments {
        count: argc as usize,
        values: argv,
    }));
}

/// Decodes all inputs before publishing a list, so a failed access can be retried.
fn decode(arguments: Arguments) -> Result<Rc<Object>> {
    let mut values = Words::new(3);
    values
        .values
        .try_reserve_exact(arguments.count)
        .unwrap_or_else(|_| fail("MemoryError", "argument list allocation failed"));
    for index in 0..arguments.count {
        // SAFETY: initialization guarantees this entry remains readable and
        // NUL-terminated until clear; decoding invokes no callbacks.
        let value = unsafe { CStr::from_ptr(*arguments.values.add(index)) };
        let text = value.to_str().map_err(|error| {
            Error::new(
                Kind::UnicodeDecodeError,
                format!(
                    "invalid UTF-8 in argument {index} at byte {}",
                    error.valid_up_to()
                ),
            )
        })?;
        values
            .values
            .push(Rc::into_raw(Rc::new(Str::new(text))) as usize);
    }
    Ok(values.into_object())
}

/// Returns an owned reference to the same mutable list on every successful call.
/// Strict UTF-8 failure returns null with a pending UnicodeDecodeError. Pending
/// state must be clear; init must already have captured this main activation.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_argv() -> *const Object {
    if let Some(value) = ARGV.with(|argv| argv.borrow().clone()) {
        return Rc::into_raw(value);
    }
    let arguments = ARGUMENTS
        .get()
        .unwrap_or_else(|| fail("SystemError", "arguments were not initialized"));
    let result = decode(arguments).map(|value| {
        let previous = ARGV.with(|argv| argv.replace(Some(value.clone())));
        drop(previous);
        Rc::into_raw(value)
    });
    finish(result, std::ptr::null())
}

/// Releases the runtime's list root and captured pointers on either main exit.
/// Existing aliases remain valid. Cleanup is infallible and preserves pending.
#[unsafe(no_mangle)]
pub extern "C" fn imre_args_clear() {
    let previous = ARGV.with(RefCell::take);
    ARGUMENTS.set(None);
    drop(previous);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    use super::super::exceptions::{
        imre_error_pending, imre_error_set, imre_error_take, imre_exception_matches,
        imre_exception_new,
    };
    use super::super::{Int, imre_object_release, imre_seq_append, imre_seq_get, imre_seq_len};

    #[test]
    fn argv_aliases_share_mutation_and_outlive_the_runtime_root() {
        let strings = ["program", "", "é🦀"].map(|text| CString::new(text).unwrap());
        let pointers = strings.each_ref().map(|text| text.as_ptr());
        // SAFETY: all pointers and their terminated bytes outlive clear below;
        // returned handles are owned and used with their declared representations.
        unsafe {
            imre_args_init(pointers.len() as i32, pointers.as_ptr());
            let first = Rc::from_raw(imre_sys_argv());
            let alias = Rc::from_raw(imre_sys_argv());
            assert!(Rc::ptr_eq(&first, &alias));
            assert_eq!(Int(imre_seq_len(Rc::as_ptr(&first))).as_small(), Some(3));
            let unicode = Rc::from_raw(
                imre_seq_get(Rc::as_ptr(&first), Int::small(2).unwrap().0) as *const Str
            );
            assert_eq!(unicode.as_str(), "é🦀");
            let added = Rc::new(Str::new("new argument"));
            imre_seq_append(Rc::as_ptr(&first), Rc::as_ptr(&added) as usize, 3);
            assert_eq!(Int(imre_seq_len(Rc::as_ptr(&alias))).as_small(), Some(4));
            let weak = Rc::downgrade(&first);
            let pending = imre_exception_new(Kind::RuntimeError as u32, std::ptr::null());
            imre_error_set(pending);
            imre_args_clear();
            assert_eq!(imre_error_take(), pending);
            imre_object_release(pending);
            imre_object_release(pending);
            drop(first);
            assert_eq!(Int(imre_seq_len(Rc::as_ptr(&alias))).as_small(), Some(4));
            drop(alias);
            assert!(weak.upgrade().is_none());
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn invalid_argv_never_publishes_partial_data_and_retries_cleanly() {
        let strings = [
            CString::new("valid").unwrap(),
            CString::new([0xff]).unwrap(),
        ];
        let pointers = strings.each_ref().map(|text| text.as_ptr());
        // SAFETY: argument storage is live until clear; each failed access returns
        // no object, and its pending exception is consumed before the next call.
        unsafe {
            imre_args_init(pointers.len() as i32, pointers.as_ptr());
            for _ in 0..2 {
                assert!(imre_sys_argv().is_null());
                let error = imre_error_take();
                assert_eq!(
                    imre_exception_matches(error, Kind::UnicodeDecodeError as u32),
                    1
                );
                imre_object_release(error);
            }
            imre_args_clear();
            imre_args_init(0, std::ptr::null());
            let empty = imre_sys_argv();
            assert_eq!(Int(imre_seq_len(empty)).as_small(), Some(0));
            imre_object_release(empty);
            imre_args_clear();
        }
        assert_eq!(imre_error_pending(), 0);
    }
}
