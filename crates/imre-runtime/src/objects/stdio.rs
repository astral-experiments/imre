//! The sys.stdout binding and the original stream have independent lifetimes.
use super::{Error, Kind, Object, acquire_rc, files};
use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

#[derive(Clone, Default)]
enum Binding {
    #[default]
    Original,
    File(Rc<Object>),
    None,
    Missing,
}

thread_local! {
    static STDOUT: RefCell<Binding> = const { RefCell::new(Binding::Original) };
    static ORIGINAL: OnceCell<Rc<Object>> = const { OnceCell::new() };
    static STDERR: OnceCell<Rc<Object>> = const { OnceCell::new() };
}

fn original(stderr: bool) -> Rc<Object> {
    let root = if stderr { &STDERR } else { &ORIGINAL };
    root.with(|slot| Rc::clone(slot.get_or_init(|| files::standard_stream(stderr))))
}

fn binding() -> Binding {
    STDOUT.with_borrow(Clone::clone)
}

fn file(binding: Binding) -> Result<Option<Rc<Object>>, Error> {
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
    Rc::into_raw(original(stderr != 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stdout(importing: u32) -> *const Object {
    match file(binding()) {
        Ok(Some(value)) => Rc::into_raw(value),
        Ok(None) => std::ptr::null(),
        Err(_) if importing != 0 => Error::new(
            Kind::ImportError,
            "cannot import name 'stdout' from 'sys' (unknown location)",
        )
        .returning(std::ptr::null()),
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
        Rc::into_raw(unsafe { acquire_rc(word) })
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
        Binding::File(unsafe { acquire_rc(word) })
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

pub(crate) fn has_target() -> bool {
    target().is_some()
}

/// Return whether an override handled this line, including a pending Python error.
pub(crate) fn write_line(bytes: &[u8], display: bool) -> Option<Result<(), Error>> {
    let binding = target()?;
    Some(match binding {
        Binding::File(stream) => {
            // SAFETY: the local owner roots the file; both writes invoke only native I/O.
            unsafe {
                stream.0.with_mut(|storage| {
                    let file = storage.file_mut();
                    file.write_bytes(bytes)?;
                    file.write_bytes(b"\n")
                })
            }
        }
        Binding::None if !display => Ok(()),
        Binding::None | Binding::Missing => Err(Error::new(Kind::RuntimeError, "lost sys.stdout")),
        Binding::Original => unreachable!(),
    })
}
