//! Per-reader logical positions for one archive. Path inputs transfer the file
//! descriptor; caller-supplied streams are retained without taking close ownership.

use crate::ObjectOwner;
use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;

use super::{Error, Info, Object, Result, directory, files};
use crate::objects::exceptions::Kind;

/// Member handles share one buffered file. Each handle supplies its own absolute offset.
pub(super) struct Disk {
    reader: BufReader<File>,
    // Directory reads and failed I/O leave the next offset unknown.
    next: Option<u64>,
}

impl Disk {
    fn read_at(&mut self, at: u64, mut buf: &mut [u8]) -> io::Result<usize> {
        let previous = self.next.take();
        if let Some(delta) = previous.and_then(|p| at.checked_signed_diff(p)) {
            self.reader.seek_relative(delta)?;
        } else {
            self.reader.seek(SeekFrom::Start(at))?;
        }
        let mut n = 0;
        while !buf.is_empty() {
            match self.reader.read(buf) {
                Ok(0) => break,
                Ok(read) => {
                    n += read;
                    buf = &mut buf[read..];
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
        self.next = Some(at + n as u64);
        Ok(n)
    }
}

#[derive(Clone)]
pub(super) enum Source {
    Owned(crate::Shared<RefCell<Disk>>),
    Borrowed(ObjectOwner),
}

impl Source {
    /// Open a path-backed archive independently of any cached directory.
    pub(super) fn open(path: &str) -> Result<Self> {
        let (file, metadata) = files::open_file(path, 0)?;
        let size = crate::text_output::binary_capacity(metadata.blksize().into());
        crate::Shared::try_new(RefCell::new(Disk {
            reader: BufReader::with_capacity(size, file),
            next: None,
        }))
        .map(Self::Owned)
        .map_err(|_| Error::allocation())
    }

    /// Resolve an absolute archive position using the backing stream's seek rules.
    pub(super) fn absolute_offset(&self, offset: i128) -> Result<u64> {
        match self {
            Self::Owned(_) => files::disk_offset(offset),
            Self::Borrowed(object) => {
                // SAFETY: the archive retains the binary stream during validation.
                unsafe { object.0.with_mut(|s| s.file_mut().absolute_offset(offset)) }
            }
        }
    }

    pub(super) fn directory(&self, empty_unicode_path: fn() -> Result<()>) -> Result<Vec<Info>> {
        match self {
            Self::Owned(disk) => {
                let mut disk = disk.borrow_mut();
                disk.next = None;
                directory::entries(&mut disk.reader, empty_unicode_path)
            }
            Self::Borrowed(object) => {
                let mut cursor = BorrowedCursor {
                    object,
                    error: None,
                };
                let result = directory::entries(&mut cursor, empty_unicode_path);
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
            Self::Owned(disk) => disk
                .borrow_mut()
                .read_at(at, buf)
                .map_err(files::errno_error),
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
