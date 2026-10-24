//! Coalesces a splitlines tail when its only consumer extends a private joined list.

use super::{Boundaries, LineOffsets, Object, Str, Words, split_lines, text};
use std::rc::Rc;

/// Produces the first line followed by the joined tail, omitting the latter if
/// the original had no second line. A subsequent [1:] and join therefore keep
/// empty lines and do not introduce a separator for an empty slice.
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
    if let Some(header) = offsets.next(value) {
        result
            .values
            .push(Rc::into_raw(text::owned_string(&value[header])) as usize);
        let tail = &value[offsets.start..];
        if !tail.is_empty() {
            // An LF-only suffix delimited by LF is already the desired output.
            let joined = if separator == "\n"
                && matches!(offsets.boundaries, Boundaries::CrLf)
                && memchr::memchr(b'\r', tail.as_bytes()).is_none()
            {
                text::owned_string(tail.strip_suffix('\n').unwrap_or(tail))
            } else {
                let mut chunk = Str::default();
                let mut first = true;
                while let Some(row) = offsets.next(value) {
                    let sep = if first { "" } else { separator.as_str() };
                    let Some(n) = row.len().checked_add(sep.len()) else {
                        return split_lines(value, |range| text::owned_string(&value[range]));
                    };
                    if chunk.try_reserve(n).is_err() {
                        // Let the original list/join determine when MemoryError is observed.
                        return split_lines(value, |range| text::owned_string(&value[range]));
                    }
                    chunk.push_str(sep);
                    chunk.push_str(&value[row]);
                    first = false;
                }
                Rc::new(chunk)
            };
            result.values.push(Rc::into_raw(joined) as usize);
        }
    }
    Rc::into_raw(result.into_object())
}
