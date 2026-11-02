use super::{Error, Id, NONE, Result};
use std::ffi::CStr;

type MkfifoAt = unsafe extern "C" fn(libc::c_int, *const libc::c_char, libc::mode_t) -> libc::c_int;

/// macOS added descriptor-relative FIFO creation in 13.0.
pub(super) fn at_function() -> Option<MkfifoAt> {
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: the name is terminated and RTLD_DEFAULT searches the loaded system libraries.
        let ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"mkfifoat".as_ptr()) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: mkfifoat in libSystem has the declared POSIX signature.
        Some(unsafe { std::mem::transmute::<*mut libc::c_void, MkfifoAt>(ptr) })
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        Some(libc::mkfifoat)
    }
}

pub(super) fn create(path: &CStr, mode: libc::mode_t, dir_fd: i32) -> Result<Id> {
    loop {
        let rc = if dir_fd == libc::AT_FDCWD {
            // SAFETY: path remains terminated and live through the synchronous call.
            unsafe { libc::mkfifo(path.as_ptr(), mode) }
        } else {
            let mkfifoat = at_function().ok_or_else(|| {
                Error::new("NotImplementedError", "dir_fd unavailable on this platform")
            })?;
            // SAFETY: the kernel validates the caller's descriptor, and path remains live.
            unsafe { mkfifoat(dir_fd, path.as_ptr(), mode) }
        };
        if rc == 0 {
            return Ok(NONE);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            // CPython uses posix_error() without attaching the pathname.
            return Err(crate::objects::errno_error(error).into());
        }
    }
}
