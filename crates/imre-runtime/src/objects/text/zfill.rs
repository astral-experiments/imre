//! Pad Unicode text with ASCII zeros after an optional leading ASCII sign.

use super::{Error, Int, Kind, Result, Str, StrOwner, finish};

pub(crate) fn width(index: &Int) -> Result<usize> {
    crate::signed_index(index)
        .map(|width| width.max(0) as usize)
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C ssize_t",
            )
        })
}

/// Count width in code points, but preserve the source's UTF-8 or surrogate storage units.
pub(crate) fn units<T: Copy + Eq + From<u8>>(
    source: &[T],
    characters: usize,
    width: usize,
    copy: bool,
) -> Result<Option<Vec<T>>> {
    let padding = width.saturating_sub(characters);
    if padding == 0 && (!copy || source.is_empty()) {
        return Ok(None);
    }
    let size = source
        .len()
        .checked_add(padding)
        .ok_or_else(Error::memory_error)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .map_err(|_| Error::memory_error())?;
    let sign = usize::from(
        source
            .first()
            .is_some_and(|&ch| ch == b'+'.into() || ch == b'-'.into()),
    );
    output.extend_from_slice(&source[..sign]);
    output.resize(sign + padding, b'0'.into());
    output.extend_from_slice(&source[sign..]);
    Ok(Some(output))
}

pub(crate) fn text(source: &str, width: usize, copy: bool) -> Result<Option<String>> {
    Ok(
        units(source.as_bytes(), source.chars().count(), width, copy)?.map(|bytes| {
            // SAFETY: only ASCII zeros were inserted, before the UTF-8 text or after its ASCII sign.
            unsafe { String::from_utf8_unchecked(bytes) }
        }),
    )
}

/// # Safety
/// The caller roots the string storage and tagged index. A subtype requires a fresh string even without padding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_zfill(source: *const Str, index: usize, copy: u32) -> *const Str {
    finish(
        (|| {
            // SAFETY: the caller roots the tagged index through the call.
            let index = unsafe { Int::borrow(index) };
            let width = width(&index)?;
            // SAFETY: the caller keeps the immutable Str alive for this borrow.
            let source = unsafe { &*source };
            if let Some(padded) = text(source, width, copy != 0)? {
                let owner =
                    StrOwner::try_new(Str::from(padded)).map_err(|_| Error::memory_error())?;
                Ok(StrOwner::into_raw(owner))
            } else {
                // SAFETY: an unchanged exact string or canonical empty string retains its storage.
                unsafe { crate::imre_str_retain(source) };
                Ok(source as *const Str)
            }
        })(),
        std::ptr::null(),
    )
}
