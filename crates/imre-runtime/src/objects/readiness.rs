//! POSIX readiness observes fd numbers without taking ownership.
use super::exceptions::{Error, Kind, Result, finish};
use super::records::{RecordDescriptor, allocate_record};
use super::{Int, List, Object, Storage, StorageCell, Words, c_int, errno_error, object};
use std::{
    io, ptr,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct Poll {
    slots: Vec<libc::pollfd>,
    positions: std::collections::HashMap<i32, usize>,
}

impl Poll {
    pub(crate) fn contains(&self, fd: i32) -> bool {
        self.positions.contains_key(&fd)
    }

    pub(crate) fn set(
        &mut self,
        fd: i32,
        events: i16,
    ) -> std::result::Result<(), std::collections::TryReserveError> {
        if let Some(&index) = self.positions.get(&fd) {
            self.slots[index].events = events;
        } else {
            self.slots.try_reserve(1)?;
            self.positions.try_reserve(1)?;
            self.positions.insert(fd, self.slots.len());
            self.slots.push(libc::pollfd {
                fd,
                events,
                revents: 0,
            });
        }
        Ok(())
    }

    pub(crate) fn remove(&mut self, fd: i32) -> bool {
        let Some(index) = self.positions.remove(&fd) else {
            return false;
        };
        self.slots[index].fd = -1;
        if self.slots.len() > self.positions.len().saturating_mul(2) {
            self.compact();
        }
        true
    }

    /// Waits on the live descriptors and returns only kernel-reported events.
    pub(crate) fn ready(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<(usize, impl Iterator<Item = (i32, u16)> + '_)> {
        self.compact();
        let fds = &mut self.slots;
        let nfds = fds
            .len()
            .try_into()
            .map_err(|_| Error::new(Kind::OverflowError, "too many file descriptors"))?;
        let count = wait(timeout, |remaining| {
            let ms = remaining.map_or(-1, |d| d.as_nanos().div_ceil(1_000_000) as i32);
            // SAFETY: nfds is the length of this initialized, exclusively borrowed slice.
            unsafe { libc::poll(fds.as_mut_ptr(), nfds, ms) }
        })
        .map_err(errno_error)? as usize;
        Ok((
            count,
            fds.iter()
                .filter(|p| p.revents != 0)
                .take(count)
                .map(|p| (p.fd, p.revents as u16)),
        ))
    }

    // Deletion leaves a tombstone so re-registering a number moves it to the end.
    fn compact(&mut self) {
        if self.slots.len() != self.positions.len() {
            self.slots.retain(|p| p.fd >= 0);
            for (i, p) in self.slots.iter().enumerate() {
                *self.positions.get_mut(&p.fd).unwrap() = i;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_poll_new() -> *const Object {
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::Poll(Box::default()),
    ))))
}

/// Change the interest mask without taking ownership of the fd.
///
/// # Safety
/// The caller roots a poll object and integer arguments. A zero mask word is omitted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_poll_update(
    owner: *const Object,
    mode: u32,
    word: usize,
    mask: usize,
) {
    finish(
        (|| {
            let fd = fd(word)?;
            let events = if mask == 0 {
                libc::POLLIN | libc::POLLPRI | libc::POLLOUT
            } else {
                // SAFETY: a present mask is a rooted canonical integer.
                let mask = unsafe { Int::borrow(mask) };
                poll_mask(&mask)?
            };
            // SAFETY: the rooted receiver is exclusively borrowed without user callbacks.
            unsafe {
                object(owner).0.with_mut(|state| {
                    let Storage::Poll(p) = state else {
                        unreachable!()
                    };
                    if mode == 2 {
                        if !p.remove(fd) {
                            return Err(Error::key(super::words::WordKind::new(2).acquire(word)));
                        }
                    } else if mode == 1 && !p.contains(fd) {
                        return Err(errno_error(io::Error::from_raw_os_error(libc::ENOENT)));
                    } else {
                        p.set(fd, events).map_err(|_| Error::allocation())?;
                    }
                    Ok(())
                })
            }
        })(),
        (),
    );
}

/// Convert an interest mask to its unsigned 16-bit kernel representation.
pub(crate) fn poll_mask(mask: &Int) -> Result<i16> {
    let small = mask.as_small();
    if small.map_or_else(|| num_traits::Signed::is_negative(&*mask.big()), |n| n < 0) {
        return Err(Error::new(Kind::ValueError, "Cannot convert negative int"));
    }
    small
        .and_then(|n| u16::try_from(n).ok())
        .map(|n| n as i16)
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large for C unsigned short",
            )
        })
}

fn fd(word: usize) -> Result<i32> {
    checked_fd(c_int(word)?)
}

/// Reject negative descriptors before passing them to either readiness syscall.
pub(crate) fn checked_fd(fd: i32) -> Result<i32> {
    if fd < 0 {
        return Err(Error::new(
            Kind::ValueError,
            format!("file descriptor cannot be a negative integer ({fd})"),
        ));
    }
    Ok(fd)
}

// Keep the original deadline across EINTR. A zero timeout still observes readiness.
fn wait(
    timeout: Option<Duration>,
    mut call: impl FnMut(Option<Duration>) -> i32,
) -> io::Result<i32> {
    let start = Instant::now();
    let mut remaining = timeout;
    loop {
        let n = call(remaining);
        if n >= 0 {
            return Ok(n);
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
        remaining = timeout.map(|limit| limit.saturating_sub(start.elapsed()));
        if remaining == Some(Duration::ZERO) {
            return Ok(0);
        }
    }
}

// Nullable floats own their payload; bare floats contain the bits directly.
unsafe fn timeout(tag: u32, word: usize, poll: bool) -> Result<Option<Duration>> {
    use super::words::NULLABLE;
    if tag == 0 || tag & NULLABLE != 0 && word == 0 {
        return Ok(None);
    }
    let value = match tag & !NULLABLE {
        1 | 2 => {
            let n = if tag & !NULLABLE == 1 {
                Some((word - usize::from(tag & NULLABLE != 0)) as i64)
            } else {
                (word & 1 != 0).then_some((word as i64) >> 1)
            };
            return int_timeout(n, poll);
        }
        7 if tag & NULLABLE == 0 => f64::from_bits(word as u64),
        // SAFETY: the caller roots the present optional float's allocation.
        7 => unsafe { *(word as *const f64) },
        _ => {
            return Err(Error::new(
                Kind::TypeError,
                if poll {
                    "timeout must be an integer or None"
                } else {
                    "timeout must be a float or None"
                },
            ));
        }
    };
    float_timeout(value, poll)
}

/// Convert seconds or poll milliseconds without first rounding a Python integer to float.
pub(crate) fn int_timeout(n: Option<i64>, poll: bool) -> Result<Option<Duration>> {
    let overflow = || {
        Error::new(
            Kind::OverflowError,
            "timestamp too large to convert to C PyTime_t",
        )
    };
    let n = n.ok_or_else(overflow)?;
    let ns = n
        .checked_mul(if poll { 1_000_000 } else { 1_000_000_000 })
        .ok_or_else(overflow)?;
    if poll && i32::try_from(n).is_err() {
        return Err(Error::new(Kind::OverflowError, "timeout is too large"));
    }
    if n < 0 && !poll {
        return Err(Error::new(Kind::ValueError, "timeout must be non-negative"));
    }
    Ok((ns >= 0).then(|| Duration::from_nanos(ns as u64)))
}

/// Convert fractional timeouts before waiting on file descriptors.
pub(crate) fn float_timeout(value: f64, poll: bool) -> Result<Option<Duration>> {
    if value.is_nan() {
        return Err(Error::new(
            Kind::ValueError,
            "Invalid value NaN (not a number)",
        ));
    }
    let ns = value * if poll { 1e6 } else { 1e9 };
    if !(-9223372036854775808.0..9223372036854775808.0).contains(&ns) {
        return Err(Error::new(
            Kind::OverflowError,
            "timestamp out of range for platform time_t",
        ));
    }
    if poll && (value.floor() < f64::from(i32::MIN) || value.ceil() > f64::from(i32::MAX)) {
        return Err(Error::new(Kind::OverflowError, "timeout is too large"));
    }
    if value < 0.0 && !poll {
        return Err(Error::new(Kind::ValueError, "timeout must be non-negative"));
    }
    Ok((value >= 0.0).then(|| Duration::from_nanos(ns.ceil() as u64)))
}

/// Return fresh (fd, revents) tuples for descriptors with kernel events.
///
/// # Safety
/// The caller roots a poller and tagged timeout, and supplies a tuple[int, int] descriptor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_poll_wait(
    owner: *const Object,
    tag: u32,
    word: usize,
    pair: *const RecordDescriptor,
) -> *const Object {
    // SAFETY: rooted arguments and the single-threaded syscall have no user callbacks.
    let result = unsafe {
        object(owner).0.with_mut(|state| {
            let Storage::Poll(p) = state else {
                unreachable!()
            };
            let timeout = timeout(tag, word, true)?;
            let (count, events) = p.ready(timeout)?;
            let mut out = Words::new(6); // owned records
            out.values
                .try_reserve(count)
                .map_err(|_| Error::allocation())?;
            for (fd, mask) in events {
                // SAFETY: pair describes two writable tagged integer fields. The result
                // sequence takes ownership of each fully initialized tuple.
                let tuple = ptr::NonNull::new(allocate_record(pair))
                    .ok_or_else(Error::allocation)?
                    .as_ptr();
                tuple.write(Int::from_usize(fd as usize).into_raw());
                tuple
                    .add(1)
                    .write(Int::from_usize(usize::from(mask)).into_raw());
                out.values.push(tuple as usize);
            }
            Ok(crate::ObjectOwner::into_raw(out.into_object()))
        })
    };
    finish(result, ptr::null())
}

/// The three checked descriptor sets used by one POSIX select call.
pub(crate) struct SelectFds {
    sets: [libc::fd_set; 3],
    max: i32,
}

impl SelectFds {
    /// Start with no descriptors in any interest set.
    pub(crate) fn new() -> Self {
        Self {
            // SAFETY: all-zero fd sets are valid and contain no descriptors.
            sets: unsafe { std::mem::zeroed() },
            max: -1,
        }
    }

    /// Validate each descriptor and limit each input separately, including duplicate entries.
    pub(crate) fn insert(&mut self, group: usize, index: usize, fd: i32) -> Result<()> {
        if fd as usize >= libc::FD_SETSIZE {
            return Err(Error::new(
                Kind::ValueError,
                "filedescriptor out of range in select()",
            ));
        }
        if index >= libc::FD_SETSIZE {
            return Err(Error::new(
                Kind::ValueError,
                "too many file descriptors in select()",
            ));
        }
        // SAFETY: fd is nonnegative and strictly below FD_SETSIZE.
        unsafe { libc::FD_SET(fd, &mut self.sets[group]) };
        self.max = self.max.max(fd);
        Ok(())
    }

    /// Replace the interest sets with the kernel's readiness result, retrying interrupted waits.
    pub(crate) fn wait(&mut self, timeout: Option<Duration>) -> Result<bool> {
        let mut ready = self.sets;
        let count = wait(timeout, |remaining| {
            ready = self.sets;
            let [r, w, x] = &mut ready;
            let mut tv = remaining.map(|d| {
                let us = d.as_nanos().div_ceil(1_000);
                libc::timeval {
                    tv_sec: (us / 1_000_000) as _,
                    tv_usec: (us % 1_000_000) as _,
                }
            });
            // SAFETY: all sets and optional timeval are writable for the syscall.
            unsafe {
                libc::select(
                    self.max + 1,
                    r,
                    w,
                    x,
                    tv.as_mut().map_or(ptr::null_mut(), ptr::from_mut),
                )
            }
        })
        .map_err(errno_error)?;
        self.sets = if count == 0 { Self::new().sets } else { ready };
        Ok(count != 0)
    }

    /// Test an original descriptor against its ready set.
    pub(crate) fn ready(&self, group: usize, fd: i32) -> bool {
        // SAFETY: only descriptor numbers within the fd_set are inspected.
        (fd as usize) < libc::FD_SETSIZE && unsafe { libc::FD_ISSET(fd, &self.sets[group]) }
    }
}

/// Select over three integer lists, retaining each input's order and duplicates.
///
/// # Safety
/// The three lists stay rooted; out has three fresh pointer slots for list[int].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_select_wait(
    r: *const List,
    w: *const List,
    x: *const List,
    tag: u32,
    word: usize,
    out: *mut *const List,
) {
    finish(
        (|| {
            // SAFETY: the caller roots the tagged timeout.
            let timeout = unsafe { timeout(tag, word, false) }?;
            let mut sets = SelectFds::new();
            let lists = [r, w, x];
            for (group, list) in lists.iter().enumerate() {
                // SAFETY: list points to the rooted integer list for this interest set.
                unsafe {
                    (&**list).0.with(|values| {
                        values
                            .iter()
                            .enumerate()
                            .try_for_each(|(index, value)| sets.insert(group, index, fd(value.0)?))
                    })
                }?;
            }
            let any_ready = sets.wait(timeout)?;
            for (i, list) in lists.into_iter().enumerate() {
                let mut result = Vec::new();
                if any_ready {
                    // SAFETY: the exact list is still rooted, and every fd was checked above.
                    unsafe {
                        (&*list).0.with(|values| -> Result<()> {
                            for value in values {
                                if sets.ready(i, (value.0 as i64 >> 1) as i32) {
                                    result.try_reserve(1).map_err(|_| Error::allocation())?;
                                    result.push(value.clone());
                                }
                            }
                            Ok(())
                        })
                    }?;
                }
                // SAFETY: transfer the fresh list to its owned field; the caller also
                // releases partially initialized output if a later allocation fails.
                unsafe {
                    out.add(i).write(crate::new_list(result));
                }
            }
            Ok(())
        })(),
        (),
    )
}
