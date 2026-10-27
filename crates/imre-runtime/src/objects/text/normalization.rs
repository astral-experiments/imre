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
    text: &'a str,
    quick: IsNormalized,
    normalized: impl Fn(&'a str) -> I,
) -> Result<Option<Str>> {
    if quick == IsNormalized::Yes {
        return Ok(None);
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
    Ok(Some(result))
}

/// Return normalized text, or `None` when the original string can be reused.
pub(crate) fn normalize_unicode(form: &str, text: &str) -> Result<Option<Str>> {
    if text.is_empty() {
        // CPython returns empty strings before validating the form.
        return Ok(None);
    }
    // NUL has the same quick-check and combining properties as an unassigned scalar.
    let chars = text
        .chars()
        .map(|ch| if newer_printable(ch) { '\0' } else { ch });
    match form {
        "NFC" => normalize(text, is_nfc_quick(chars), |s| s.nfc()),
        "NFD" => normalize(text, is_nfd_quick(chars), |s| s.nfd()),
        "NFKC" => normalize(text, is_nfkc_quick(chars), |s| s.nfkc()),
        "NFKD" => normalize(text, is_nfkd_quick(chars), |s| s.nfkd()),
        _ => Err(Error::new(Kind::ValueError, "invalid normalization form")),
    }
}

/// # Safety
/// The caller roots the UTF-8 form and input until ownership of the result transfers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_unicode_normalize(form: *const Str, text: *const Str) -> *const Str {
    // SAFETY: both arguments are immutable, rooted strings.
    let normalized = unsafe { normalize_unicode((&*form).as_str(), (&*text).as_str()) };
    let result = normalized.and_then(|normalized| {
        if let Some(normalized) = normalized {
            crate::checked_owned_str(normalized)
        } else {
            // SAFETY: the original raw handle names the allocation containing the strong count.
            unsafe { StrOwner::increment_strong_count(text) };
            Ok(text)
        }
    });
    finish(result, std::ptr::null())
}
