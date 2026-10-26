//! CPython 3.14 uses Unicode 16. The locked normalization tables use Unicode 17.

use crate::StrOwner;
use crate::integer_text::newer_printable;
use crate::objects::Str;
use crate::objects::exceptions::{Error, Kind, Result, finish};
use unicode_normalization::{
    IsNormalized, UnicodeNormalization, is_nfc_quick, is_nfd_quick, is_nfkc_quick, is_nfkd_quick,
};

/// Unicode 17 additions affecting normalization were unassigned starters in Unicode 16.
/// Keep each such scalar and normalize independently on either side of it.
fn normalize<'a, I: Iterator<Item = char>>(
    text: &'a Str,
    quick: IsNormalized,
    normalized: impl Fn(&'a str) -> I,
) -> Result<*const Str> {
    if quick == IsNormalized::Yes {
        // SAFETY: the caller retains this string throughout the operation.
        unsafe { StrOwner::increment_strong_count(text) };
        return Ok(text);
    }
    let mut result = Str::default();
    let mut start = 0;
    for (end, marker) in text
        .match_indices(newer_printable)
        .chain([(text.len(), "")])
    {
        for ch in normalized(&text[start..end]).chain(marker.chars()) {
            result
                .try_reserve(ch.len_utf8())
                .map_err(|_| Error::allocation())?;
            result.push(ch);
        }
        start = end + marker.len();
    }
    crate::checked_owned_str(result)
}

/// # Safety
/// The caller roots the UTF-8 form and input until ownership of the result transfers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_unicode_normalize(form: *const Str, text: *const Str) -> *const Str {
    // SAFETY: both arguments are immutable, rooted strings.
    let (form, text) = unsafe { (&*form, &*text) };
    if text.is_empty() {
        // CPython returns empty strings before validating the form.
        // SAFETY: the input stays rooted while the returned owner is acquired.
        unsafe { StrOwner::increment_strong_count(text) };
        return text;
    }
    // NUL has the same quick-check and combining properties as an unassigned scalar.
    let chars = text
        .chars()
        .map(|ch| if newer_printable(ch) { '\0' } else { ch });
    let result = match form.as_str() {
        "NFC" => normalize(text, is_nfc_quick(chars), |s| s.nfc()),
        "NFD" => normalize(text, is_nfd_quick(chars), |s| s.nfd()),
        "NFKC" => normalize(text, is_nfkc_quick(chars), |s| s.nfkc()),
        "NFKD" => normalize(text, is_nfkd_quick(chars), |s| s.nfkd()),
        _ => Err(Error::new(Kind::ValueError, "invalid normalization form")),
    };
    finish(result, std::ptr::null())
}
