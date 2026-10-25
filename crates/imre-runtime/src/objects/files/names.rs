//! Preserve the original spelling and byte encoding of POSIX path arguments.
use super::super::exceptions::{Error, push_string_repr};
use super::super::{Bytes, Str, bytes, paths};
use super::errno_with_paths;
use std::ffi::OsStr;
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
