//! Borrow a directory descriptor for a scan, or open a stream on an encoded path.
use std::ffi::CStr;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::ptr::NonNull;

#[derive(Debug)]
pub(crate) struct Directory {
    stream: Option<NonNull<libc::DIR>>,
    rewind_on_close: bool,
}

pub(crate) struct Entry {
    pub(crate) name: Vec<u8>,
    pub(crate) ino: libc::ino_t,
    pub(crate) d_type: u8,
}

/// Duplication failure has no Python filename; fdopendir failures carry the caller's fd.
pub(crate) fn duplicate(fd: i32) -> super::Result<OwnedFd> {
    let fd = super::dup(fd)?;
    // SAFETY: dup transferred a new close-on-exec descriptor to us.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

impl Directory {
    pub(crate) fn open(path: &CStr) -> io::Result<Self> {
        // SAFETY: the pathname is NUL-terminated and opendir transfers ownership on success.
        let stream = NonNull::new(unsafe { libc::opendir(path.as_ptr()) })
            .ok_or_else(io::Error::last_os_error)?;
        Ok(Self {
            stream: Some(stream),
            rewind_on_close: false,
        })
    }

    pub(crate) fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        // SAFETY: this descriptor is owned. fdopendir takes ownership only on success.
        let stream = NonNull::new(unsafe { libc::fdopendir(fd.as_raw_fd()) })
            .ok_or_else(io::Error::last_os_error)?;
        let _ = fd.into_raw_fd();
        Ok(Self {
            stream: Some(stream),
            rewind_on_close: true,
        })
    }

    pub(crate) fn is_open(&self) -> bool {
        self.stream.is_some()
    }

    pub(crate) fn next_entry(&mut self) -> io::Result<Option<Entry>> {
        let Some(stream) = self.stream else {
            return Ok(None);
        };
        loop {
            // SAFETY: this stream is live and exclusively accessed here. Clear the
            // calling thread's errno to distinguish end of directory from a read error.
            let raw = unsafe {
                #[cfg(target_os = "linux")]
                {
                    *libc::__errno_location() = 0;
                }
                #[cfg(target_vendor = "apple")]
                {
                    *libc::__error() = 0;
                }
                #[cfg(target_os = "android")]
                {
                    *libc::__errno() = 0;
                }
                libc::readdir(stream.as_ptr())
            };
            if raw.is_null() {
                let error = io::Error::last_os_error();
                self.close();
                return if error.raw_os_error() == Some(0) {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            // SAFETY: readdir returns a live record with a NUL-terminated name.
            // Copy its fields before reading again; d_name can be variable length.
            let (name, ino, d_type) = unsafe {
                (
                    CStr::from_ptr((&raw const (*raw).d_name).cast()).to_bytes(),
                    (*raw).d_ino,
                    (*raw).d_type,
                )
            };
            if !matches!(name, b"." | b"..") {
                return Ok(Some(Entry {
                    name: name.to_vec(),
                    ino,
                    d_type,
                }));
            }
        }
    }

    pub(crate) fn close(&mut self) {
        if let Some(stream) = self.stream.take() {
            // SAFETY: we uniquely own this open DIR. Duplicating an fd shares its
            // offset with the caller, so CPython rewinds it before closing the duplicate.
            unsafe {
                if self.rewind_on_close {
                    libc::rewinddir(stream.as_ptr());
                }
                libc::closedir(stream.as_ptr());
            }
        }
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        self.close();
    }
}

/// The path and descriptor are those retained by the entry, independent of the stream's lifetime.
pub(crate) fn stat(dir_fd: i32, path: &CStr, follow: bool) -> io::Result<libc::stat> {
    let mut stat = MaybeUninit::uninit();
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    // SAFETY: fstatat validates dir_fd and fills the output on success without taking ownership.
    if unsafe { libc::fstatat(dir_fd, path.as_ptr(), stat.as_mut_ptr(), flags) } == 0 {
        // SAFETY: the successful call initialized stat.
        Ok(unsafe { stat.assume_init() })
    } else {
        Err(io::Error::last_os_error())
    }
}
