//! Preserve the original spelling and byte encoding of POSIX path arguments.
use super::super::exceptions::{Error, Kind, finish, push_string_repr};
use super::super::{Bytes, Int, Str, bytes, paths};
use super::errno_with_paths;
use num_traits::ToPrimitive;
use std::ffi::{CString, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::rc::Rc;

pub(in crate::objects) enum Name<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
    Path(Rc<Str>),
}

impl<'a> Name<'a> {
    /// The call retains all three possible handle kinds until the operation finishes.
    pub(in crate::objects) unsafe fn from_handle(value: *const (), tag: u32) -> Self {
        // SAFETY: the compiler supplies the tag for the exact live allocation.
        unsafe {
            match tag {
                0 => Self::Text(&*value.cast::<Str>()),
                1 => Self::Bytes(&*value.cast::<Bytes>()),
                2 => Self::Path(Rc::from_raw(paths::imre_path_text(value.cast()))),
                _ => unreachable!(),
            }
        }
    }

    pub(in crate::objects) fn path(&self) -> &Path {
        match self {
            Self::Text(text) => Path::new(text),
            Self::Bytes(bytes) => Path::new(OsStr::from_bytes(bytes)),
            Self::Path(text) => Path::new(text.as_str()),
        }
    }

    pub(in crate::objects) fn repr(&self, original: bool) -> String {
        let mut result = String::new();
        match self {
            Self::Bytes(value) => bytes::write_repr(&mut result, value),
            Self::Path(text) if original => {
                result.push_str("PosixPath(");
                push_string_repr(text, &mut result);
                result.push(')');
            }
            _ => push_string_repr(self.path().to_str().unwrap(), &mut result),
        }
        result
    }

    pub(in crate::objects) fn display(&self) -> String {
        match self {
            Self::Bytes(_) => self.repr(false),
            _ => self.path().display().to_string(),
        }
    }

    pub(in crate::objects) fn error(&self, error: io::Error) -> Error {
        errno_with_paths(error, |out| {
            out.push_str(": ");
            out.push_str(&self.repr(false));
        })
    }
}

/// Remove the entry itself, using the raw spelling even for trailing slashes.
/// Validate the path before converting the optional directory descriptor.
///
/// # Safety
/// The caller roots the path for its static tag and the optional integer word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_os_unlink(value: *const (), tag: u32, fd: usize, remove: u32) {
    // SAFETY: the path's owner stays live through the syscall and error rendering.
    let name = unsafe { Name::from_handle(value, tag) };
    let result = (|| {
        let path = CString::new(name.path().as_os_str().as_bytes()).map_err(|_| {
            let function = if remove == 0 { "unlink" } else { "remove" };
            Error::new(
                Kind::ValueError,
                format!("{function}: embedded null character in path"),
            )
        })?;
        let dir_fd = if fd == 0 {
            libc::AT_FDCWD
        } else {
            // SAFETY: a nonzero optional integer has the native tagged-int ABI.
            let integer = unsafe { Int::borrow(fd) };
            let n = integer.big();
            n.to_i32().ok_or_else(|| {
                Error::new(
                    Kind::OverflowError,
                    if n.sign() == num_bigint::Sign::Minus {
                        "fd is less than minimum"
                    } else {
                        "fd is greater than maximum"
                    },
                )
            })?
        };
        // SAFETY: the zero-terminated path is owned until unlinkat returns.
        if unsafe { libc::unlinkat(dir_fd, path.as_ptr(), 0) } == -1 {
            return Err(name.error(io::Error::last_os_error()));
        }
        Ok(())
    })();
    finish(result, ());
}
