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
/// until `relar_args_clear`. A later activation may initialize after clearing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_args_init(argc: i32, argv: *const *const c_char) {
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

/// The Python interpreter for explicitly launched Python helpers. Preserve
/// symlinks so a selected virtual environment keeps its own interpreter path.
#[unsafe(no_mangle)]
pub extern "C" fn relar_sys_executable() -> *const Str {
    use rustix::fs::{Access, access};
    let interpreter = std::env::var_os("RELAR_PYTHON")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .map(|directory| directory.join("python3"))
                .find(|candidate| candidate.is_file() && access(candidate, Access::EXEC_OK).is_ok())
        });
    let text = interpreter
        .and_then(|path| std::path::absolute(path).ok())
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or_default();
    Rc::into_raw(Rc::new(Str::from(text)))
}

/// # Safety
/// The argument is a borrowed, live Python integer, or zero for no exit status.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_sys_exit(code: usize) {
    if code == 0 {
        Error::new(Kind::SystemExit, "").raise();
        return;
    }
    // SAFETY: the caller owns the exact integer throughout conversion.
    let code = unsafe { super::Int::borrow(code) };
    Error::new(Kind::SystemExit, code.big().to_string()).raise();
}

/// Returns an owned reference to the same mutable list on every successful call.
/// Strict UTF-8 failure returns null with a pending UnicodeDecodeError. Pending
/// state must be clear; init must already have captured this main activation.
#[unsafe(no_mangle)]
pub extern "C" fn relar_sys_argv() -> *const Object {
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
pub extern "C" fn relar_args_clear() {
    let previous = ARGV.with(RefCell::take);
    ARGUMENTS.set(None);
    drop(previous);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    use super::super::exceptions::{
        relar_error_pending, relar_error_set, relar_error_take, relar_exception_matches,
        relar_exception_new,
    };
    use super::super::{Int, relar_object_release, relar_seq_append, relar_seq_get, relar_seq_len};

    #[test]
    fn argv_aliases_share_mutation_and_outlive_the_runtime_root() {
        let strings = ["program", "", "é🦀"].map(|text| CString::new(text).unwrap());
        let pointers = strings.each_ref().map(|text| text.as_ptr());
        // SAFETY: all pointers and their terminated bytes outlive clear below;
        // returned handles are owned and used with their declared representations.
        unsafe {
            relar_args_init(pointers.len() as i32, pointers.as_ptr());
            let first = Rc::from_raw(relar_sys_argv());
            let alias = Rc::from_raw(relar_sys_argv());
            assert!(Rc::ptr_eq(&first, &alias));
            assert_eq!(Int(relar_seq_len(Rc::as_ptr(&first))).as_small(), Some(3));
            let unicode = Rc::from_raw(
                relar_seq_get(Rc::as_ptr(&first), Int::small(2).unwrap().0) as *const Str
            );
            assert_eq!(unicode.as_str(), "é🦀");
            let added = Rc::new(Str::new("new argument"));
            relar_seq_append(Rc::as_ptr(&first), Rc::as_ptr(&added) as usize, 3);
            assert_eq!(Int(relar_seq_len(Rc::as_ptr(&alias))).as_small(), Some(4));
            let weak = Rc::downgrade(&first);
            let pending = relar_exception_new(Kind::RuntimeError as u32, std::ptr::null());
            relar_error_set(pending);
            relar_args_clear();
            assert_eq!(relar_error_take(), pending);
            relar_object_release(pending);
            relar_object_release(pending);
            drop(first);
            assert_eq!(Int(relar_seq_len(Rc::as_ptr(&alias))).as_small(), Some(4));
            drop(alias);
            assert!(weak.upgrade().is_none());
        }
        assert_eq!(relar_error_pending(), 0);
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
            relar_args_init(pointers.len() as i32, pointers.as_ptr());
            for _ in 0..2 {
                assert!(relar_sys_argv().is_null());
                let error = relar_error_take();
                assert_eq!(
                    relar_exception_matches(error, Kind::UnicodeDecodeError as u32),
                    1
                );
                relar_object_release(error);
            }
            relar_args_clear();
            relar_args_init(0, std::ptr::null());
            let empty = relar_sys_argv();
            assert_eq!(Int(relar_seq_len(empty)).as_small(), Some(0));
            relar_object_release(empty);
            relar_args_clear();
        }
        assert_eq!(relar_error_pending(), 0);
    }
}
