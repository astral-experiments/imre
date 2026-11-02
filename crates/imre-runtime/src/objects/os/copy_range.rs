use super::{Error, Int, Kind, Result, c_int, finish, transfer, transferred};

pub(crate) fn copy_count(count: isize) -> Result<usize> {
    usize::try_from(count)
        .map_err(|_| Error::new(Kind::ValueError, "negative value for 'count' not allowed"))
}

/// Copy between borrowed Linux file descriptors. Addressable offsets stay live across EINTR.
pub(crate) fn copy_file_range<E: From<Error>>(
    src: i32,
    dst: i32,
    count: usize,
    mut offset_src: Option<i64>,
    mut offset_dst: Option<i64>,
    dispatch: impl FnMut() -> std::result::Result<(), E>,
) -> std::result::Result<usize, E> {
    crate::initialize_io();
    let offset_src = offset_src
        .as_mut()
        .map_or(std::ptr::null_mut(), std::ptr::from_mut);
    let offset_dst = offset_dst
        .as_mut()
        .map_or(std::ptr::null_mut(), std::ptr::from_mut);
    transfer(
        || {
            // SAFETY: each non-null pointer is a separate writable loff_t for the duration of
            // the syscall. The kernel validates the raw descriptors and retains no pointers.
            transferred(unsafe {
                libc::syscall(
                    libc::SYS_copy_file_range,
                    src,
                    offset_src,
                    dst,
                    offset_dst,
                    count,
                    0u32,
                )
            } as isize)
        },
        dispatch,
    )
}

unsafe fn count_word(word: usize) -> Result<usize> {
    // SAFETY: the caller roots a native integer.
    let count = unsafe { Int::borrow(word) };
    let value = crate::signed_index(&count).ok_or_else(|| {
        Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C ssize_t",
        )
    })?;
    copy_count(value)
}

unsafe fn offset_word(word: usize) -> Result<Option<i64>> {
    if word == 0 {
        return Ok(None);
    }
    // SAFETY: a nonzero optional integer word is a rooted native integer.
    unsafe { Int::borrow(word) }
        .to_i64()
        .map(Some)
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C long",
            )
        })
}

/// Check the converted count before calling either offset's Python index method.
///
/// # Safety
/// The caller roots the native integer word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_os_check_copy_count(word: usize) {
    // SAFETY: guaranteed by the caller.
    finish(unsafe { count_word(word) }.map(|_| ()), ());
}

/// Check a converted source offset before calling the destination offset's index method.
///
/// # Safety
/// The caller roots the optional integer word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_os_check_copy_offset(word: usize) {
    // SAFETY: guaranteed by the caller.
    finish(unsafe { offset_word(word) }.map(|_| ()), ());
}

/// # Safety
/// The caller roots the descriptors, count, and optional integer offsets for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_os_copy_file_range(
    src: usize,
    dst: usize,
    count: usize,
    offset_src: usize,
    offset_dst: usize,
) -> usize {
    finish(
        (|| {
            let src = c_int(src)?;
            let dst = c_int(dst)?;
            // SAFETY: the count and both optional integer words are rooted by the caller.
            let (count, offset_src, offset_dst) = unsafe {
                (
                    count_word(count)?,
                    offset_word(offset_src)?,
                    offset_word(offset_dst)?,
                )
            };
            copy_file_range(
                src,
                dst,
                count,
                offset_src,
                offset_dst,
                super::super::stdlib::check_interrupt,
            )
            .map(|n| Int::from_usize(n).into_raw())
        })(),
        0,
    )
}
