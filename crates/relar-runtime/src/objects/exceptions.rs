//! Explicit language errors; no Rust unwind crosses the generated-code ABI.
//!
//! Pending and dynamically active errors own independent references. Their
//! Explicit causes form an immutable chain: the compiler attaches them only to
//! freshly constructed, unescaped exceptions. No traceback or mutable exception
//! attributes can introduce ownership cycles.

use std::cell::{Cell, RefCell};
use std::io::{self, Write};
use std::rc::Rc;

use ruff_python_literal::escape::{Escape, UnicodeEscape};

use super::{Object, Storage, StorageCell, Str, acquire_rc, fail, object};

/// Stable ABI identifiers for the supported builtin exception hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum Kind {
    BaseException = 0,
    Exception = 1,
    ArithmeticError = 2,
    ZeroDivisionError = 3,
    OverflowError = 4,
    LookupError = 5,
    IndexError = 6,
    KeyError = 7,
    ValueError = 8,
    AssertionError = 9,
    RuntimeError = 10,
    EOFError = 11,
    OSError = 12,
    TypeError = 13,
    AttributeError = 14,
    MemoryError = 15,
    SystemError = 16,
    UnicodeError = 17,
    UnicodeDecodeError = 18,
    UnsupportedOperation = 19,
    FileNotFoundError = 20,
    PermissionError = 21,
    IsADirectoryError = 22,
    NotADirectoryError = 23,
    FileExistsError = 24,
    SubprocessError = 25,
    CalledProcessError = 26,
    TimeoutExpired = 27,
    BadGzipFile = 28,
    ZlibError = 29,
    TomlDecodeError = 30,
    JsonDecodeError = 31,
    SystemExit = 32,
}

impl Kind {
    fn from_raw(kind: u32) -> Self {
        match kind {
            0 => Self::BaseException,
            1 => Self::Exception,
            2 => Self::ArithmeticError,
            3 => Self::ZeroDivisionError,
            4 => Self::OverflowError,
            5 => Self::LookupError,
            6 => Self::IndexError,
            7 => Self::KeyError,
            8 => Self::ValueError,
            9 => Self::AssertionError,
            10 => Self::RuntimeError,
            11 => Self::EOFError,
            12 => Self::OSError,
            13 => Self::TypeError,
            14 => Self::AttributeError,
            15 => Self::MemoryError,
            16 => Self::SystemError,
            17 => Self::UnicodeError,
            18 => Self::UnicodeDecodeError,
            19 => Self::UnsupportedOperation,
            20 => Self::FileNotFoundError,
            21 => Self::PermissionError,
            22 => Self::IsADirectoryError,
            23 => Self::NotADirectoryError,
            24 => Self::FileExistsError,
            25 => Self::SubprocessError,
            26 => Self::CalledProcessError,
            27 => Self::TimeoutExpired,
            28 => Self::BadGzipFile,
            29 => Self::ZlibError,
            30 => Self::TomlDecodeError,
            31 => Self::JsonDecodeError,
            32 => Self::SystemExit,
            _ => fail("SystemError", "invalid exception kind"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::BaseException => "BaseException",
            Self::Exception => "Exception",
            Self::ArithmeticError => "ArithmeticError",
            Self::ZeroDivisionError => "ZeroDivisionError",
            Self::OverflowError => "OverflowError",
            Self::LookupError => "LookupError",
            Self::IndexError => "IndexError",
            Self::KeyError => "KeyError",
            Self::ValueError => "ValueError",
            Self::AssertionError => "AssertionError",
            Self::RuntimeError => "RuntimeError",
            Self::EOFError => "EOFError",
            Self::OSError => "OSError",
            Self::TypeError => "TypeError",
            Self::AttributeError => "AttributeError",
            Self::MemoryError => "MemoryError",
            Self::SystemError => "SystemError",
            Self::UnicodeError => "UnicodeError",
            Self::UnicodeDecodeError => "UnicodeDecodeError",
            Self::UnsupportedOperation => "UnsupportedOperation",
            Self::FileNotFoundError => "FileNotFoundError",
            Self::PermissionError => "PermissionError",
            Self::IsADirectoryError => "IsADirectoryError",
            Self::NotADirectoryError => "NotADirectoryError",
            Self::FileExistsError => "FileExistsError",
            Self::SubprocessError => "SubprocessError",
            Self::CalledProcessError => "CalledProcessError",
            Self::TimeoutExpired => "TimeoutExpired",
            Self::BadGzipFile => "BadGzipFile",
            Self::ZlibError => "error",
            Self::TomlDecodeError => "TOMLDecodeError",
            Self::JsonDecodeError => "JSONDecodeError",
            Self::SystemExit => "SystemExit",
        }
    }

    fn matches(self, expected: Self) -> bool {
        match expected {
            Self::BaseException => true,
            Self::Exception => !matches!(self, Self::BaseException | Self::SystemExit),
            Self::ArithmeticError => matches!(
                self,
                Self::ArithmeticError | Self::ZeroDivisionError | Self::OverflowError
            ),
            Self::LookupError => {
                matches!(self, Self::LookupError | Self::IndexError | Self::KeyError)
            }
            Self::ValueError => matches!(
                self,
                Self::ValueError
                    | Self::JsonDecodeError
                    | Self::TomlDecodeError
                    | Self::UnicodeError
                    | Self::UnicodeDecodeError
                    | Self::UnsupportedOperation
            ),
            Self::UnicodeError => matches!(self, Self::UnicodeError | Self::UnicodeDecodeError),
            Self::SubprocessError => {
                matches!(
                    self,
                    Self::SubprocessError | Self::CalledProcessError | Self::TimeoutExpired
                )
            }
            Self::OSError => matches!(
                self,
                Self::OSError
                    | Self::UnsupportedOperation
                    | Self::FileNotFoundError
                    | Self::PermissionError
                    | Self::IsADirectoryError
                    | Self::NotADirectoryError
                    | Self::FileExistsError
                    | Self::BadGzipFile
            ),
            _ => self == expected,
        }
    }
}

pub(super) struct Exception {
    kind: Kind,
    message: Rc<Str>,
    cause: Option<Rc<Object>>,
    suppress_context: bool,
    process: Option<Box<super::processes::ProcessError>>,
    timeout: Option<Box<super::processes::TimeoutError>>,
}

impl Drop for Exception {
    /// Release uniquely owned cause tails iteratively, including very long chains.
    fn drop(&mut self) {
        let mut next = self.cause.take();
        while let Some(cause) = next {
            let Ok(object) = Rc::try_unwrap(cause) else {
                break;
            };
            let Storage::Exception(mut exception) = object.0.into_inner() else {
                unreachable!("exception causes have exact exception storage");
            };
            next = exception.cause.take();
        }
    }
}

/// An owned error returned from a storage access before pending state is changed.
pub(crate) struct Error {
    kind: Kind,
    message: Str,
}

impl Error {
    pub(crate) fn new(kind: Kind, message: impl Into<Str>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn into_parts(self) -> (&'static str, Str) {
        (self.kind.name(), self.message)
    }

    pub(crate) fn index() -> Self {
        Self::new(Kind::IndexError, "list index out of range")
    }

    /// Installs this error only after any runtime storage access has ended.
    /// Keep exception allocation and cleanup out of successful access paths.
    #[cold]
    #[inline(never)]
    pub(crate) fn raise(self) {
        replace_pending(Some(new_exception(self.kind, Rc::new(self.message))));
    }

    /// Returns a failure sentinel after installing the owned exception.
    #[cold]
    #[inline(never)]
    pub(crate) fn returning<T>(self, sentinel: T) -> T {
        self.raise();
        sentinel
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// Converts an internal Result at the ABI boundary, after storage borrows end.
pub(crate) fn finish<T>(result: Result<T>, sentinel: T) -> T {
    result.unwrap_or_else(|error| error.returning(sentinel))
}

thread_local! {
    static PENDING: RefCell<Option<Rc<Object>>> = const { RefCell::new(None) };
    static ACTIVE: RefCell<Option<Rc<Object>>> = const { RefCell::new(None) };
    // The non-Drop mirror lets successful calls query one TLS byte without
    // initializing or checking the owning cell's destructor/borrow state.
    // Only replace_pending/take_pending mutate the owner; both synchronize this
    // flag before dropping displaced owners. The single-thread, no-callback
    // contract makes the brief interval between owner and flag writes unobservable.
    static PENDING_PRESENT: Cell<bool> = const { Cell::new(false) };
}

fn new_exception(kind: Kind, message: Rc<Str>) -> Rc<Object> {
    Rc::new(Object(StorageCell::new(Storage::Exception(Exception {
        kind,
        message,
        cause: None,
        suppress_context: false,
        process: None,
        timeout: None,
    }))))
}

/// Publishes a completed child's rich failure after all input storage borrows end.
#[cold]
pub(super) fn raise_process(error: super::processes::ProcessError) {
    replace_pending(Some(Rc::new(Object(StorageCell::new(Storage::Exception(
        Exception {
            kind: Kind::CalledProcessError,
            message: Rc::new(Str::new("")),
            cause: None,
            suppress_context: false,
            process: Some(Box::new(error)),
            timeout: None,
        },
    ))))));
}

/// Publishes the immutable deadline snapshot after the child has been reaped.
#[cold]
pub(super) fn raise_timeout(error: super::processes::TimeoutError) {
    replace_pending(Some(Rc::new(Object(StorageCell::new(Storage::Exception(
        Exception {
            kind: Kind::TimeoutExpired,
            message: Rc::new(Str::new("")),
            cause: None,
            suppress_context: false,
            process: None,
            timeout: Some(Box::new(error)),
        },
    ))))));
}

/// Replaces the pending owner without dropping it during a TLS mutable borrow.
fn replace_pending(value: Option<Rc<Object>>) {
    let present = value.is_some();
    let previous = PENDING.with(|pending| pending.replace(value));
    PENDING_PRESENT.set(present);
    drop(previous);
}

/// Clears presence before transferring the owner, without dropping under a TLS borrow.
fn take_pending() -> Option<Rc<Object>> {
    let pending = PENDING.with(RefCell::take);
    PENDING_PRESENT.set(false);
    pending
}

/// Uses Ruff's Python repr rules and its pinned Unicode printable categories.
pub(crate) fn string_repr(value: &str) -> Str {
    let escaped = UnicodeEscape::new_repr(value);
    let mut result = String::new();
    let quote = escaped.layout().quote.as_char();
    result.push(quote);
    escaped
        .write_body(&mut result)
        .expect("writing to a String cannot fail");
    result.push(quote);
    Str::from(result)
}

/// Acquires nullable exception input without retaining a storage reference.
unsafe fn acquire_exception(value: *const Object) -> Option<Rc<Object>> {
    if value.is_null() {
        return None;
    }
    // SAFETY: the caller roots the object; only immutable kind metadata escapes.
    unsafe { object(value).0.with(Storage::exception_kind) };
    // SAFETY: the shape-checked handle remains rooted throughout acquisition.
    Some(unsafe { acquire_rc(value as usize) })
}

impl Storage {
    fn exception(&self) -> &Exception {
        match self {
            Self::Exception(exception) => exception,
            _ => fail("SystemError", "expected an exception object"),
        }
    }

    fn exception_kind(&self) -> Kind {
        self.exception().kind
    }

    fn exception_message(&self) -> Rc<Str> {
        let exception = self.exception();
        if let Some(timeout) = &exception.timeout {
            return Rc::new(timeout.message());
        }
        exception.process.as_ref().map_or_else(
            || exception.message.clone(),
            |process| Rc::new(process.message()),
        )
    }
}

/// Acquires one sealed process-error field: cmd, returncode, stdout, or stderr;
/// field 4 is the output alias of stdout and preserves its attribute error name.
/// An ordinary exception receiver raises AttributeError instead of exposing storage.
///
/// # Safety
/// `value` is a rooted exception, `field` is 0 through 4, and pending is clear.
/// Success returns one owned word of kind 5, 2, 3, 3, or 3 respectively. Check pending
/// before interpreting zero, then release the result using that exact kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_subprocess_error_field(value: *const Object, field: u32) -> usize {
    if field > 4 {
        fail("SystemError", "invalid process exception field");
    }
    // SAFETY: only an acquired word or owned error leaves this rooted storage read.
    let result = unsafe {
        object(value).0.with(|storage| {
            let exception = storage.exception();
            if field == 0
                && let Some(timeout) = &exception.timeout
            {
                return Ok(timeout.field(0));
            }
            exception.process.as_ref().map_or_else(
                || {
                    let name = ["cmd", "returncode", "stdout", "stderr", "output"][field as usize];
                    Err(Error::new(
                        Kind::AttributeError,
                        format!(
                            "'{}' object has no attribute '{name}'",
                            exception.kind.name()
                        ),
                    ))
                },
                |process| Ok(process.field(field)),
            )
        })
    };
    finish(result, 0)
}

/// Acquires a field from a proved TimeoutExpired view without changing its type.
///
/// # Safety
/// `value` is a rooted TimeoutExpired and `field` is 0 through 4. The returned
/// owned word has kind 5, integer or float, 264, 264, or 264 respectively.
/// A seconds read requires a program-wide proof of its numeric kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_subprocess_timeout_field(value: *const Object, field: u32) -> usize {
    // SAFETY: the checker establishes an exact TimeoutExpired before projection.
    unsafe {
        object(value).0.with(|storage| {
            storage
                .exception()
                .timeout
                .as_ref()
                .unwrap_or_else(|| fail("SystemError", "expected TimeoutExpired storage"))
                .field(field)
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_error_pending() -> u32 {
    u32::from(PENDING_PRESENT.get())
}

/// Transfers the pending reference to the caller and clears pending state.
#[unsafe(no_mangle)]
pub extern "C" fn relar_error_take() -> *const Object {
    take_pending().map_or(std::ptr::null(), Rc::into_raw)
}

/// # Safety
/// `value` is a rooted exception or null. This borrows/acquires it; null clears
/// pending state. Cleanup and error access remain valid while an error is pending.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_error_set(value: *const Object) {
    // SAFETY: the caller supplies a rooted exception or null.
    replace_pending(unsafe { acquire_exception(value) });
}

/// Constructs an owned builtin exception from zero or one string argument.
/// A null message means zero arguments; KeyError's one-string form uses repr.
///
/// # Safety
/// `kind` is a supported builtin kind. `message` is a rooted string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_exception_new(kind: u32, message: *const Str) -> *const Object {
    let kind = Kind::from_raw(kind);
    let message = if message.is_null() {
        Rc::new(Str::new(""))
    } else if kind == Kind::KeyError {
        // SAFETY: the caller roots the immutable string argument.
        Rc::new(string_repr(unsafe { &*message }))
    } else {
        // SAFETY: the non-null argument is a rooted Rc<Str>.
        unsafe { acquire_rc(message as usize) }
    };
    Rc::into_raw(new_exception(kind, message))
}

/// # Safety
/// `value` is a rooted exception or null; acquires one additional reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_exception_retain(value: *const Object) {
    // SAFETY: exceptions use the ordinary Object ownership representation.
    unsafe { super::relar_object_retain(value) };
}

/// # Safety
/// Consumes one owned exception reference, or accepts null during cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_exception_release(value: *const Object) {
    // SAFETY: exceptions use the ordinary Object ownership representation.
    unsafe { super::relar_object_release(value) };
}

/// # Safety
/// `value` is a rooted exception and `expected_kind` is a supported builtin kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_exception_matches(value: *const Object, expected_kind: u32) -> u32 {
    let expected = Kind::from_raw(expected_kind);
    // SAFETY: the rooted exception is read without reentry or escaped references.
    u32::from(unsafe {
        object(value)
            .0
            .with(|storage| storage.exception_kind().matches(expected))
    })
}

/// Returns an owned string containing Python's str(exception) representation.
///
/// # Safety
/// `value` must be a rooted exception object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_exception_message(value: *const Object) -> *const Str {
    // SAFETY: the clone owns its immutable message independently of the exception.
    Rc::into_raw(unsafe { object(value).0.with(Storage::exception_message) })
}

/// Attaches an explicit cause before publishing a fresh exception and returns
/// an additional owned reference to that same exception. Null represents `from None`.
///
/// # Safety
/// `value` is a fresh, unescaped exception, and `cause` is a rooted exception or
/// null. The cause cannot contain `value`; neither input is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_exception_with_cause(
    value: *const Object,
    cause: *const Object,
) -> *const Object {
    // SAFETY: the caller roots the nullable exception cause.
    let cause = unsafe { acquire_exception(cause) };
    // SAFETY: the fresh object cannot alias a cause, and no storage borrow escapes.
    unsafe {
        object(value).0.with_mut(|storage| {
            let Storage::Exception(exception) = storage else {
                fail("SystemError", "expected an exception object");
            };
            debug_assert!(exception.cause.is_none() && !exception.suppress_context);
            exception.cause = cause;
            exception.suppress_context = true;
        });
        relar_exception_retain(value);
    }
    value
}

/// Returns an independently owned explicit cause, or null when absent.
///
/// # Safety
/// `value` is a rooted exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_exception_cause(value: *const Object) -> *const Object {
    // SAFETY: the returned clone owns the cause after storage access ends.
    unsafe {
        object(value)
            .0
            .with(|storage| storage.exception().cause.clone())
            .map_or(std::ptr::null(), Rc::into_raw)
    }
}

/// # Safety
/// `value` is a rooted exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_exception_suppress_context(value: *const Object) -> u32 {
    // SAFETY: only immutable metadata escapes storage access.
    unsafe {
        object(value)
            .0
            .with(|storage| u32::from(storage.exception().suppress_context))
    }
}

/// Acquires the dynamically active exception, or returns null outside a handler.
#[unsafe(no_mangle)]
pub extern "C" fn relar_exception_current() -> *const Object {
    ACTIVE.with(|active| {
        active
            .borrow()
            .clone()
            .map_or(std::ptr::null(), Rc::into_raw)
    })
}

/// Borrows/acquires the incoming active exception and transfers the previous
/// owner to the caller. Nested handlers save and restore that owner explicitly.
///
/// # Safety
/// `value` is a rooted exception or null; the returned reference must be released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_exception_swap_active(value: *const Object) -> *const Object {
    // SAFETY: the caller roots the nullable exception before TLS replacement.
    let new = unsafe { acquire_exception(value) };
    ACTIVE.with(|active| active.replace(new).map_or(std::ptr::null(), Rc::into_raw))
}

/// # Safety
/// `value` must be a rooted, non-null exception. Pending state acquires a reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_exception_raise(value: *const Object) {
    if value.is_null() {
        fail("SystemError", "cannot raise a null exception");
    }
    // SAFETY: the caller supplies a rooted exception.
    unsafe { relar_error_set(value) };
}

/// Bare raise observes the dynamically active handler, including across calls.
#[unsafe(no_mangle)]
pub extern "C" fn relar_exception_reraise() {
    let active = ACTIVE.with(|active| active.borrow().clone());
    if let Some(active) = active {
        replace_pending(Some(active));
    } else {
        Error::new(Kind::RuntimeError, "No active exception to reraise").raise();
    }
}

/// Reports and consumes the pending error after generated-code cleanup. Returning
/// the exit status lets the C entry point finish normally, including sanitizer
/// teardown. A failed best-effort stdout flush preserves the original error.
#[unsafe(no_mangle)]
pub extern "C" fn relar_error_unhandled() -> i32 {
    let pending = take_pending();
    let active = ACTIVE.with(RefCell::take);
    drop(active);
    let Some(pending) = pending else {
        return 0;
    };
    let mut chain = Vec::new();
    let mut current = Some(pending.clone());
    while let Some(exception) = current {
        // SAFETY: the current owner roots the exception; only owned data escapes.
        let (kind, message, cause) = unsafe {
            exception.0.with(|storage| {
                let exception = storage.exception();
                (
                    exception.kind,
                    storage.exception_message(),
                    exception.cause.clone(),
                )
            })
        };
        if chain.is_empty() && kind == Kind::SystemExit {
            let _ = crate::flush_stdout();
            if message.is_empty() {
                return 0;
            }
            return message.parse::<i64>().map_or_else(
                |_| {
                    eprintln!("OverflowError: Python int too large to convert to C long");
                    -1
                },
                |code| code as i32,
            );
        }
        chain.push((kind, message));
        current = cause;
    }
    drop(pending);
    let _ = crate::flush_stdout();
    let mut stderr = io::stderr().lock();
    for (index, (kind, message)) in chain.into_iter().rev().enumerate() {
        if index != 0 {
            let _ = writeln!(
                stderr,
                "\nThe above exception was the direct cause of the following exception:\n"
            );
        }
        if message.is_empty() {
            let _ = writeln!(stderr, "{}", kind.name());
        } else {
            let _ = writeln!(stderr, "{}: {message}", kind.name());
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::mappings::{
        relar_dict_delete, relar_dict_get, relar_dict_iter, relar_dict_new, relar_dict_set,
        relar_mapping_iter_next,
    };
    use crate::objects::{
        relar_seq_append, relar_seq_get_index, relar_seq_new, relar_seq_pop, relar_seq_set_index,
    };
    use crate::{
        Int, relar_int_binary, relar_int_parse, relar_list_append, relar_list_copy_index,
        relar_list_get_index, relar_list_new, relar_list_pop, relar_list_repeat,
    };
    use num_bigint::BigInt;

    fn take_error(kind: Kind) -> Rc<Object> {
        assert_eq!(relar_error_pending(), 1);
        let value = relar_error_take();
        assert!(!value.is_null());
        assert_eq!(relar_error_pending(), 0);
        // SAFETY: take transfers a non-null owned exception reference.
        let value = unsafe { Rc::from_raw(value) };
        // SAFETY: the owned exception is rooted and the expected kind is valid.
        let matches = unsafe { relar_exception_matches(Rc::as_ptr(&value), kind as u32) };
        assert_eq!(matches, 1);
        value
    }

    fn message(value: &Rc<Object>) -> Rc<Str> {
        // SAFETY: the rooted exception accessor transfers one string reference.
        unsafe { Rc::from_raw(relar_exception_message(Rc::as_ptr(value))) }
    }

    #[test]
    fn explicit_causes_preserve_identity_and_independent_ownership() {
        let original = new_exception(Kind::KeyError, Rc::new(Str::new("original")));
        let wrapped = new_exception(Kind::ValueError, Rc::new(Str::new("wrapped")));
        let weak = Rc::downgrade(&original);
        // SAFETY: wrapped is fresh; every borrowed input remains rooted and each
        // transferred reference is adopted immediately.
        unsafe {
            assert!(relar_exception_cause(Rc::as_ptr(&wrapped)).is_null());
            assert_eq!(relar_exception_suppress_context(Rc::as_ptr(&wrapped)), 0);
            let result = Rc::from_raw(relar_exception_with_cause(
                Rc::as_ptr(&wrapped),
                Rc::as_ptr(&original),
            ));
            assert!(Rc::ptr_eq(&result, &wrapped));
            assert_eq!(relar_exception_suppress_context(Rc::as_ptr(&wrapped)), 1);
            let cause = Rc::from_raw(relar_exception_cause(Rc::as_ptr(&wrapped)));
            assert!(Rc::ptr_eq(&cause, &original));
            drop((original, wrapped, result));
            assert_eq!(message(&cause).as_str(), "original");
            drop(cause);
            assert!(weak.upgrade().is_none());

            let suppressed = new_exception(Kind::ValueError, Rc::new(Str::new("hidden")));
            drop(Rc::from_raw(relar_exception_with_cause(
                Rc::as_ptr(&suppressed),
                std::ptr::null(),
            )));
            assert!(relar_exception_cause(Rc::as_ptr(&suppressed)).is_null());
            assert_eq!(relar_exception_suppress_context(Rc::as_ptr(&suppressed)), 1);
        }
    }

    #[test]
    fn deep_cause_chains_release_without_recursive_drop() {
        let message = Rc::new(Str::new("chain"));
        let mut tail = new_exception(Kind::ValueError, message.clone());
        let weak_first = Rc::downgrade(&tail);
        for _ in 0..100_000 {
            let next = new_exception(Kind::ValueError, message.clone());
            // SAFETY: next is fresh and tail remains rooted until attachment.
            unsafe {
                drop(Rc::from_raw(relar_exception_with_cause(
                    Rc::as_ptr(&next),
                    Rc::as_ptr(&tail),
                )));
            }
            tail = next;
        }
        drop(tail);
        assert!(weak_first.upgrade().is_none());
        assert_eq!(Rc::strong_count(&message), 1);
    }

    #[test]
    fn pending_and_active_errors_transfer_independent_owned_references() {
        let text = Rc::new(Str::new("outer"));
        let weak_text = Rc::downgrade(&text);
        // SAFETY: the argument is a rooted string; construction transfers an owner.
        let outer = unsafe {
            Rc::from_raw(relar_exception_new(
                Kind::ValueError as u32,
                Rc::as_ptr(&text),
            ))
        };
        let weak_outer = Rc::downgrade(&outer);
        drop(text);
        // SAFETY: all exception inputs are rooted and every transferred owner is
        // adopted or released below, including same-value pending replacement.
        unsafe {
            relar_exception_raise(Rc::as_ptr(&outer));
            relar_error_set(Rc::as_ptr(&outer));
            let pending = take_error(Kind::ValueError);
            assert!(Rc::ptr_eq(&outer, &pending));
            assert!(relar_exception_swap_active(Rc::as_ptr(&outer)).is_null());
            drop(pending);
            let inner = Rc::from_raw(relar_exception_new(
                Kind::IndexError as u32,
                std::ptr::null(),
            ));
            let previous = Rc::from_raw(relar_exception_swap_active(Rc::as_ptr(&inner)));
            assert!(Rc::ptr_eq(&outer, &previous));
            drop(outer);
            let current = Rc::from_raw(relar_exception_current());
            assert!(Rc::ptr_eq(&inner, &current));
            relar_exception_reraise();
            let raised = take_error(Kind::IndexError);
            assert!(Rc::ptr_eq(&inner, &raised));
            let displaced = Rc::from_raw(relar_exception_swap_active(Rc::as_ptr(&previous)));
            assert!(Rc::ptr_eq(&inner, &displaced));
            drop((inner, current, raised, displaced, previous));
            relar_exception_reraise();
            let raised = take_error(Kind::ValueError);
            assert_eq!(message(&raised).as_str(), "outer");
            drop(raised);
            relar_exception_release(relar_exception_swap_active(std::ptr::null()));
        }
        assert!(weak_outer.upgrade().is_none());
        assert!(weak_text.upgrade().is_none());
        assert!(relar_exception_current().is_null());
        relar_exception_reraise();
        assert_eq!(
            message(&take_error(Kind::RuntimeError)).as_str(),
            "No active exception to reraise"
        );
    }

    #[test]
    fn replacing_clearing_and_reporting_pending_errors_allow_later_recovery() {
        let first = new_exception(Kind::ValueError, Rc::new(Str::new("first")));
        let second = new_exception(Kind::IndexError, Rc::new(Str::new("second")));
        let weak_first = Rc::downgrade(&first);
        let weak_second = Rc::downgrade(&second);
        // SAFETY: inputs stay rooted through acquisition, then pending state
        // becomes their sole owner. Null explicitly clears that owner.
        unsafe {
            relar_error_set(Rc::as_ptr(&first));
            drop(first);
            assert_eq!(relar_error_pending(), 1);
            relar_error_set(Rc::as_ptr(&second));
            drop(second);
            assert!(weak_first.upgrade().is_none());
            assert_eq!(relar_error_pending(), 1);
            relar_error_set(std::ptr::null());
        }
        assert!(weak_second.upgrade().is_none());
        assert_eq!(relar_error_pending(), 0);
        assert!(relar_error_take().is_null());

        let reported = new_exception(Kind::RuntimeError, Rc::new(Str::new("reported test error")));
        let weak_reported = Rc::downgrade(&reported);
        // SAFETY: pending and active independently acquire this rooted exception.
        unsafe {
            relar_error_set(Rc::as_ptr(&reported));
            assert!(relar_exception_swap_active(Rc::as_ptr(&reported)).is_null());
        }
        drop(reported);
        assert_eq!(relar_error_unhandled(), 1);
        assert!(weak_reported.upgrade().is_none());
        assert_eq!(relar_error_pending(), 0);
        assert!(relar_exception_current().is_null());
        assert_eq!(relar_error_unhandled(), 0);
        for _ in 0..2 {
            relar_exception_reraise();
            assert_eq!(
                message(&take_error(Kind::RuntimeError)).as_str(),
                "No active exception to reraise"
            );
        }
    }

    #[test]
    fn builtin_hierarchy_and_keyerror_string_arguments_match_python() {
        for (actual, expected, matches) in [
            (Kind::ZeroDivisionError, Kind::ArithmeticError, true),
            (Kind::OverflowError, Kind::ArithmeticError, true),
            (Kind::IndexError, Kind::LookupError, true),
            (Kind::KeyError, Kind::LookupError, true),
            (Kind::ValueError, Kind::ArithmeticError, false),
            (Kind::SystemError, Kind::Exception, true),
            (Kind::Exception, Kind::BaseException, true),
            (Kind::BaseException, Kind::Exception, false),
        ] {
            // SAFETY: valid kinds and a null zero-argument message satisfy the ABI.
            unsafe {
                let value = Rc::from_raw(relar_exception_new(actual as u32, std::ptr::null()));
                assert_eq!(
                    relar_exception_matches(Rc::as_ptr(&value), expected as u32),
                    u32::from(matches)
                );
            }
        }
        for (argument, expected) in [
            (None, ""),
            (Some(""), "''"),
            (Some("a'b"), "\"a'b\""),
            (Some("a'\"b"), "'a\\'\"b'"),
            (Some("\n\t\r\\\0"), "'\\n\\t\\r\\\\\\x00'"),
            (
                Some("é e\u{301} 😀\u{a0}\u{2028}\u{e000}"),
                "'é e\u{301} 😀\\xa0\\u2028\\ue000'",
            ),
        ] {
            let argument = argument.map(|value| Rc::new(Str::new(value)));
            let pointer = argument.as_ref().map_or(std::ptr::null(), Rc::as_ptr);
            // SAFETY: the nullable argument remains rooted throughout construction.
            let error =
                unsafe { Rc::from_raw(relar_exception_new(Kind::KeyError as u32, pointer)) };
            drop(argument);
            assert_eq!(message(&error).as_str(), expected);
        }
    }

    #[test]
    fn caught_arithmetic_and_list_failures_preserve_owners_and_allow_recovery() {
        let big = Rc::new(BigInt::from(1_u8) << 120_u32);
        let weak = Rc::downgrade(&big);
        let integer = Int(Rc::into_raw(big) as usize);
        // SAFETY: the constructor transfers one owned list reference.
        let list = unsafe { Rc::from_raw(relar_list_new()) };
        let zero = Int::small(0).unwrap();
        // SAFETY: all integer/list inputs stay rooted, and each successful owned
        // result is adopted exactly once. Failure sentinels are never acquired.
        unsafe {
            assert_eq!(relar_int_binary(integer.0, zero.0, 3), 1);
            drop(take_error(Kind::ZeroDivisionError));
            assert_eq!(relar_int_parse(&Str::new("12__3")), 1);
            drop(take_error(Kind::ValueError));
            relar_list_append(Rc::as_ptr(&list), integer.0);
            drop(integer);
            assert_eq!(weak.strong_count(), 1);
            relar_list_copy_index(Rc::as_ptr(&list), 1, 0);
            drop(take_error(Kind::IndexError));
            assert_eq!(weak.strong_count(), 1);
            assert_eq!(relar_list_get_index(Rc::as_ptr(&list), 7), 0);
            drop(take_error(Kind::IndexError));
            let huge = Int::new(BigInt::from(1_u8) << 100_u32);
            assert!(relar_list_repeat(Rc::as_ptr(&list), huge.0).is_null());
            drop(take_error(Kind::OverflowError));
            let capacity = Int::new(BigInt::from(isize::MAX));
            assert!(relar_list_repeat(Rc::as_ptr(&list), capacity.0).is_null());
            drop(take_error(Kind::MemoryError));
            let popped = Int(relar_list_pop(Rc::as_ptr(&list)));
            assert_eq!(popped.big().as_ref(), &(BigInt::from(1_u8) << 120_u32));
            drop(popped);
            assert_eq!(relar_list_pop(Rc::as_ptr(&list)), 0);
            drop(take_error(Kind::IndexError));
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn failed_sequence_updates_release_staged_values_after_access() {
        let text = Rc::new(Str::new("staged"));
        let weak = Rc::downgrade(&text);
        // SAFETY: constructors transfer owners; all string/exception slots use
        // their declared representation and every getter's owner is adopted.
        unsafe {
            let sequence = Rc::from_raw(relar_seq_new(3));
            relar_seq_set_index(Rc::as_ptr(&sequence), 0, Rc::as_ptr(&text) as usize, 3);
            drop(take_error(Kind::IndexError));
            assert_eq!(Rc::strong_count(&text), 1);
            assert_eq!(relar_seq_pop(Rc::as_ptr(&sequence)), 0);
            drop(take_error(Kind::IndexError));
            drop(sequence);
            let sequence = Rc::from_raw(relar_seq_new(5));
            let exception = Rc::from_raw(relar_exception_new(
                Kind::Exception as u32,
                Rc::as_ptr(&text),
            ));
            let weak_exception = Rc::downgrade(&exception);
            relar_seq_append(Rc::as_ptr(&sequence), Rc::as_ptr(&exception) as usize, 5);
            drop(exception);
            let exception =
                Rc::from_raw(relar_seq_get_index(Rc::as_ptr(&sequence), 0) as *const Object);
            relar_exception_raise(Rc::as_ptr(&exception));
            drop((sequence, exception, text));
            let pending = take_error(Kind::Exception);
            assert_eq!(message(&pending).as_str(), "staged");
            drop(pending);
            assert!(weak_exception.upgrade().is_none());
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn mapping_errors_preserve_outputs_and_scalar_key_messages() {
        let string_key = Rc::new(Str::new("a'b\n"));
        let integer_key = Int::new(BigInt::from(1_u8) << 100_u32);
        // SAFETY: dictionary keys/values match their declared representations;
        // iterator output words are independent writable stack slots.
        unsafe {
            let strings = Rc::from_raw(relar_dict_new(3, 2));
            assert_eq!(
                relar_dict_get(Rc::as_ptr(&strings), Rc::as_ptr(&string_key) as usize),
                0
            );
            assert_eq!(message(&take_error(Kind::KeyError)).as_str(), "\"a'b\\n\"");
            relar_dict_delete(Rc::as_ptr(&strings), Rc::as_ptr(&string_key) as usize);
            assert_eq!(message(&take_error(Kind::KeyError)).as_str(), "\"a'b\\n\"");
            let integers = Rc::from_raw(relar_dict_new(2, 2));
            assert_eq!(relar_dict_get(Rc::as_ptr(&integers), integer_key.0), 0);
            assert_eq!(
                message(&take_error(Kind::KeyError)).as_str(),
                "1267650600228229401496703205376"
            );
            relar_dict_set(Rc::as_ptr(&integers), integer_key.0, 1);
            let iterator = Rc::from_raw(relar_dict_iter(Rc::as_ptr(&integers), 1));
            relar_dict_delete(Rc::as_ptr(&integers), integer_key.0);
            let (mut key, mut value) = (17, 19);
            assert_eq!(
                relar_mapping_iter_next(Rc::as_ptr(&iterator), &mut key, &mut value),
                0
            );
            drop(take_error(Kind::RuntimeError));
            assert_eq!((key, value), (17, 19));
            relar_dict_set(Rc::as_ptr(&integers), integer_key.0, 1);
            assert_eq!(
                relar_mapping_iter_next(Rc::as_ptr(&iterator), &mut key, &mut value),
                0
            );
            drop(take_error(Kind::RuntimeError));
            assert_eq!((key, value), (17, 19));
        }
    }
}
