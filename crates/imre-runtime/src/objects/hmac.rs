//! HMAC verification shared by typed callers and runtime-loaded Python modules.

use super::exceptions::{Error, Kind, finish};
use super::{Bytes, Str};

pub(crate) fn compare_bytes(left: &[u8], right: &[u8]) -> bool {
    constant_time_eq::constant_time_eq(left, right)
}

pub(crate) fn compare_text(left: &str, right: &str) -> Result<bool, &'static str> {
    if !left.is_ascii() || !right.is_ascii() {
        return Err("comparing strings with non-ASCII characters is not supported");
    }
    Ok(compare_bytes(left.as_bytes(), right.as_bytes()))
}

/// # Safety
/// Both immutable byte handles remain rooted until the comparison returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hmac_compare_digest_bytes(
    left: *const Bytes,
    right: *const Bytes,
) -> bool {
    // SAFETY: both inputs are rooted exact bytes and no borrow escapes.
    unsafe { compare_bytes(&*left, &*right) }
}

/// # Safety
/// Both immutable string handles remain rooted until the comparison returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hmac_compare_digest_str(left: *const Str, right: *const Str) -> bool {
    // SAFETY: both inputs are rooted exact strings and no borrow escapes.
    let result = unsafe { compare_text(&*left, &*right) };
    finish(
        result.map_err(|message| Error::new(Kind::TypeError, message)),
        false,
    )
}
