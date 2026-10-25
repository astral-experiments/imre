//! Copy directly between open descriptions with the offset and fallback rules of shutil.
use std::cell::Cell;
use std::io;
use std::os::fd::RawFd;
use std::rc::Rc;

use super::super::exceptions::{Error, Result, finish, Filename};
use super::super::files::{errno_error, errno_with_filenames};
use super::super::{Int, Object, Str, object};

pub(super) const RANGE: u32 = 0;
pub(super) const SENDFILE: u32 = 1;
pub(super) const FCOPYFILE: u32 = 2;

thread_local! {
    static FLAGS: Cell<[bool; 3]> = const { Cell::new([
        cfg!(target_os = "linux"), cfg!(any(target_os = "linux", target_os = "android")),
        cfg!(target_os = "macos"),
    ]) };
}

pub(super) fn enabled(method: u32) -> bool {
    FLAGS.with(|flags| flags.get()[method as usize])
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_shutil_fastcopy_flag(method: u32) -> u32 {
    u32::from(enabled(method))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_shutil_set_fastcopy_flag(method: u32, value: u32) {
    FLAGS.with(|flags| {
        let mut state = flags.get();
        state[method as usize] = value != 0;
        flags.set(state);
    });
}

fn blocksize(fd: RawFd) -> usize {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat initializes this single struct on success, without retaining it.
    let size = if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } == 0 {
        // SAFETY: fstat succeeded; negative sizes cannot lower the minimum.
        unsafe { stat.assume_init() }.st_size.max(1 << 23) as u64
    } else {
        1 << 27
    };
    if usize::BITS < 64 {
        size.min(1 << 30) as usize
    } else {
        size as usize
    }
}

/// Return one transfer's progress; explicit offsets must not move the corresponding descriptor.
fn transfer(method: u32, src: RawFd, dst: RawFd, offset: i64, size: usize) -> io::Result<usize> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut at = offset as libc::off64_t;
        // SAFETY: both kernel operations borrow descriptors and a live offset for one call.
        let n = unsafe {
            match method {
                #[cfg(target_os = "linux")]
                RANGE => libc::copy_file_range(src, std::ptr::null_mut(), dst, &mut at, size, 0),
                SENDFILE => libc::sendfile64(dst, src, &mut at, size),
                _ => return Err(io::Error::from_raw_os_error(libc::ENOSYS)),
            }
        };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = (method, src, dst, offset, size);
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }
}

pub(super) fn copy_fds(
    method: u32,
    src: RawFd,
    dst: RawFd,
    flags: u32,
    error: impl Fn(io::Error) -> Error,
) -> Result<()> {
    if method == FCOPYFILE {
        #[cfg(target_os = "macos")]
        {
            // SAFETY: null state selects library-managed state; fd ownership is retained.
            if unsafe { libc::fcopyfile(src, dst, std::ptr::null_mut(), flags) } == 0 {
                return Ok(());
            }
            let failure = io::Error::last_os_error();
            let fallback = matches!(failure.raw_os_error(), Some(libc::EINVAL | libc::ENOTSUP));
            let e = error(failure);
            return Err(if fallback {
                Error::fastcopy_giveup(Some(e))
            } else {
                e
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = flags;
            return Err(Error::fastcopy_giveup(Some(error(
                io::Error::from_raw_os_error(libc::ENOSYS),
            ))));
        }
    }
    let size = blocksize(src);
    let mut offset = 0;
    loop {
        match transfer(method, src, dst, offset, size) {
            Ok(0) if method == RANGE && offset == 0 => return Err(Error::fastcopy_giveup(None)),
            Ok(0) => return Ok(()),
            Ok(n) => offset += n as i64,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let code = e.raw_os_error();
                let error = error(e);
                if method == SENDFILE && code == Some(libc::ENOTSOCK) {
                    imre_shutil_set_fastcopy_flag(SENDFILE, 0);
                    return Err(Error::fastcopy_giveup(Some(error)));
                }
                if code != Some(libc::ENOSPC) && offset == 0 {
                    // SAFETY: lseek with zero displacement observes the current offset.
                    let position = unsafe { libc::lseek(dst, 0, libc::SEEK_CUR) };
                    if position < 0 {
                        return Err(errno_error(io::Error::last_os_error()));
                    }
                    if position == 0 {
                        return Err(Error::fastcopy_giveup(Some(error)));
                    }
                }
                return Err(error);
            }
        }
    }
}

/// # Safety
/// Both values are rooted file objects. Borrowing a descriptor leaves both streams open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_fastcopy(
    method: u32,
    src: *const Object,
    dst: *const Object,
    flags: usize,
) {
    let file = |ptr| -> Result<(RawFd, Option<Rc<Str>>)> {
        // SAFETY: the caller keeps the stream and its underlying descriptor alive across the copy.
        unsafe {
            object(ptr).0.with(|s| {
                let f = s.file();
                Ok((
                    f.fileno()
                        .map_err(|e| Error::fastcopy_giveup(Some(e)))?,
                    f.name.clone(),
                ))
            })
        }
    };
    let result = (|| {
        let (src, from) = file(src)?;
        let (dst, to) = file(dst)?;
        let flags = if method == FCOPYFILE {
            // SAFETY: the third operand for fcopyfile is a rooted language integer.
            unsafe { Int::borrow(flags) }
                .as_small()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| {
                    Error::new(
                        super::super::exceptions::Kind::OverflowError,
                        "Python int too large to convert to C int",
                    )
                })? as u32
        } else {
            0
        };
        copy_fds(method, src, dst, flags, |e| {
            errno_with_filenames(e, from.clone().map(Filename::Str), to.clone().map(Filename::Str))
        })
    })();
    finish(result, ());
}

/// # Safety
/// The compiler roots a language integer for the duration of the descriptor conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_fastcopy_blocksize(fd: usize) -> usize {
    // SAFETY: caller retains this exact integer word.
    let value = unsafe { Int::borrow(fd) };
    let n = value.as_small().and_then(|n| i32::try_from(n).ok());
    match n {
        Some(fd) => Int::from_usize(blocksize(fd)).into_raw(),
        None => finish(
            Err(Error::new(
                super::super::exceptions::Kind::OverflowError,
                "Python int too large to convert to C int",
            )),
            0,
        ),
    }
}
