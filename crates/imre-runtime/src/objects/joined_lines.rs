//! Coalesces a splitlines tail when its only consumer extends a private joined list.

use super::{Boundaries, LineOffsets, Object, Str, Words, imre_str_splitlines, text};
use crate::ObjectOwner;
use crate::StrOwner;

/// Joins the remaining lines. Decline on allocation failure so that only the
/// original split and final join determine whether Python sees MemoryError.
fn joined_tail(value: &str, separator: &str, mut offsets: LineOffsets) -> Option<Str> {
    let tail = &value[offsets.start..];
    let mut chunk = Str::default();
    // An LF-only suffix delimited by LF is already the desired output.
    if separator == "\n"
        && matches!(offsets.boundaries, Boundaries::CrLf)
        && memchr::memchr(b'\r', tail.as_bytes()).is_none()
    {
        let data = tail.strip_suffix('\n').unwrap_or(tail);
        chunk.try_reserve(data.len()).ok()?;
        chunk.push_str(data);
    } else {
        let mut first = true;
        while let Some(row) = offsets.next(value) {
            let sep = if first { "" } else { separator };
            chunk.try_reserve(row.len().checked_add(sep.len())?).ok()?;
            chunk.push_str(sep);
            chunk.push_str(&value[row]);
            first = false;
        }
    }
    Some(chunk)
}

/// A nonempty [1:] becomes one joined chunk. Its first slot is never read;
/// keeping this placeholder lets the original slicing code discard it normally.
/// An absent second line still produces no chunk, including for a trailing LF.
///
/// # Safety
/// Both handles are rooted strings. The compiler proves no other observation of
/// this temporary list or its destination is possible.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_splitlines_joined_tail(
    value: *const Str,
    separator: *const Str,
) -> *const Object {
    // SAFETY: the caller holds both immutable strings throughout the operation.
    let (value, separator) = unsafe { (&*value, &*separator) };
    let mut offsets = LineOffsets::new(value);
    let mut result = Words::new(3);
    if offsets.next(value).is_some() && offsets.start < value.len() {
        let Some(chunk) =
            joined_tail(value, separator, offsets).and_then(|text| StrOwner::try_new(text).ok())
        else {
            // SAFETY: this is the original splitlines operation on the same rooted input.
            return unsafe { imre_str_splitlines(value) };
        };
        result
            .values
            .push(StrOwner::into_raw(text::owned_string("")) as usize);
        result.values.push(StrOwner::into_raw(chunk) as usize);
    }
    ObjectOwner::into_raw(result.into_object())
}
