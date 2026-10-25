//! Preserve the original spelling and byte encoding of POSIX path arguments.
use super::super::exceptions::{Error, Filename, push_string_repr};
use super::super::{Bytes, Str, acquire_rc, bytes, paths};
use super::errno_with_filenames;
use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::rc::Rc;

pub(in crate::objects) enum Name {
    Text(Rc<Str>),
    Bytes(Rc<Bytes>),
    Path(Rc<Str>),
}

impl Name {
    /// The call retains all three possible handle kinds until the operation finishes.
    pub(in crate::objects) unsafe fn from_handle(value: *const (), tag: u32) -> Self {
        // SAFETY: the compiler supplies the tag for the exact live allocation.
        unsafe {
            match tag {
                0 => Self::Text(acquire_rc(value as usize)),
                1 => Self::Bytes(acquire_rc(value as usize)),
                2 => Self::Path(Rc::from_raw(paths::imre_path_text(value.cast()))),
                _ => unreachable!(),
            }
        }
    }

    pub(in crate::objects) fn path(&self) -> &Path {
        match self {
            Self::Text(text) => Path::new(text.as_str()),
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
        let spelling = match self {
            Self::Text(text) | Self::Path(text) => Filename::Str(Rc::clone(text)),
            Self::Bytes(bytes) => Filename::Bytes(Rc::clone(bytes)),
        };
        errno_with_filenames(error, Some(spelling), None)
    }
}
