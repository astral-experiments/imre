//! POSIX descriptors are integers in the program's descriptor table. A call
//! can observe a closed number or a number subsequently reused by the kernel.

use super::exceptions::{Error, Kind, Result, finish};
use super::files::errno_error;
use super::{Bytes, Int, Str};
use std::ffi::CString;
use std::io;

macro_rules! open_flags {
    ($($name:ident = $flag:ident),* $(,)?) => {$(
        #[unsafe(no_mangle)]
        pub static $name: usize = ((libc::$flag as usize) << 1) | 1;
    )*};
}
open_flags! {
    imre_os_o_rdonly = O_RDONLY, imre_os_o_wronly = O_WRONLY, imre_os_o_rdwr = O_RDWR,
    imre_os_o_append = O_APPEND, imre_os_o_creat = O_CREAT, imre_os_o_excl = O_EXCL,
    imre_os_o_trunc = O_TRUNC, imre_os_o_cloexec = O_CLOEXEC, imre_os_o_directory = O_DIRECTORY,
    imre_os_o_nofollow = O_NOFOLLOW, imre_os_o_nonblock = O_NONBLOCK,
}

/// Open relative to an optional directory fd. Set close-on-exec atomically.
///
/// # Safety
/// The caller roots an exact Bytes (tag 1) or Str (tag 0) for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_os_open(
    path: *const (),
    bytes: u32,
    flags: usize,
    mode: usize,
    dir_fd: usize,
) -> usize {
    // SAFETY: the caller's representation tag describes this rooted path.
    let path: &[u8] = unsafe {
        if bytes == 0 {
            (&*path.cast::<Str>()).as_bytes()
        } else {
            &*path.cast::<Bytes>()
        }
    };
    finish(
        (|| {
            let name = CString::new(path).map_err(|_| {
                Error::new(Kind::ValueError, "open: embedded null character in path")
            })?;
            let flags = c_int(flags)? | libc::O_CLOEXEC;
            let mode = c_int(mode)?;
            let dir_fd = if dir_fd == 0 {
                libc::AT_FDCWD
            } else {
                c_int_value(dir_fd).ok_or_else(|| {
                    // SAFETY: a present dir_fd is a rooted canonical integer.
                    let fd = unsafe { Int::borrow(dir_fd) };
                    let message = if fd.big().sign() == num_bigint::Sign::Minus {
                        "fd is less than minimum"
                    } else {
                        "fd is greater than maximum"
                    };
                    Error::new(Kind::OverflowError, message)
                })?
            };
            loop {
                // SAFETY: the path is terminated and the variadic mode is promoted to int.
                let fd = unsafe { libc::openat(dir_fd, name.as_ptr(), flags, mode) };
                if fd >= 0 {
                    return Ok(Int::from_usize(fd as usize).into_raw());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(super::files::errno_with_paths(error, |message| {
                        message.push_str(": ");
                        if bytes == 0 {
                            super::exceptions::push_string_repr(
                                std::str::from_utf8(path).expect("Str is UTF-8"),
                                message,
                            );
                        } else {
                            super::bytes::write_repr(message, path);
                        }
                    }));
                }
            }
        })(),
        0,
    )
}

/// All C ints fit in the immediate representation. Heap integers cannot fit.
fn c_int_value(word: usize) -> Option<libc::c_int> {
    (word & 1 != 0)
        .then_some((word as i64) >> 1)
        .and_then(|n| libc::c_int::try_from(n).ok())
}

fn c_int(word: usize) -> Result<libc::c_int> {
    c_int_value(word).ok_or_else(|| {
        Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C int",
        )
    })
}

fn checked(ret: libc::c_int) -> Result<libc::c_int> {
    if ret < 0 {
        Err(errno_error(io::Error::last_os_error()))
    } else {
        Ok(ret)
    }
}

/// Close exactly once: retrying an interrupted close can close a reused fd.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_close(fd: usize) {
    finish(
        c_int(fd).and_then(|fd| {
            // SAFETY: close accepts any descriptor number, including an invalid one.
            checked(unsafe { libc::close(fd) }).map(|_| ())
        }),
        (),
    );
}

/// Duplicate the open file description with close-on-exec already set.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_dup(fd: usize) -> usize {
    finish(
        c_int(fd).and_then(|fd| {
            // SAFETY: F_DUPFD_CLOEXEC takes the integer lower bound for the new fd.
            checked(unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) })
                .map(|fd| Int::from_usize(fd as usize).into_raw())
        }),
        0,
    )
}

/// Duplicate onto a specified number. POSIX dup2 preserves close-on-exec for
/// identical numbers; Linux dup3 rejects identical numbers with EINVAL.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_dup2(fd: usize, target: usize, inheritable: usize) -> usize {
    finish(
        (|| {
            let (fd, target) = (c_int(fd)?, c_int(target)?);
            let inherit = c_int(inheritable)? != 0;
            // SAFETY: both calls validate the supplied descriptor numbers.
            let result = unsafe {
                #[cfg(target_os = "linux")]
                if !inherit {
                    libc::dup3(fd, target, libc::O_CLOEXEC)
                } else {
                    libc::dup2(fd, target)
                }
                #[cfg(not(target_os = "linux"))]
                libc::dup2(fd, target)
            };
            let result = checked(result)?;
            #[cfg(not(target_os = "linux"))]
            if !inherit && let Err(error) = set_cloexec(result) {
                // SAFETY: release the duplicated descriptor if setup failed.
                unsafe { libc::close(result) };
                return Err(error);
            }
            Ok(Int::from_usize(result as usize).into_raw())
        })(),
        0,
    )
}

#[cfg(not(target_os = "linux"))]
fn set_cloexec(fd: i32) -> Result<()> {
    // SAFETY: F_SETFD takes the integer descriptor flags.
    checked(unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) }).map(|_| ())
}

/// Return two non-inheritable descriptors. The caller allocates the tuple
/// before creating the pipe so a failed tuple allocation leaks neither end.
///
/// # Safety
/// The caller supplies two writable word slots in a fresh tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_os_pipe(out: *mut usize) {
    finish(
        (|| {
            let mut fds = [-1; 2];
            // SAFETY: both pipe variants initialize the two writable int slots.
            let result = unsafe {
                #[cfg(target_os = "linux")]
                {
                    libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    libc::pipe(fds.as_mut_ptr())
                }
            };
            checked(result)?;
            #[cfg(not(target_os = "linux"))]
            if let Err(error) = set_cloexec(fds[0]).and_then(|()| set_cloexec(fds[1])) {
                // SAFETY: the fresh pipe owns both descriptors until publication.
                unsafe {
                    libc::close(fds[0]);
                    libc::close(fds[1]);
                }
                return Err(error);
            }
            for (index, fd) in fds.into_iter().enumerate() {
                // SAFETY: each initialized slot takes one immediate integer.
                unsafe {
                    out.add(index)
                        .write(Int::from_usize(fd as usize).into_raw())
                };
            }
            Ok(())
        })(),
        (),
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_os_isatty(fd: usize) -> i32 {
    finish(
        c_int(fd).map(|fd| {
            // SAFETY: isatty checks the descriptor and returns zero for an invalid fd.
            unsafe { libc::isatty(fd) }
        }),
        0,
    )
}

struct Flag {
    get: i32,
    set: i32,
    mask: i32,
}
const BLOCKING: Flag = Flag {
    get: libc::F_GETFL,
    set: libc::F_SETFL,
    mask: libc::O_NONBLOCK,
};
const INHERITABLE: Flag = Flag {
    get: libc::F_GETFD,
    set: libc::F_SETFD,
    mask: libc::FD_CLOEXEC,
};

impl Flag {
    fn get(&self, fd: i32) -> Result<i32> {
        // SAFETY: F_GETFL and F_GETFD take no third argument and validate the fd.
        checked(unsafe { libc::fcntl(fd, self.get) })
    }

    fn enabled(&self, fd: usize) -> Result<i32> {
        Ok(i32::from(self.get(c_int(fd)?)? & self.mask == 0))
    }

    fn update(&self, fd: usize, enabled: usize) -> Result<()> {
        let fd = c_int(fd)?;
        let enabled = c_int(enabled)? != 0;
        let old = self.get(fd)?;
        let new = if enabled {
            old & !self.mask
        } else {
            old | self.mask
        };
        if new != old {
            // SAFETY: both commands accept an integer set of flags, preserving
            // every unrelated flag observed above.
            checked(unsafe { libc::fcntl(fd, self.set, new) })?;
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_os_get_blocking(fd: usize) -> i32 {
    finish(BLOCKING.enabled(fd), 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_os_set_blocking(fd: usize, value: usize) {
    finish(BLOCKING.update(fd, value), ());
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_os_get_inheritable(fd: usize) -> i32 {
    finish(INHERITABLE.enabled(fd), 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_os_set_inheritable(fd: usize, value: usize) {
    finish(INHERITABLE.update(fd, value), ());
}
