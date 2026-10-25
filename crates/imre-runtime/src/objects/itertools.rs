//! Iterators keep their input owners even when other consumers share the cursor.

use super::*;
use words::WordKind;

pub(super) struct Repeat {
    pub(super) value: OwnedWord,
    pub(super) remaining: Option<usize>,
}

impl Repeat {
    pub(super) fn next(&mut self) -> Option<OwnedWord> {
        if let Some(remaining) = &mut self.remaining {
            *remaining = remaining.checked_sub(1)?;
        }
        Some(self.value.clone())
    }
}

fn ssize(value: &Int) -> Result<i64, Error> {
    value
        .as_small()
        .or_else(|| value.big().to_i64())
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C ssize_t",
            )
        })
}

/// # Safety
/// The caller roots the iterator and the canonical integer default.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cursor_length_hint(cursor: *const Object, default: usize) -> usize {
    // SAFETY: both arguments remain independently rooted during this read.
    unsafe {
        let default = Int::borrow(default);
        if let Err(error) = ssize(&default) {
            return error.returning(0);
        }
        let remaining = object(cursor).0.with(|storage| match storage {
            Storage::Repeat(repeat) => repeat.remaining,
            _ => None,
        });
        remaining
            .map_or_else(|| (*default).clone(), Int::from_usize)
            .into_raw()
    }
}

/// # Safety
/// The caller roots a value of the exact supplied kind. A nonzero `times`
/// is a rooted canonical integer; zero means that the argument was omitted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_repeat(kind: u32, value: usize, times: usize) -> *const Object {
    let remaining = if times == 0 {
        None
    } else {
        // SAFETY: the caller roots the count throughout conversion.
        let times = unsafe { Int::borrow(times) };
        let times = match ssize(&times) {
            Ok(times) => times,
            Err(error) => return error.returning(std::ptr::null()),
        };
        Some(times.max(0) as usize)
    };
    // SAFETY: the kind describes the caller's rooted argument.
    let value = unsafe { WordKind::new(kind).acquire(value) };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Repeat(Repeat {
        value,
        remaining,
    })))))
}
