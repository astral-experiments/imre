//! The private subprocess selector binding controls the system call used to
//! multiplex pipe descriptors. Each communicate call snapshots the factory.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

static USE_SELECT: AtomicBool = AtomicBool::new(false);
thread_local! {
    static CLASSES: [crate::ObjectOwner; 2] = [false, true].map(|select|
        crate::ObjectOwner::new(Object(StorageCell::new(Storage::ProcessSelectorClass(select)))));
}

/// Return a rooted selector class: poll (0), select (1), or the current binding (2).
#[unsafe(no_mangle)]
pub extern "C" fn imre_popen_selector_class(which: u32) -> *const Object {
    let select = match which {
        0 => false,
        1 => true,
        2 => current(),
        _ => fail("SystemError", "invalid subprocess selector class"),
    };
    CLASSES.with(|classes| {
        crate::ObjectOwner::into_raw(crate::ObjectOwner::clone(&classes[usize::from(select)]))
    })
}

/// # Safety
/// The caller roots a selector class through the assignment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_set_selector(class: *const Object) {
    // SAFETY: the compiler checks the precise selector-class type.
    let select = unsafe {
        object(class).0.with(|storage| match storage {
            Storage::ProcessSelectorClass(value) => *value,
            _ => fail("SystemError", "expected a subprocess selector class"),
        })
    };
    USE_SELECT.store(select, Ordering::Relaxed);
}

pub(super) fn current() -> bool {
    USE_SELECT.load(Ordering::Relaxed)
}

/// Fill revents for the same three native pipe owners. False asks the caller
/// to recompute the remaining deadline after a signal interrupted the wait.
pub(super) fn wait(
    select: bool,
    fds: &mut [libc::pollfd; 3],
    remaining: Option<f64>,
) -> Result<bool> {
    let rc = if select {
        // SAFETY: fd_set contains only bits. Validate every fd before using
        // FD_SET: an fd outside FD_SETSIZE would write past the fixed object.
        let (mut read, mut write): (libc::fd_set, libc::fd_set) = unsafe { std::mem::zeroed() };
        let mut nfds = 0;
        for entry in fds.iter().filter(|entry| entry.fd >= 0) {
            if entry.fd as usize >= libc::FD_SETSIZE {
                return Err(Error::new(
                    Kind::ValueError,
                    "filedescriptor out of range in select()",
                ));
            }
            nfds = nfds.max(entry.fd + 1);
            // SAFETY: each fd was checked against the size of this initialized set.
            unsafe {
                if entry.events & libc::POLLIN != 0 {
                    libc::FD_SET(entry.fd, &mut read);
                }
                if entry.events & libc::POLLOUT != 0 {
                    libc::FD_SET(entry.fd, &mut write);
                }
            }
        }
        let mut timeout = remaining
            .map(|seconds| {
                if seconds.is_nan() {
                    return Err(Error::new(
                        Kind::ValueError,
                        "Invalid value NaN (not a number)",
                    ));
                }
                // select() first converts this float to CPython's signed
                // nanosecond clock, then rounds up to whole microseconds.
                let nanos = (seconds * 1e9).ceil();
                if nanos >= i64::MAX as f64 {
                    return Err(Error::new(
                        Kind::OverflowError,
                        "timestamp out of range for platform time_t",
                    ));
                }
                let nanos = nanos as i64;
                let micros = nanos / 1000 + i64::from(nanos % 1000 != 0);
                Ok(libc::timeval {
                    tv_sec: (micros / 1_000_000) as _,
                    tv_usec: (micros % 1_000_000) as _,
                })
            })
            .transpose()?;
        // SAFETY: the sets and optional timeval live through this call;
        // the caller roots every descriptor, and no exception fd set is needed.
        let rc = unsafe {
            libc::select(
                nfds,
                &mut read,
                &mut write,
                std::ptr::null_mut(),
                timeout
                    .as_mut()
                    .map_or(std::ptr::null_mut(), std::ptr::from_mut),
            )
        };
        if rc >= 0 {
            for entry in fds.iter_mut() {
                entry.revents = 0;
                if entry.fd >= 0 {
                    // SAFETY: all nonnegative descriptors passed the bounds check above.
                    unsafe {
                        if libc::FD_ISSET(entry.fd, &read) {
                            entry.revents |= libc::POLLIN;
                        }
                        if libc::FD_ISSET(entry.fd, &write) {
                            entry.revents |= libc::POLLOUT;
                        }
                    }
                }
            }
        }
        rc
    } else {
        let ms = remaining.map(poll_milliseconds).transpose()?.unwrap_or(-1);
        // SAFETY: all entries live through the call; a negative fd is ignored by poll.
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) }
    };
    if rc < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(errno_error(error));
        }
        return Ok(false);
    }
    Ok(true)
}
