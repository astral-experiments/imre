//! The sys.stdout binding and the original stream have independent lifetimes.
use super::{Error, Kind, Object, files};
use crate::ObjectOwner;
use std::cell::{OnceCell, RefCell};

#[derive(Clone, Default)]
enum Binding {
    #[default]
    Original,
    File(ObjectOwner),
    None,
    Missing,
}

thread_local! {
    static STDOUT: RefCell<Binding> = const { RefCell::new(Binding::Original) };
    static ORIGINAL: OnceCell<ObjectOwner> = const { OnceCell::new() };
    static STDERR: OnceCell<ObjectOwner> = const { OnceCell::new() };
}

fn original(stderr: bool) -> ObjectOwner {
    let root = if stderr { &STDERR } else { &ORIGINAL };
    root.with(|slot| ObjectOwner::clone(slot.get_or_init(|| files::standard_stream(stderr))))
}

fn binding() -> Binding {
    STDOUT.with_borrow(Clone::clone)
}

fn file(binding: Binding) -> Result<Option<ObjectOwner>, Error> {
    match binding {
        Binding::Original => Ok(Some(original(false))),
        Binding::File(value) => Ok(Some(value)),
        Binding::None => Ok(None),
        Binding::Missing => Err(Error::new(
            Kind::AttributeError,
            "module 'sys' has no attribute 'stdout'",
        )),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_original_output(stderr: u32) -> *const Object {
    ObjectOwner::into_raw(original(stderr != 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_has_stdout() -> u32 {
    STDOUT.with_borrow(|b| u32::from(!matches!(b, Binding::Missing)))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stdout() -> *const Object {
    match file(binding()) {
        Ok(Some(value)) => ObjectOwner::into_raw(value),
        Ok(None) => std::ptr::null(),
        Err(error) => error.returning(std::ptr::null()),
    }
}

/// # Safety
/// A nonzero optional word is a rooted native file, and name is a rooted string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_optional_receiver(
    word: usize,
    name: *const super::Str,
) -> *const Object {
    if word != 0 {
        // SAFETY: checked optional TextIO stores the file handle directly.
        ObjectOwner::into_raw(unsafe { ObjectOwner::acquire_word(word) })
    } else {
        // SAFETY: the caller roots the attribute's name for the diagnostic.
        let name = unsafe { &*name };
        Error::new(
            Kind::AttributeError,
            format!("'NoneType' object has no attribute '{name}'"),
        )
        .returning(std::ptr::null())
    }
}

/// # Safety
/// A non-null word points to a rooted native text stream; zero denotes None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sys_stdout_set(word: usize) {
    let value = if word == 0 {
        Binding::None
    } else {
        // SAFETY: the caller's live word has the checked TextIO representation.
        Binding::File(unsafe { ObjectOwner::acquire_word(word) })
    };
    drop(STDOUT.replace(value));
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stdout_del() {
    if matches!(STDOUT.replace(Binding::Missing), Binding::Missing) {
        Error::new(
            Kind::AttributeError,
            "'module' object has no attribute 'stdout'",
        )
        .raise();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stdout_clear() {
    drop(STDOUT.take());
}

// Return None for the original fast path until an alias has exposed the file's
// close state. Reading or mutating the namespace does not duplicate the OS fd.
fn target() -> Option<Binding> {
    match binding() {
        Binding::Original => ORIGINAL.with(|slot| slot.get().cloned().map(Binding::File)),
        binding => Some(binding),
    }
}

/// Print to the current binding, retaining a direct string for encoding errors.
pub(crate) fn write_line(
    text: &str,
    input: impl FnOnce() -> crate::Result<crate::StrOwner>,
) -> Result<(), Error> {
    match target() {
        None => crate::sys::write_stdout_with(text, true, input),
        Some(Binding::File(stream)) => {
            // SAFETY: the local owner roots the file; neither write invokes user code.
            unsafe { stream.0.with_mut(|s| s.file_mut().write_line(text, input)) }
        }
        Some(Binding::None) => Ok(()),
        Some(Binding::Missing) => Err(Error::new(Kind::RuntimeError, "lost sys.stdout")),
        Some(Binding::Original) => unreachable!(),
    }
}

/// Pin stdout before repr can replace it. Render a second time only if the original write cannot encode it.
pub(super) fn display(mut render: impl FnMut() -> Option<crate::StrOwner>) -> bool {
    let stream = match target() {
        None => None,
        Some(Binding::File(stream)) => Some(stream),
        Some(Binding::None | Binding::Missing) => {
            return Error::new(Kind::RuntimeError, "lost sys.stdout").returning(false);
        }
        Some(Binding::Original) => unreachable!(),
    };
    // Repr may access and close an initially unexposed original.
    let pinned = || {
        stream
            .clone()
            .or_else(|| ORIGINAL.with(|slot| slot.get().cloned()))
    };
    let write = |text: &str, owner: Option<&crate::StrOwner>| {
        let input = || {
            owner
                .cloned()
                .map_or_else(|| crate::sys::error_text(text), Ok)
        };
        match pinned() {
            None => crate::sys::write_stdout_with(text, false, input),
            Some(file) => {
                // SAFETY: the local owner pins the file across this native write.
                unsafe { file.0.with_mut(|s| s.file_mut().write_with(text, input)) }
            }
        }
    };
    let escape = match render() {
        Some(text) => match write(&text, Some(&text)) {
            Ok(()) => false,
            Err(error) if error.is_kind(Kind::UnicodeEncodeError) => true,
            Err(error) => return error.returning(false),
        },
        None => {
            // PyFile_WriteObject can also receive UnicodeEncodeError from repr itself.
            let pending = super::exceptions::take_pending();
            let encoding_error = pending.as_ref().is_some_and(|e| {
                // SAFETY: the owned pending value is an actual Python exception.
                unsafe {
                    super::exceptions::imre_exception_matches(
                        ObjectOwner::as_ptr(e),
                        Kind::UnicodeEncodeError as u32,
                    ) != 0
                }
            });
            if !encoding_error {
                super::exceptions::replace_pending(pending);
                return false;
            }
            true
        }
    };
    if escape {
        if let Some(file) = pinned() {
            // SAFETY: the owner protects the wrapper while reading retained metadata.
            if !unsafe { file.0.with(|s| s.file().has_encoding()) } {
                return Error::new(Kind::TypeError, "bad argument type for built-in operation")
                    .returning(false);
            }
        }
        let Some(retry) = render() else {
            return false;
        };
        let result = match pinned() {
            None => crate::sys::write_escaped_stdout(&retry),
            Some(file) => {
                // SAFETY: the repr callback finished before this file is borrowed.
                unsafe {
                    file.0
                        .with_mut(|s| s.file_mut().write_escaped_display(&retry))
                }
            }
        };
        if let Err(error) = result {
            return error.returning(false);
        }
    }
    match write("\n", None) {
        Ok(()) => true,
        Err(error) => error.returning(false),
    }
}
