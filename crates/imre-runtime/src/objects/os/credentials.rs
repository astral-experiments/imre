//! Native POSIX credential conversion and process-local system calls.

use super::{Int, checked, finish};
use crate::objects::exceptions::{Error, Kind, Result};
use num_bigint::Sign;
use num_traits::ToPrimitive;

/// The argument converter first tries a signed long. CPython distinguishes
/// truncation there from a value too large for an unsigned long.
pub(crate) fn credential_id(value: &Int, group: bool) -> Result<u64> {
    let label = if group { "gid" } else { "uid" };
    let max = if group {
        libc::gid_t::MAX as u64
    } else {
        libc::uid_t::MAX as u64
    };
    let signed = value.as_small().or_else(|| value.big().to_i64());
    let under = || Error::new(Kind::OverflowError, format!("{label} is less than minimum"));
    let over = || {
        Error::new(
            Kind::OverflowError,
            format!("{label} is greater than maximum"),
        )
    };
    let raw = if let Some(value) = signed.and_then(|n| libc::c_long::try_from(n).ok()) {
        if value == -1 {
            max
        } else {
            u64::try_from(value)
                .ok()
                .filter(|n| *n <= max)
                .ok_or_else(under)?
        }
    } else {
        let n = value.big();
        if n.sign() == Sign::Minus {
            return Err(under());
        }
        n.to_u64().filter(|n| *n < max).ok_or_else(over)?
    };
    Ok(raw)
}

/// Convert the integer index before any later credential argument is converted.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_credential_id(word: usize, group: i32) -> usize {
    // SAFETY: the caller owns the checked Python integer until conversion finishes.
    let value = unsafe { Int::borrow(word) };
    finish(
        credential_id(&value, group != 0).map(|raw| Int::from_usize(raw as usize).into_raw()),
        0,
    )
}

fn result_id(raw: u64, max: u64) -> Int {
    if raw == max {
        Int::small(-1).unwrap()
    } else {
        Int::from_usize(raw as usize)
    }
}

macro_rules! getter {
    ($name:ident, $sys:ident, $id:ty) => {
        pub(crate) fn $sys() -> Int {
            // SAFETY: POSIX identity getters take no arguments and cannot fail.
            result_id(unsafe { libc::$sys() } as u64, <$id>::MAX as u64)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn $name() -> usize {
            $sys().into_raw()
        }
    };
}
getter!(imre_os_getuid, getuid, libc::uid_t);
getter!(imre_os_geteuid, geteuid, libc::uid_t);
getter!(imre_os_getgid, getgid, libc::gid_t);
getter!(imre_os_getegid, getegid, libc::gid_t);

/// Read the supplementary groups of the current process, preserving the kernel's order.
pub(crate) fn getgroups() -> Result<Vec<Int>> {
    // SAFETY: a zero-sized query accepts a null buffer.
    let count = checked(unsafe { libc::getgroups(0, std::ptr::null_mut()) })?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut groups = Vec::new();
    groups
        .try_reserve_exact(count as usize)
        .map_err(|_| Error::memory_error())?;
    groups.resize(count as usize, 0);
    // SAFETY: the kernel can fill at most count gid_t entries in the live buffer.
    let filled = checked(unsafe { libc::getgroups(count, groups.as_mut_ptr()) })?;
    Ok(groups
        .into_iter()
        .take(filled as usize)
        .map(|gid| result_id(gid as u64, libc::gid_t::MAX as u64))
        .collect())
}

macro_rules! setter {
    ($name:ident, $sys:ident, $id:ty, $($arg:ident),+) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: usize),+) {
            // SAFETY: the converter returns tagged, range-checked uid_t/gid_t
            // values. The kernel enforces the caller's actual privileges.
            finish(checked(unsafe { libc::$sys($(($arg >> 1) as $id),+) }).map(|_| ()), ());
        }
    };
}
setter!(imre_os_setuid, setuid, libc::uid_t, id);
setter!(imre_os_seteuid, seteuid, libc::uid_t, id);
setter!(imre_os_setgid, setgid, libc::gid_t, id);
setter!(imre_os_setegid, setegid, libc::gid_t, id);
setter!(imre_os_setreuid, setreuid, libc::uid_t, real, effective);
setter!(imre_os_setregid, setregid, libc::gid_t, real, effective);

#[cfg(target_os = "linux")]
mod saved {
    use super::*;

    macro_rules! getter3 {
        ($name:ident, $sys:ident, $id:ty) => {
            /// # Safety
            /// The caller owns three writable tuple slots.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $name(out: *mut usize) {
                let (mut real, mut effective, mut saved) = (0, 0, 0);
                // SAFETY: all three ID slots live across the syscall, and out
                // is written only after the kernel returned successfully.
                unsafe {
                    finish(
                        checked(libc::$sys(&mut real, &mut effective, &mut saved)).map(|_| {
                            for (i, id) in [real, effective, saved].into_iter().enumerate() {
                                out.add(i)
                                    .write(result_id(id as u64, <$id>::MAX as u64).into_raw());
                            }
                        }),
                        (),
                    );
                }
            }
        };
    }
    getter3!(imre_os_getresuid, getresuid, libc::uid_t);
    getter3!(imre_os_getresgid, getresgid, libc::gid_t);
    setter!(
        imre_os_setresuid,
        setresuid,
        libc::uid_t,
        real,
        effective,
        saved
    );
    setter!(
        imre_os_setresgid,
        setresgid,
        libc::gid_t,
        real,
        effective,
        saved
    );
}
