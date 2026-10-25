//! C-main argument capture and independent Python-visible argument lists.

use crate::StrOwner;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_char};

use super::exceptions::{Error, Kind, Result, finish};
use super::{Object, Str, Words, fail};

#[derive(Clone, Copy)]
struct Arguments {
    count: usize,
    values: *const *const c_char,
}

thread_local! {
    static ARGUMENTS: Cell<Option<Arguments>> = const { Cell::new(None) };
    static ORIGINAL: RefCell<Option<crate::ObjectOwner>> = const { RefCell::new(None) };
    static LAUNCHER: RefCell<std::io::Result<Option<Vec<u8>>>> = const { RefCell::new(Ok(None)) };
    static ARGV: RefCell<Option<crate::ObjectOwner>> = const { RefCell::new(None) };
}

/// Captures argv without decoding or allocating. Generated main calls this only
/// when its closed program can access either argument list.
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
fn decode(arguments: Arguments) -> Result<crate::ObjectOwner> {
    decode_bytes(
        arguments.count,
        (0..arguments.count).map(|index| {
            // SAFETY: initialization guarantees each terminated entry outlives this call.
            unsafe { CStr::from_ptr(*arguments.values.add(index)).to_bytes() }
        }),
    )
}

fn decode_bytes<'a>(
    count: usize,
    arguments: impl Iterator<Item = &'a [u8]>,
) -> Result<crate::ObjectOwner> {
    let mut values = Words::new(3);
    values
        .values
        .try_reserve_exact(count)
        .map_err(|_| Error::new(Kind::MemoryError, "argument list allocation failed"))?;
    for (index, bytes) in arguments.enumerate() {
        let text = std::str::from_utf8(bytes).map_err(|error| {
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
            .push(StrOwner::into_raw(StrOwner::new(Str::new(text))) as usize);
    }
    Ok(values.into_object())
}

fn python_executable() -> Option<std::path::PathBuf> {
    use rustix::fs::{Access, access};
    std::env::var_os("IMRE_PYTHON")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .map(|directory| directory.join("python3"))
                .find(|candidate| candidate.is_file() && access(candidate, Access::EXEC_OK).is_ok())
        })
        .and_then(|path| std::path::absolute(path).ok())
}

/// The scripts directory belongs to the selected interpreter. Venv symlinks
/// stay inside the venv; aliases for a standalone installation follow the target.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sysconfig_scripts() -> *const Str {
    let directory = python_executable()
        .or_else(|| std::env::current_exe().ok())
        .and_then(|path| {
            let bin = path.parent()?;
            if bin.join("pyvenv.cfg").is_file()
                || bin
                    .parent()
                    .is_some_and(|prefix| prefix.join("pyvenv.cfg").is_file())
            {
                Some(bin.to_owned())
            } else {
                let executable = path.canonicalize().unwrap_or(path);
                let bin = executable.parent()?;
                // Debian's system interpreter installs console scripts into
                // /usr/local by default; venvs and package builds keep /usr/bin.
                let debian_local = bin == std::path::Path::new("/usr/bin")
                    && std::path::Path::new("/usr/share/python3/debian_defaults").is_file()
                    && !matches!(
                        std::env::var("DEB_PYTHON_INSTALL_LAYOUT").as_deref(),
                        Ok("deb" | "deb_system")
                    );
                Some(if debian_local {
                    bin.with_file_name("local").join("bin")
                } else {
                    bin.to_owned()
                })
            }
        });
    let text = directory
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or_default();
    StrOwner::into_raw(StrOwner::new(Str::from(text)))
}

/// The Python interpreter for explicitly launched Python helpers. Preserve
/// symlinks so a selected virtual environment keeps its own interpreter path.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_executable() -> *const Str {
    let text = python_executable()
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or_default();
    StrOwner::into_raw(StrOwner::new(Str::from(text)))
}

/// Returns an owned reference to the same mutable list on every successful call.
/// Strict UTF-8 failure returns null with a pending UnicodeDecodeError. Pending
/// state must be clear; init must already have captured this main activation.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_argv() -> *const Object {
    argument_list(&ARGV, || decode(c_arguments()))
}

fn c_arguments() -> Arguments {
    ARGUMENTS
        .get()
        .unwrap_or_else(|| fail("SystemError", "arguments were not initialized"))
}

pub(super) fn save_launcher_arguments(args: std::io::Result<Option<Vec<u8>>>) {
    LAUNCHER.with(|raw| *raw.borrow_mut() = args);
}

/// Like argv, this list has stable identity. It starts with the original
/// invocation and never observes mutations of the program's sys.argv.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_orig_argv() -> *const Object {
    argument_list(&ORIGINAL, || {
        let list = LAUNCHER.with(|raw| match &*raw.borrow() {
            Ok(Some(bytes)) => {
                let count = bytes.iter().filter(|&&b| b == 0).count();
                let bytes = bytes.strip_suffix(&[0]).ok_or_else(|| {
                    Error::new(Kind::OSError, "incomplete launcher argument vector")
                })?;
                decode_bytes(count, bytes.split(|&b| b == 0))
            }
            Ok(None) => decode(c_arguments()),
            Err(error) => Err(Error::new(Kind::OSError, error.to_string())),
        })?;
        save_launcher_arguments(Ok(None));
        Ok(list)
    })
}

fn argument_list(
    root: &'static std::thread::LocalKey<RefCell<Option<crate::ObjectOwner>>>,
    build: impl FnOnce() -> Result<crate::ObjectOwner>,
) -> *const Object {
    if let Some(value) = root.with(|root| root.borrow().clone()) {
        return crate::ObjectOwner::into_raw(value);
    }
    finish(
        build().map(|value| {
            root.with(|root| root.replace(Some(value.clone())));
            crate::ObjectOwner::into_raw(value)
        }),
        std::ptr::null(),
    )
}

/// Releases the runtime's list root and captured pointers on either main exit.
/// Existing aliases remain valid. Cleanup is infallible and preserves pending.
#[unsafe(no_mangle)]
pub extern "C" fn imre_args_clear() {
    ORIGINAL.with(RefCell::take);
    save_launcher_arguments(Ok(None));
    let previous = ARGV.with(RefCell::take);
    ARGUMENTS.set(None);
    super::hashlib::clear_algorithms();
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
            let first = crate::ObjectOwner::from_raw(imre_sys_argv());
            let alias = crate::ObjectOwner::from_raw(imre_sys_argv());
            assert!(crate::ObjectOwner::ptr_eq(&first, &alias));
            assert_eq!(
                Int(imre_seq_len(crate::ObjectOwner::as_ptr(&first))).as_small(),
                Some(3)
            );
            let unicode = StrOwner::from_raw(imre_seq_get(
                crate::ObjectOwner::as_ptr(&first),
                Int::small(2).unwrap().0,
            ) as *const Str);
            assert_eq!(unicode.as_str(), "é🦀");
            let added = StrOwner::new(Str::new("new argument"));
            imre_seq_append(
                crate::ObjectOwner::as_ptr(&first),
                StrOwner::as_ptr(&added) as usize,
                3,
            );
            assert_eq!(
                Int(imre_seq_len(crate::ObjectOwner::as_ptr(&alias))).as_small(),
                Some(4)
            );
            let weak = crate::ObjectOwner::downgrade(&first);
            let pending = imre_exception_new(Kind::RuntimeError as u32, std::ptr::null());
            imre_error_set(pending);
            imre_args_clear();
            assert_eq!(imre_error_take(), pending);
            imre_object_release(pending);
            imre_object_release(pending);
            drop(first);
            assert_eq!(
                Int(imre_seq_len(crate::ObjectOwner::as_ptr(&alias))).as_small(),
                Some(4)
            );
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
