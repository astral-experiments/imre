//! Per-reader logical positions for one archive. Path inputs transfer the file
//! descriptor; caller-supplied streams are retained without taking close ownership.

use crate::ObjectOwner;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;

use super::{Error, Info, Object, Result, directory, files};
use crate::objects::exceptions::Kind;

#[derive(Clone)]
pub(super) enum Source {
    Owned(crate::Shared<File>),
    Borrowed(ObjectOwner),
}

impl Source {
    /// Resolve an absolute archive position using the backing stream's seek rules.
    pub(super) fn absolute_offset(&self, offset: i128) -> Result<u64> {
        match self {
            Self::Owned(file) => (&**file)
                .seek(SeekFrom::Start(files::disk_offset(offset)?))
                .map_err(files::errno_error),
            Self::Borrowed(object) => {
                // SAFETY: the archive retains the binary stream during validation.
                unsafe { object.0.with_mut(|s| s.file_mut().absolute_offset(offset)) }
            }
        }
    }

    pub(super) fn directory(&self) -> Result<Vec<Info>> {
        match self {
            Self::Owned(file) => directory::entries(&mut &**file),
            Self::Borrowed(object) => {
                let mut cursor = BorrowedCursor {
                    object,
                    error: None,
                };
                let result = directory::entries(&mut cursor);
                // zipfile turns OS errors during EOCD lookup into BadZipFile, but
                // a closed file-like object reports ValueError from the input.
                match cursor.error {
                    Some(error) if error.is_kind(Kind::ValueError) => Err(error),
                    _ => result,
                }
            }
        }
    }

    /// An externally supplied file exposes seek/tell; local-extra seek and
    /// diagnostic reads must leave its cursor where zipfile would leave it.
    pub(super) fn position(&self, at: u64) -> Result<()> {
        if let Self::Borrowed(object) = self {
            // SAFETY: the source retains the exact stream throughout the seek.
            unsafe {
                object
                    .0
                    .with_mut(|s| s.file_mut().seek(SeekFrom::Start(at)))
            }?;
        }
        Ok(())
    }

    pub(super) fn read(&self, at: u64, buf: &mut [u8]) -> Result<usize> {
        match self {
            Self::Owned(file) => loop {
                match file.read_at(buf, at) {
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    result => return result.map_err(files::errno_error),
                }
            },
            Self::Borrowed(object) => {
                // SAFETY: the archive owns the input object and no callbacks run.
                unsafe {
                    object.0.with_mut(|storage| {
                        let f = storage.file_mut();
                        f.seek(SeekFrom::Start(at))?;
                        f.read_into(buf)
                    })
                }
            }
        }
    }
}

struct BorrowedCursor<'a> {
    object: &'a Object,
    error: Option<Error>,
}

impl BorrowedCursor<'_> {
    fn result<T>(&mut self, result: Result<T>) -> io::Result<T> {
        result.map_err(|error| {
            self.error = Some(error);
            io::Error::other("archive input error")
        })
    }
}

impl Read for BorrowedCursor<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: the caller retains the input; only owned counts or errors escape.
        let result = unsafe { self.object.0.with_mut(|s| s.file_mut().read_into(buf)) };
        self.result(result)
    }
}

impl Seek for BorrowedCursor<'_> {
    fn seek(&mut self, at: SeekFrom) -> io::Result<u64> {
        // SAFETY: the sealed source operations do not invoke Python code.
        let result = unsafe { self.object.0.with_mut(|s| s.file_mut().seek(at)) };
        self.result(result)
    }
}
