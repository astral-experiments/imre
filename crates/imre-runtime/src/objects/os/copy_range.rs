use super::{Error, Int, Kind, RangeTransfer, Result, c_int, finish};

unsafe fn count_word(word: usize) -> Result<usize> {
    // SAFETY: the caller roots a native integer.
    let count = unsafe { Int::borrow(word) };
    let value = crate::signed_index(&count).ok_or_else(|| {
        Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C ssize_t",
        )
    })?;
    usize::try_from(value)
        .map_err(|_| Error::new(Kind::ValueError, "negative value for 'count' not allowed"))
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
            RangeTransfer::CopyFileRange
                .run(
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
