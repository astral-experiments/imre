//! Linux availability checks and fallback providers for time.get_clock_info.

use super::{Error, Kind, clock_resolution};
use crate::objects::errno_error;
use std::{io, mem::MaybeUninit};

// CPython samples the clock even for an information request, including its
// conversion to a signed 64-bit nanosecond count.
fn check_timestamp(seconds: impl Into<i64>, nanos: impl Into<i64>) -> Result<(), Error> {
    seconds
        .into()
        .checked_mul(1_000_000_000)
        .and_then(|whole| whole.checked_add(nanos.into()))
        .map(|_| ())
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "timestamp too large to convert to C PyTime_t",
            )
        })
}

/// CPython obtains a clock reading even for a metadata-only query.
pub(super) fn probe(
    id: libc::clockid_t,
    implementation: &'static str,
) -> Result<(&'static str, f64), Error> {
    let mut sample = MaybeUninit::uninit();
    // SAFETY: clock_gettime initializes the timespec on success.
    if unsafe { libc::clock_gettime(id, sample.as_mut_ptr()) } != 0 {
        if id == libc::CLOCK_PROCESS_CPUTIME_ID {
            return process_fallback();
        }
        return Err(errno_error(io::Error::last_os_error()));
    }
    // SAFETY: the successful clock_gettime call initialized sample.
    let sample = unsafe { sample.assume_init() };
    let check = || check_timestamp(sample.tv_sec, sample.tv_nsec);
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

/// Try the remaining POSIX process CPU clocks in the same order as CPython.
fn process_fallback() -> Result<(&'static str, f64), Error> {
    let mut usage = MaybeUninit::uninit();
    // SAFETY: getrusage initializes usage on success.
    let (implementation, resolution) = if unsafe {
        libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr())
    } == 0
    {
        // SAFETY: getrusage returned success.
        let usage = unsafe { usage.assume_init() };
        for tv in [usage.ru_utime, usage.ru_stime] {
            check_timestamp(tv.tv_sec, tv.tv_usec * 1000)?;
        }
        ("getrusage(RUSAGE_SELF)", 1e-6)
    } else {
        let mut times = MaybeUninit::uninit();
        // SAFETY: times writes the process accounting snapshot into a tms.
        if unsafe { libc::times(times.as_mut_ptr()) } != -1 {
            // SAFETY: _SC_CLK_TCK is a standard sysconf key.
            let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
            if ticks <= 0 {
                return Err(errno_error(io::Error::last_os_error()));
            }
            ("times()", 1.0 / ticks as f64)
        } else {
            unsafe extern "C" {
                fn clock() -> libc::clock_t;
            }
            // SAFETY: clock has no arguments. Linux uses one million CLOCKS_PER_SEC.
            if unsafe { clock() } == -1 {
                return Err(Error::new(
                    Kind::RuntimeError,
                    "the processor time used is not available or its value cannot be represented",
                ));
            }
            ("clock()", 1e-6)
        }
    };
    Ok((implementation, resolution))
}
