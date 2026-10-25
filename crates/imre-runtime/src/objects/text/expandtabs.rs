//! Expand tab stops in Unicode code points. Only CR and LF reset columns.

use super::super::{Str, c_int};
use super::{Error, Kind, Result, finish, owned_string};
use crate::StrOwner;

/// A prior tab always leaves the column divisible by its width. Only the
/// part after the last CR/LF in this unchanged span affects the next stop.
fn spans(source: &str, width: usize) -> impl Iterator<Item = (&str, usize)> {
    source.split_inclusive('\t').map(move |part| {
        let Some(text) = part.strip_suffix('\t') else {
            return (part, 0);
        };
        let spaces = if width == 0 {
            0
        } else {
            let line = text.rsplit(['\r', '\n']).next().unwrap();
            width - line.chars().count() % width
        };
        (text, spaces)
    })
}

/// Check the Python character count separately from our UTF-8 storage size.
fn output_size(source: &str, width: usize, max_chars: usize) -> Result<usize> {
    let mut chars = 0usize;
    let mut bytes = 0usize;
    for (text, spaces) in spans(source, width) {
        chars = chars
            .checked_add(text.chars().count())
            .and_then(|n| n.checked_add(spaces))
            .filter(|n| *n <= max_chars)
            .ok_or_else(|| Error::new(Kind::OverflowError, "new string is too long"))?;
        bytes = bytes
            .checked_add(text.len())
            .and_then(|n| n.checked_add(spaces))
            .ok_or_else(Error::memory_error)?;
    }
    Ok(bytes)
}

/// # Safety
/// The caller keeps the string and tagged Python integer rooted through the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_expandtabs(source: *const Str, tabsize: usize) -> *const Str {
    finish(
        (|| {
            // CPython converts to C int before even a no-tab or empty-string result.
            let width = c_int(tabsize)?.max(0) as usize;
            // SAFETY: the caller holds this immutable string.
            let text = unsafe { &*source };
            if !text.as_bytes().contains(&b'\t') {
                // SAFETY: transfer one additional reference to the original owner.
                unsafe { crate::imre_str_retain(source) };
                return Ok(source);
            }
            let bytes = output_size(text, width, isize::MAX as usize)?;
            let mut result = Str::try_with_capacity(bytes).map_err(|_| Error::memory_error())?;
            const SPACES: &str = "                                                                ";
            for (part, mut spaces) in spans(text, width) {
                result.push_str(part);
                while spaces >= SPACES.len() {
                    result.push_str(SPACES);
                    spaces -= SPACES.len();
                }
                result.push_str(&SPACES[..spaces]);
            }
            let handle = if result.len() <= 2 {
                owned_string(&result)
            } else {
                StrOwner::new(result)
            };
            Ok(StrOwner::into_raw(handle))
        })(),
        std::ptr::null(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_overflow_and_utf8_storage_have_separate_bounds() {
        assert_eq!(output_size("é\t☀", 4, 5).ok(), Some(8));
        assert!(output_size("é\t☀", 4, 4).is_err());
    }
}
