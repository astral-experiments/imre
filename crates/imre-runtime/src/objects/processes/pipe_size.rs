//! Set the capacity of newly created subprocess pipes before fork.

use super::*;

/// Validate the Python argument, then apply fcntl's unsigned-int conversion
/// only to positive values. A positive multiple of 2³² still requests size 0.
///
/// # Safety
/// The caller roots the original value with this representation throughout.
pub(super) unsafe fn capacity(mut word: usize, mut kind: WordKind) -> Result<Option<u32>> {
    if kind.raw() != kind.base() {
        if word == 0 {
            return Ok(None);
        }
        if kind.base() == 1 {
            word -= 1;
        }
    }
    if kind.base() == 9 {
        (word, kind) = crate::objects::unions::member(word);
    }
    match kind.base() {
        0 => Ok(None),
        1 => Ok((word != 0).then_some(1)),
        2 => {
            // SAFETY: WordKind identified an exact rooted Python integer.
            let value = unsafe { Int::borrow(word) };
            if let Some(n) = value.as_small() {
                return Ok((n > 0).then_some(n as u32));
            }
            let n = value.big();
            Ok((n.sign() == num_bigint::Sign::Plus)
                .then(|| n.magnitude().iter_u32_digits().next().unwrap_or(0)))
        }
        _ => Err(Error::new(Kind::TypeError, "pipesize must be an integer")),
    }
}

/// Create both CLOEXEC ends and apply the requested Linux capacity before a child can start.
pub(super) fn open(size: Option<u32>) -> io::Result<(io::PipeReader, io::PipeWriter)> {
    let (read, write) = io::pipe()?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Some(size) = size {
        loop {
            // SAFETY: the write end is open; F_SETPIPE_SZ accepts a C int.
            if unsafe { libc::fcntl(write.as_raw_fd(), libc::F_SETPIPE_SZ, size as libc::c_int) }
                >= 0
            {
                break;
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let _ = size;
    Ok((read, write))
}
