//! POSIX descriptors are integers in the program's descriptor table. A call
//! can observe a closed number or a number subsequently reused by the kernel.

mod credentials;

use super::exceptions::{Error, Filename, Kind, Result, finish};
use super::files::{errno_error, errno_with_filenames};
use super::{Int, Str, c_int};
use crate::StrOwner;
use num_traits::ToPrimitive;
use std::ffi::CString;
use std::io;

pub(super) mod links;
pub(super) mod stat_result;

/// The target's primary or alternate separator, shared by typed and boxed imports.
pub(crate) fn os_separator(alternate: bool) -> Option<&'static str> {
    match (cfg!(windows), alternate) {
        (false, true) => None,
        (true, false) => Some(r"\"),
        _ => Some("/"),
    }
}

/// Return the target OS separator as an interned one-character string, or None.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_separator(alternate: u32) -> *const Str {
    let Some(sep) = os_separator(alternate != 0) else {
        return std::ptr::null();
    };
    separator_string(sep)
}

/// Return the target search-path separator as an interned one-character string.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_pathsep() -> *const Str {
    separator_string(if cfg!(windows) { ";" } else { ":" })
}

fn separator_string(sep: &str) -> *const Str {
    finish(
        super::text::try_owned_string(sep).map(StrOwner::into_raw),
        std::ptr::null(),
    )
}

/// Query the same POSIX metadata for Path views, fresh predicates, and metadata copies.
pub(super) fn metadata(
    path: impl AsRef<std::path::Path>,
    follow: bool,
) -> io::Result<std::fs::Metadata> {
    if follow {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    }
}

/// Snapshot names and the kernel's directory-entry types without looking up an inode.
pub(super) fn directory_entry_types(
    root: &str,
) -> io::Result<Vec<(std::ffi::OsString, rustix::fs::FileType)>> {
    use rustix::fs::{Dir, Mode, OFlags, open};
    use std::os::unix::ffi::OsStringExt;

    let fd = open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut entries = Vec::new();
    for item in Dir::new(fd)? {
        let entry = item?;
        let name = entry.file_name().to_bytes();
        if !matches!(name, b"." | b"..") {
            entries.push((
                std::ffi::OsString::from_vec(name.to_vec()),
                entry.file_type(),
            ));
        }
    }
    Ok(entries)
}

macro_rules! descriptor_constants {
    ($($name:ident = $flag:ident),* $(,)?) => {$(
        #[unsafe(no_mangle)]
        pub static $name: usize = ((libc::$flag as usize) << 1) | 1;
    )*};
}
descriptor_constants! {
    imre_os_o_rdonly = O_RDONLY, imre_os_o_wronly = O_WRONLY, imre_os_o_rdwr = O_RDWR,
    imre_os_o_append = O_APPEND, imre_os_o_creat = O_CREAT, imre_os_o_excl = O_EXCL,
    imre_os_o_trunc = O_TRUNC, imre_os_o_cloexec = O_CLOEXEC, imre_os_o_directory = O_DIRECTORY,
    imre_os_o_nofollow = O_NOFOLLOW, imre_os_o_nonblock = O_NONBLOCK,
    imre_os_seek_set = SEEK_SET, imre_os_seek_cur = SEEK_CUR, imre_os_seek_end = SEEK_END,
    imre_os_seek_data = SEEK_DATA, imre_os_seek_hole = SEEK_HOLE,
    imre_os_pollin = POLLIN, imre_os_pollpri = POLLPRI, imre_os_pollout = POLLOUT,
    imre_os_pollerr = POLLERR, imre_os_pollhup = POLLHUP, imre_os_pollnval = POLLNVAL,
}

/// Return a kernel path and the filesystem-protocol value to keep on OSError.
///
/// # Safety
/// `value` is the rooted str, bytes or Path selected by `tag`.
unsafe fn syscall_path(
    value: *const (),
    tag: u32,
    function: &str,
    parameter: &str,
) -> Result<(CString, Filename)> {
    let terminated = |bytes: &[u8]| {
        CString::new(bytes).map_err(|_| {
            Error::new(
                Kind::ValueError,
                format!("{function}: embedded null character in {parameter}"),
            )
        })
    };
    if tag == 1 {
        // SAFETY: the caller roots this bytes handle through conversion.
        let bytes: crate::BytesOwner = unsafe { crate::acquire_bytes(value as usize) };
        return Ok((terminated(&bytes)?, Filename::Bytes(bytes)));
    }
    // SAFETY: exact str is borrowed; Path.__fspath__ supplies an owned str.
    let text = unsafe {
        match tag {
            0 => crate::StrOwner::acquire_word(value as usize),
            2 => StrOwner::from_raw(super::paths::imre_path_text(value.cast())),
            _ => unreachable!(),
        }
    };
    Ok((terminated(text.as_bytes())?, Filename::Str(text)))
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
    finish(
        (|| {
            // SAFETY: the caller's representation tag describes this rooted path.
            let (name, path) = unsafe { syscall_path(path, bytes, "open", "path") }?;
            let flags = c_int(flags)? | libc::O_CLOEXEC;
            let mode = c_int(mode)?;
            // SAFETY: dir_fd is a rooted optional integer according to the ABI.
            let dir_fd = unsafe { directory_fd(dir_fd) }?;
            loop {
                // SAFETY: the path is terminated and the variadic mode is promoted to int.
                let fd = unsafe { libc::openat(dir_fd, name.as_ptr(), flags, mode) };
                if fd >= 0 {
                    return Ok(Int::from_usize(fd as usize).into_raw());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(errno_with_filenames(error, Some(path), None));
                }
            }
        })(),
        0,
    )
}

/// Remove the entry itself, using the raw spelling even for trailing slashes.
/// Validate the path before converting the optional directory descriptor.
///
/// # Safety
/// The caller roots the path for its static tag and the optional integer word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_os_unlink(value: *const (), tag: u32, fd: usize, remove: u32) {
    let result = (|| {
        let function = if remove == 0 { "unlink" } else { "remove" };
        // SAFETY: the path's owner stays live through the syscall and error rendering.
        let (path, name) = unsafe { syscall_path(value, tag, function, "path") }?;
        // SAFETY: fd is a rooted optional integer.
        let dir_fd = unsafe { directory_fd(fd) }?;
        // SAFETY: the zero-terminated path is owned until unlinkat returns.
        if unsafe { libc::unlinkat(dir_fd, path.as_ptr(), 0) } == -1 {
            return Err(errno_with_filenames(
                io::Error::last_os_error(),
                Some(name),
                None,
            ));
        }
        Ok(())
    })();
    finish(result, ());
}

/// Convert a rooted optional directory descriptor after validating the path.
unsafe fn directory_fd(fd: usize) -> Result<libc::c_int> {
    Ok(if fd == 0 {
        libc::AT_FDCWD
    } else {
        // SAFETY: a nonzero optional integer has the native tagged-int ABI.
        let integer = unsafe { Int::borrow(fd) };
        let n = integer.big();
        n.to_i32().ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                if n.sign() == num_bigint::Sign::Minus {
                    "fd is less than minimum"
                } else {
                    "fd is greater than maximum"
                },
            )
        })?
    })
}

fn checked(ret: libc::c_int) -> Result<libc::c_int> {
    if ret < 0 {
        Err(errno_error(io::Error::last_os_error()))
    } else {
        Ok(ret)
    }
}

/// Like strerror(3), describe known and unknown target errno values.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_strerror(code: usize) -> *const Str {
    finish(
        c_int(code).and_then(|code| {
            // SAFETY: copy libc's terminated string before any further calls.
            let message = unsafe { libc::strerror(code) };
            if message.is_null() {
                return Err(Error::new(
                    Kind::ValueError,
                    "strerror() argument out of range",
                ));
            }
            // SAFETY: the non-null pointer returned above remains live during this copy.
            let text = unsafe { std::ffi::CStr::from_ptr(message) }.to_string_lossy();
            Ok(StrOwner::into_raw(StrOwner::new(Str::from(text.as_ref()))))
        }),
        std::ptr::null(),
    )
}

/// Move the shared open-file offset, retaining the kernel's errors for pipes,
/// closed descriptors, and unsupported sparse-file seeks.
///
/// # Safety
/// All arguments are rooted tagged Python integers for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_os_lseek(fd: usize, position: usize, whence: usize) -> usize {
    finish(
        (|| {
            let fd = c_int(fd)?;
            // SAFETY: the compiler retains the exact integer for the call.
            let position = unsafe { Int::borrow(position) };
            let position = position
                .as_small()
                .or_else(|| position.big().to_i64())
                .ok_or_else(|| {
                    Error::new(
                        Kind::OverflowError,
                        "Python int too large to convert to C long",
                    )
                })?;
            let whence = c_int(whence)?;
            // SAFETY: lseek takes integer values and retains no pointers.
            let offset = unsafe { libc::lseek(fd, position, whence) };
            if offset < 0 {
                Err(errno_error(io::Error::last_os_error()))
            } else {
                Ok(Int::from_usize(offset as usize).into_raw())
            }
        })(),
        0,
    )
}

/// Close exactly once: retrying an interrupted close can close a reused fd.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_close(fd: usize) {
    finish(c_int(fd).and_then(close), ());
}

/// Close the descriptor once, including when the kernel reports an interrupt.
pub(crate) fn close(fd: i32) -> Result<()> {
    // SAFETY: close accepts any descriptor number, including an invalid one.
    checked(unsafe { libc::close(fd) }).map(|_| ())
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
    // SAFETY: the caller supplies the two uninitialized integer slots.
    finish(unsafe { make_pipe(out, libc::O_CLOEXEC) }, ());
}

/// Atomically create a pipe with the caller's flags, including inheritable ends for zero.
///
/// # Safety
/// The caller supplies two writable word slots in a fresh tuple.
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_os_pipe2(out: *mut usize, flags: usize) {
    // SAFETY: no descriptor is published until both ends were created successfully.
    finish(
        c_int(flags).and_then(|flags| unsafe { make_pipe(out, flags) }),
        (),
    );
}

/// Initialize the caller's two tuple slots only after establishing the requested flags.
unsafe fn make_pipe(out: *mut usize, flags: libc::c_int) -> Result<()> {
    let fds = pipe(flags)?;
    for (index, fd) in fds.into_iter().enumerate() {
        // SAFETY: each initialized slot takes one immediate integer.
        unsafe {
            out.add(index)
                .write(Int::from_usize(fd as usize).into_raw())
        };
    }
    Ok(())
}

/// Return both descriptors after establishing the requested flags on each end.
pub(crate) fn pipe(flags: libc::c_int) -> Result<[i32; 2]> {
    let mut fds = [-1; 2];
    // SAFETY: both pipe variants initialize the two writable int slots.
    let result = unsafe {
        #[cfg(target_os = "linux")]
        {
            libc::pipe2(fds.as_mut_ptr(), flags)
        }
        #[cfg(not(target_os = "linux"))]
        {
            libc::pipe(fds.as_mut_ptr())
        }
    };
    checked(result)?;
    #[cfg(not(target_os = "linux"))]
    if flags & libc::O_CLOEXEC != 0
        && let Err(error) = set_cloexec(fds[0]).and_then(|()| set_cloexec(fds[1]))
    {
        // SAFETY: the fresh pipe owns both descriptors until publication.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        return Err(error);
    }
    Ok(fds)
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

pub(crate) struct Flag {
    get: i32,
    set: i32,
    mask: i32,
}
pub(crate) const BLOCKING: Flag = Flag {
    get: libc::F_GETFL,
    set: libc::F_SETFL,
    mask: libc::O_NONBLOCK,
};
pub(crate) const INHERITABLE: Flag = Flag {
    get: libc::F_GETFD,
    set: libc::F_SETFD,
    mask: libc::FD_CLOEXEC,
};

impl Flag {
    fn get(&self, fd: i32) -> Result<i32> {
        // SAFETY: F_GETFL and F_GETFD take no third argument and validate the fd.
        checked(unsafe { libc::fcntl(fd, self.get) })
    }

    /// Read the Python-facing flag, which is the inverse of its kernel mask.
    pub(crate) fn enabled(&self, fd: i32) -> Result<bool> {
        Ok(self.get(fd)? & self.mask == 0)
    }

    /// Change only this flag, preserving all unrelated descriptor or status flags.
    pub(crate) fn update(&self, fd: i32, enabled: bool) -> Result<()> {
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
    finish(
        c_int(fd).and_then(|fd| BLOCKING.enabled(fd)).map(i32::from),
        0,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_os_set_blocking(fd: usize, value: usize) {
    finish(
        c_int(fd).and_then(|fd| BLOCKING.update(fd, c_int(value)? != 0)),
        (),
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_os_get_inheritable(fd: usize) -> i32 {
    finish(
        c_int(fd)
            .and_then(|fd| INHERITABLE.enabled(fd))
            .map(i32::from),
        0,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_os_set_inheritable(fd: usize, value: usize) {
    finish(
        c_int(fd).and_then(|fd| INHERITABLE.update(fd, c_int(value)? != 0)),
        (),
    );
}
