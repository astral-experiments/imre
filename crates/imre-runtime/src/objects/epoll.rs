//! Linux epoll owns the control fd; registrations are maintained by the kernel.

use super::errno_error;
use super::exceptions::{Error, Kind};
use super::time::Deadline;
use std::cell::Cell;
use std::io;
use std::os::fd::RawFd;
use std::time::Duration;

pub(crate) struct Epoll {
    fd: Cell<RawFd>,
}

impl Epoll {
    /// Create a non-inheritable epoll for -1, or adopt the supplied fd without duplicating it.
    pub(crate) fn from_fd(fd: RawFd) -> io::Result<Self> {
        let fd = if fd == -1 {
            // SAFETY: epoll_create1 takes only a flag word.
            unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) }
        } else {
            fd
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd: Cell::new(fd) })
    }

    pub(crate) fn fd(&self) -> RawFd {
        self.fd.get()
    }

    /// Invalidate the handle first. A caller may already have closed or reused the number.
    pub(crate) fn close(&self) {
        let fd = self.fd.replace(-1);
        if fd >= 0 {
            // SAFETY: close operates on the fd number. Even an error consumes this object's claim.
            unsafe { libc::close(fd) };
        }
    }

    /// Store the registered descriptor using the native epoll_data_t.fd union layout.
    pub(crate) fn ctl(&self, op: i32, fd: RawFd, events: u32) -> io::Result<()> {
        let mut data = [0; 8];
        data[..4].copy_from_slice(&fd.to_ne_bytes());
        let mut event = libc::epoll_event {
            events,
            u64: u64::from_ne_bytes(data),
        };
        // SAFETY: event is valid for the duration of epoll_ctl, including EPOLL_CTL_DEL.
        if unsafe { libc::epoll_ctl(self.fd(), op, fd, &mut event) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Wait in milliseconds, dispatch Python signals on EINTR, and preserve the original deadline.
    pub(crate) fn poll<E: From<Error>>(
        &self,
        timeout_ns: Option<i64>,
        maxevents: i32,
        mut check_signals: impl FnMut() -> Result<(), E>,
    ) -> Result<Events, E> {
        let mut millis = -1;
        let mut deadline = None;
        if let Some(ns) = timeout_ns {
            // Signed division truncates towards zero. Only a positive remainder needs rounding up.
            let ms = ns / 1_000_000 + i64::from(ns % 1_000_000 > 0);
            millis = i32::try_from(ms)
                .map_err(|_| Error::new(Kind::OverflowError, "timeout is too large"))?
                .max(-1);
            if ns >= 0 {
                deadline = Some(Deadline::after(Duration::from_nanos(ns as u64)));
            }
        }
        let maxevents = match maxevents {
            -1 => libc::FD_SETSIZE as i32 - 1,
            1.. => maxevents,
            _ => {
                return Err(Error::new(
                    Kind::ValueError,
                    format!("maxevents must be greater than 0, got {maxevents}"),
                )
                .into());
            }
        };
        let mut events = Vec::new();
        events
            .try_reserve_exact(maxevents as usize)
            .map_err(|_| Error::memory_error())?;
        loop {
            // SAFETY: the allocation can hold maxevents epoll_event records. Read only the
            // initialized prefix reported by the kernel, so a large requested limit costs no
            // userspace initialization of untouched pages.
            let n = unsafe { libc::epoll_wait(self.fd(), events.as_mut_ptr(), maxevents, millis) };
            if n >= 0 {
                // SAFETY: epoll_wait initialized exactly n entries within the provided capacity.
                unsafe { events.set_len(n as usize) };
                break;
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(errno_error(err).into());
            }
            check_signals()?;
            if let Some(end) = deadline {
                let Some(left) = end.checked_remaining() else {
                    break;
                };
                millis = left.as_nanos().div_ceil(1_000_000) as i32;
            }
        }
        Ok(Events(events.into_iter()))
    }
}

impl Drop for Epoll {
    fn drop(&mut self) {
        self.close();
    }
}

pub(crate) struct Events(std::vec::IntoIter<libc::epoll_event>);

impl Iterator for Events {
    type Item = (RawFd, u32);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|event| {
            let [a, b, c, d, ..] = event.u64.to_ne_bytes();
            (RawFd::from_ne_bytes([a, b, c, d]), event.events)
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for Events {}
