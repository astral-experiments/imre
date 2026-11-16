//! Linux availability checks and fallback providers for time.get_clock_info.

use super::{Error, clock_resolution, process_clock_fallback, timestamp_ns};
use crate::objects::errno_error;
use std::{io, mem::MaybeUninit};

/// CPython obtains a clock reading even for a metadata-only query.
pub(super) fn probe(
    id: libc::clockid_t,
    implementation: &'static str,
) -> Result<(&'static str, f64), Error> {
    let mut sample = MaybeUninit::uninit();
    // SAFETY: clock_gettime initializes the timespec on success.
    if unsafe { libc::clock_gettime(id, sample.as_mut_ptr()) } != 0 {
        if id == libc::CLOCK_PROCESS_CPUTIME_ID {
            let (_, info) = process_clock_fallback()?;
            return Ok(info);
        }
        return Err(errno_error(io::Error::last_os_error()));
    }
    // SAFETY: the successful clock_gettime call initialized sample.
    let sample = unsafe { sample.assume_init() };
    let check = || timestamp_ns(sample.tv_sec, sample.tv_nsec);
    // The wall and monotonic providers convert their sample before querying
    // resolution. The CPU providers query resolution first.
    let cpu = matches!(
        id,
        libc::CLOCK_PROCESS_CPUTIME_ID | libc::CLOCK_THREAD_CPUTIME_ID
    );
    if !cpu {
        check()?;
    }
    let resolution = match clock_resolution(id) {
        Ok(resolution) => resolution,
        Err(_) if id == libc::CLOCK_REALTIME => 1e-9,
        Err(error) => return Err(errno_error(error)),
    };
    if cpu {
        check()?;
    }
    Ok((implementation, resolution))
}
