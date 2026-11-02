//! Linux clock selection and resolution for time.get_clock_info.

use super::{Error, Kind};
use crate::objects::errno_error;
use std::{io, mem::MaybeUninit};

pub(crate) struct ClockInfo {
    pub(crate) implementation: &'static str,
    pub(crate) monotonic: bool,
    pub(crate) resolution: f64,
}

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

pub(crate) fn clock_info(name: &[u8]) -> Result<ClockInfo, Error> {
    let (id, implementation) = match name {
        b"time" => (libc::CLOCK_REALTIME, "clock_gettime(CLOCK_REALTIME)"),
        b"monotonic" | b"perf_counter" => (libc::CLOCK_MONOTONIC, "clock_gettime(CLOCK_MONOTONIC)"),
        b"process_time" => (
            libc::CLOCK_PROCESS_CPUTIME_ID,
            "clock_gettime(CLOCK_PROCESS_CPUTIME_ID)",
        ),
        b"thread_time" => (
            libc::CLOCK_THREAD_CPUTIME_ID,
            "clock_gettime(CLOCK_THREAD_CPUTIME_ID)",
        ),
        _ => return Err(Error::new(Kind::ValueError, "unknown clock")),
    };
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
    let mut res = MaybeUninit::uninit();
    // SAFETY: clock_getres initializes the timespec on success.
    let resolution = if unsafe { libc::clock_getres(id, res.as_mut_ptr()) } == 0 {
        // SAFETY: clock_getres returned success.
        let res = unsafe { res.assume_init() };
        res.tv_sec as f64 + res.tv_nsec as f64 * 1e-9
    } else if id == libc::CLOCK_REALTIME {
        1e-9
    } else {
        return Err(errno_error(io::Error::last_os_error()));
    };
    if cpu {
        check()?;
    }
    Ok(ClockInfo {
        implementation,
        monotonic: id != libc::CLOCK_REALTIME,
        resolution,
    })
}

/// Try the remaining POSIX process CPU clocks in the same order as CPython.
fn process_fallback() -> Result<ClockInfo, Error> {
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
    Ok(ClockInfo {
        implementation,
        monotonic: true,
        resolution,
    })
}
