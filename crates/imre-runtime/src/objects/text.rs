//! Native text operations preserve Python's Unicode-scalar and error semantics.

use crate::StrOwner;
mod expandtabs;
mod lowercase;
mod normalization;
pub(crate) mod translate;
pub(crate) mod urlquote;
pub(crate) mod urlsplit;

pub(crate) use expandtabs::expand_tabs;
pub(crate) use lowercase::capitalize;
pub(super) use lowercase::{lowercase, lowercase_chars};
pub(crate) use normalization::normalize_unicode;

use num_traits::{Signed, ToPrimitive};
use std::fmt::Write;
use std::mem::ManuallyDrop;

use super::exceptions::{Error, Kind, Result, finish};
use super::{Int, Object, Str, Words, fail, python_whitespace};

#[derive(Clone, Copy)]
pub(crate) enum UrlInput<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
}

impl<'a> UrlInput<'a> {
    pub(crate) fn is_unescaped_text(self) -> bool {
        matches!(self, Self::Text(s) if !s.contains('%'))
    }

    pub(crate) fn as_bytes(self) -> &'a [u8] {
        match self {
            Self::Text(s) => s.as_bytes(),
            Self::Bytes(b) => b,
        }
    }

    /// URL splitting accepts Unicode text and decodes byte components as strict ASCII.
    pub(crate) fn decoded(self) -> Result<&'a str> {
        match self {
            Self::Text(s) => Ok(s),
            Self::Bytes(b) => {
                if let Some(i) = b.iter().position(|c| !c.is_ascii()) {
                    return Err(super::bytes::ascii_decode_error(
                        super::bytes::owned_input(b)?,
                        i,
                    ));
                }
                Ok(std::str::from_utf8(b).expect("ASCII is UTF-8"))
            }
        }
    }
}

/// Borrow the string or bytes layout selected by a URL operation's type tag.
///
/// # Safety
/// The caller must root the matching owner for the lifetime of the returned borrow.
unsafe fn url_data<'a>(word: *const std::ffi::c_void, is_bytes: bool) -> &'a [u8] {
    if is_bytes {
        // SAFETY: The byte flag selects a rooted Bytes owner.
        unsafe { &*word.cast::<crate::Bytes>() }.as_slice()
    } else {
        // SAFETY: The other flag selects a rooted UTF-8 string owner.
        unsafe { &*word.cast::<Str>() }.as_bytes()
    }
}

/// Escape Unicode scalars in a Python representation using ASCII backslash replacement.
///
/// # Safety
/// The caller roots the Unicode input for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_ascii_escape(value: *const Str) -> *const Str {
    // SAFETY: the generated caller owns this immutable string.
    let text = unsafe { &*value };
    if text.is_ascii() {
        // SAFETY: transfer another ownership share to the checked caller.
        unsafe { StrOwner::increment_strong_count(value) };
        return value;
    }
    finish(
        ascii_escape(text).and_then(|ascii| crate::checked_owned_str(Str::from(ascii))),
        std::ptr::null(),
    )
}

/// Escape a representation for both builtin ASCII conversion and percent formatting.
pub(super) fn ascii_escape(text: &str) -> Result<String> {
    let mut ascii = String::new();
    let size = text.chars().try_fold(0usize, |len, ch| {
        len.checked_add(match ch as u32 {
            0..=127 => 1,
            128..=255 => 4,
            256..=65535 => 6,
            _ => 10,
        })
    });
    if size.is_none_or(|size| ascii.try_reserve_exact(size).is_err()) {
        return Err(Error::allocation());
    }
    for ch in text.chars() {
        let n = ch as u32;
        match n {
            0..=127 => ascii.push(ch),
            128..=255 => write!(ascii, "\\x{n:02x}").unwrap(),
            256..=65535 => write!(ascii, "\\u{n:04x}").unwrap(),
            _ => write!(ascii, "\\U{n:08x}").unwrap(),
        }
    }
    Ok(ascii)
}

/// Converts a Python character ordinal within the native string domain.
pub(crate) fn chr_scalar(value: &Int) -> Result<char> {
    match value.as_small() {
        Some(0xd800..=0xdfff) => Err(Error::new(
            Kind::ValueError,
            "surrogates are outside the native string domain",
        )),
        Some(code @ 0..=0x10ffff) => Ok(char::from_u32(code as u32).expect("Unicode scalar")),
        _ => Err(Error::new(
            Kind::ValueError,
            "chr() arg not in range(0x110000)",
        )),
    }
}

/// # Safety
/// The integer is rooted. Success transfers an immutable one-character string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chr(value: usize) -> *const Str {
    // SAFETY: the caller retains the integer through this conversion.
    let value = unsafe { Int::borrow(value) };
    let result =
        chr_scalar(&value).map(|c| StrOwner::into_raw(owned_string(c.encode_utf8(&mut [0; 4]))));
    finish(result, std::ptr::null())
}

fn ord_length(length: usize) -> Error {
    Error::new(
        Kind::TypeError,
        format!("ord() expected a character, but string of length {length} found"),
    )
}

pub(crate) fn ord_char(text: &str) -> Result<u32> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Ok(u32::from(ch)),
        (None, None) => Err(ord_length(0)),
        _ => Err(ord_length(2 + chars.count())),
    }
}

pub(crate) fn ord_byte(bytes: &[u8]) -> Result<u32> {
    match bytes {
        [byte] => Ok(u32::from(*byte)),
        _ => Err(ord_length(bytes.len())),
    }
}

/// Returns the scalar value of one character or the unsigned value of one byte.
///
/// # Safety
/// The caller roots a concrete string, bytes, or their checked nullable union.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_ord(mut word: usize, mut kind: u32) -> usize {
    let result = (|| {
        if word == 0 {
            return Err(Error::new(
                Kind::TypeError,
                "ord() expected string of length 1, but NoneType found",
            ));
        }
        kind &= !super::words::NULLABLE;
        if kind == 9 {
            let (payload, member) = super::unions::member(word);
            word = payload;
            kind = member.base();
        }
        let code = match kind {
            3 => {
                // SAFETY: the representation identifies a rooted UTF-8 string.
                ord_char(unsafe { &*(word as *const Str) })?
            }
            8 => {
                // SAFETY: the representation identifies an immutable rooted bytes value.
                let bytes = unsafe { &*(word as *const crate::Bytes) };
                ord_byte(bytes.as_slice())?
            }
            _ => unreachable!("ord's checked character layout"),
        };
        Ok(Int::from_usize(code as usize).into_raw())
    })();
    finish(result, 0)
}

/// # Safety
/// The caller roots the integer and supplies a bounded literal width. BigInt's
/// formatter pads signed magnitudes, including zeroes after a negative sign.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_hex_format(value: usize, width: u32, flags: u32) -> *const Str {
    // SAFETY: the native integer owner stays live until formatting completes.
    let value = unsafe { Int::borrow(value) };
    let big = value.big();
    let n = big.as_ref();
    let width = width as usize;
    let output = match flags {
        0 => format!("{n:width$x}"),
        1 => format!("{n:0width$x}"),
        2 => format!("{n:width$X}"),
        3 => format!("{n:0width$X}"),
        _ => unreachable!("invalid hexadecimal format flags"),
    };
    StrOwner::into_raw(StrOwner::new(Str::from(output)))
}

/// Returns Python's substring membership for two exact strings.
///
/// # Safety
/// Both operands are rooted for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_contains(needle: *const Str, text: *const Str) -> u32 {
    // SAFETY: the caller roots both immutable strings through the search.
    unsafe { u32::from((&*text).contains((&*needle).as_str())) }
}

/// CPython 3.14's alphanumeric characters are Unicode 16 letters and numbers.
pub(crate) fn python_alnum(text: &str) -> bool {
    if text.is_ascii() {
        return !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric());
    }
    const RANGES: &[(u32, u32)] = include!(concat!(env!("OUT_DIR"), "/unicode_alnum.rs"));
    text.chars().all(|ch| {
        let cp = u32::from(ch);
        let index = RANGES.partition_point(|&(_, end)| end < cp);
        RANGES.get(index).is_some_and(|&(start, _)| start <= cp)
    })
}

/// Tests a native string with Python's alphanumeric predicate.
///
/// # Safety
/// The caller roots the immutable string for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_isalnum(value: *const Str) -> u32 {
    // SAFETY: the receiver stays alive for this call.
    u32::from(python_alnum(unsafe { &*value }))
}

/// Returns whether the string contains one or more Python whitespace characters and nothing else.
pub(crate) fn python_isspace(text: &str) -> bool {
    !text.is_empty() && text.chars().all(python_whitespace)
}

/// Tests a nonempty Python string against the shared Unicode whitespace table.
///
/// # Safety
/// The immutable string is rooted for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_isspace(value: *const Str) -> u32 {
    // SAFETY: the receiver stays alive and no character reference escapes.
    let value = unsafe { &*value };
    u32::from(python_isspace(value))
}

/// Returns whether the string has a cased character and every cased character is uppercase.
pub(crate) fn python_isupper(text: &str) -> bool {
    use icu_properties::{
        CodePointSetData,
        props::{Cased, Uppercase},
    };
    let mut seen = false;
    for c in text.chars() {
        if c.is_ascii_uppercase() {
            seen = true;
        } else if c.is_ascii_lowercase() || c == '\u{295}' {
            // U+0295 was lowercase in Unicode 16, uncased in newer ICU data.
            return false;
        } else if !c.is_ascii()
            && CodePointSetData::new::<Cased>().contains(c)
            && !crate::integer_text::newer_printable(c)
        {
            if !CodePointSetData::new::<Uppercase>().contains(c) {
                return false;
            }
            seen = true;
        }
    }
    seen
}

/// Tests a native string with Python's uppercase predicate.
///
/// # Safety
/// The caller roots the immutable string for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_is_upper(value: *const Str) -> u32 {
    // SAFETY: the caller supplies a rooted, exact UTF-8 string.
    u32::from(python_isupper(unsafe { &*value }))
}

/// Removes the common space/tab prefix after the caller has evaluated its text.
/// Python 3.14 normalizes whitespace-only lines, including Unicode whitespace.
///
/// # Safety
/// The caller roots the immutable string until this independently owned result returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_dedent(text: *const Str) -> *const Str {
    // SAFETY: the input stays live and no user code runs while it is borrowed.
    let text = unsafe { &*text };
    StrOwner::into_raw(StrOwner::new(dedent(text)))
}

/// Remove common indentation with the same whitespace rules in both runtimes.
pub(crate) fn dedent(text: &str) -> Str {
    let blank = |line: &str| line.chars().all(python_whitespace);
    let mut lines = text.split('\n').filter(|line| !blank(line));
    let first = lines.next().unwrap_or("");
    let mut margin = &first.as_bytes()[..first.len() - first.trim_start_matches([' ', '\t']).len()];
    for line in lines {
        if margin.is_empty() {
            break;
        }
        let common = margin
            .iter()
            .zip(line.as_bytes())
            .take_while(|(a, b)| a == b)
            .count();
        margin = &margin[..common];
    }
    // Indentation and blank lines can account for nearly the entire input.
    let mut result = Str::default();
    for (index, line) in text.split('\n').enumerate() {
        if index != 0 {
            result.push('\n');
        }
        if !blank(line) {
            result.push_str(&line[margin.len()..]);
        }
    }
    result
}

thread_local! {
    static SHORT_STRINGS: std::cell::RefCell<[Option<StrOwner>; 257]> =
        const { std::cell::RefCell::new([const { None }; 257]) };
}

/// Reuses immutable owners for empty and single Latin-1 strings.
pub(super) fn owned_string(value: &str) -> StrOwner {
    let Ok(owner) = owned_string_with(value, |text| {
        Ok::<_, std::convert::Infallible>(StrOwner::new(Str::new(text)))
    });
    owner
}

pub(super) fn try_owned_string(value: &str) -> Result<StrOwner> {
    owned_string_with(value, |text| {
        let text = Str::try_new(text).map_err(|_| Error::allocation())?;
        StrOwner::try_new(text).map_err(|_| Error::allocation())
    })
}

fn owned_string_with<E>(
    value: &str,
    new: impl FnOnce(&str) -> std::result::Result<StrOwner, E>,
) -> std::result::Result<StrOwner, E> {
    let Some(index) = short_index(value) else {
        return new(value);
    };
    cached_short(value, index, new)
}

fn short_index(value: &str) -> Option<usize> {
    Some(match value.as_bytes() {
        [] => 256,
        [byte] => usize::from(*byte),
        // These UTF-8 lead bytes encode U+0080 through U+00FF.
        [0xc2 | 0xc3, _] => value.chars().next().unwrap() as usize,
        _ => return None,
    })
}

/// Share the same native owner for empty and single Latin-1 values.
pub(crate) fn short_string(value: &str) -> Option<StrOwner> {
    let index = short_index(value)?;
    let Ok(owner) = cached_short(value, index, |text| {
        Ok::<_, std::convert::Infallible>(StrOwner::new(Str::new(text)))
    });
    Some(owner)
}

fn cached_short<E>(
    value: &str,
    index: usize,
    new: impl FnOnce(&str) -> std::result::Result<StrOwner, E>,
) -> std::result::Result<StrOwner, E> {
    SHORT_STRINGS.with_borrow_mut(|strings| {
        let slot = &mut strings[index];
        let owner = match slot {
            Some(owner) => owner,
            None => slot.insert(new(value)?),
        };
        Ok(owner.clone())
    })
}

/// Appends an owned slice of source, retaining its allocation for a full slice.
pub(super) fn push_part(parts: &mut Words, source: &StrOwner, value: &str) {
    parts
        .values
        .try_reserve(1)
        .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
    let value = if value.len() == source.len() {
        StrOwner::clone(source)
    } else {
        owned_string(value)
    };
    parts.values.push(StrOwner::into_raw(value) as usize);
}

/// Whitespace runs count as one split; a zero limit only strips the starting end.
fn whitespace_parts<'a>(
    source: &'a str,
    limit: usize,
    reverse: bool,
    mut push: impl FnMut(&'a str),
) {
    let mut remaining = source;
    let mut splits = 0;
    loop {
        remaining = if reverse {
            remaining.trim_end_matches(python_whitespace)
        } else {
            remaining.trim_start_matches(python_whitespace)
        };
        if remaining.is_empty() {
            break;
        }
        if splits == limit {
            push(remaining);
            break;
        }
        let boundary = if reverse {
            remaining.rfind(python_whitespace)
        } else {
            remaining.find(python_whitespace)
        };
        let Some(boundary) = boundary else {
            push(remaining);
            break;
        };
        if reverse {
            let width = remaining[boundary..].chars().next().unwrap().len_utf8();
            push(&remaining[boundary + width..]);
            remaining = &remaining[..boundary];
        } else {
            push(&remaining[..boundary]);
            remaining = &remaining[boundary..];
        }
        splits += 1;
    }
}

/// A one-byte UTF-8 separator is ASCII, so each match is a scalar boundary.
fn byte_parts<'a>(
    source: &'a str,
    separator: u8,
    limit: usize,
    reverse: bool,
    mut push: impl FnMut(&'a str),
) {
    debug_assert!(separator.is_ascii());
    let boundaries = memchr::memchr_iter(separator, source.as_bytes());
    if reverse {
        let mut end = source.len();
        for index in boundaries.rev().take(limit) {
            push(&source[index + 1..end]);
            end = index;
        }
        push(&source[..end]);
    } else {
        let mut start = 0;
        for index in boundaries.take(limit) {
            push(&source[start..index]);
            start = index + 1;
        }
        push(&source[start..]);
    }
}

/// Converts maxsplit before testing the separator, matching CPython precedence.
fn split_text(
    source: &StrOwner,
    separator: Option<&str>,
    maxsplit: &Int,
    reverse: bool,
) -> Result<Words> {
    let maxsplit = maxsplit
        .as_small()
        .map(|value| value as isize)
        .or_else(|| maxsplit.big().to_isize())
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C ssize_t",
            )
        })?;
    let limit = if maxsplit < 0 {
        usize::MAX
    } else {
        maxsplit as usize
    };
    let mut parts = Words::new(3);
    split_parts(source, separator, limit, reverse, |s| {
        push_part(&mut parts, source, s)
    })
    .map_err(|message| Error::new(Kind::ValueError, message))?;
    if reverse {
        parts.values.reverse();
    }
    Ok(parts)
}

/// Visit borrowed split fields from either end; rsplit visits the last field first.
pub(crate) fn split_parts<'a>(
    source: &'a str,
    separator: Option<&str>,
    limit: usize,
    reverse: bool,
    mut push: impl FnMut(&'a str),
) -> std::result::Result<(), &'static str> {
    if let Some(separator) = separator {
        if separator.is_empty() {
            return Err("empty separator");
        }
        let count = limit.saturating_add(1);
        if separator.len() == 1 {
            byte_parts(source, separator.as_bytes()[0], limit, reverse, push);
        } else if reverse {
            for part in source.rsplitn(count, separator) {
                push(part);
            }
        } else {
            for part in source.splitn(count, separator) {
                push(part);
            }
        }
    } else {
        whitespace_parts(source, limit, reverse, push);
    }
    Ok(())
}

/// Splits by an explicit separator or Python whitespace, from either end.
///
/// # Safety
/// String handles and maxsplit remain rooted; separator may be null for None.
/// Returns an owned string sequence, or null with a pending language error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_split(
    value: *const Str,
    separator: *const Str,
    maxsplit: usize,
    reverse: u32,
) -> *const Object {
    // SAFETY: inputs remain immutable and rooted. ManuallyDrop borrows the Rc
    // capability without consuming the caller's owner; each part acquires its own.
    let (source, separator, maxsplit) = unsafe {
        (
            ManuallyDrop::new(StrOwner::from_raw(value)),
            separator.as_ref(),
            Int::borrow(maxsplit),
        )
    };
    finish(
        split_text(
            &source,
            separator.map(|value| value.as_str()),
            &maxsplit,
            reverse != 0,
        )
        .map(|parts| crate::ObjectOwner::into_raw(parts.into_object())),
        std::ptr::null(),
    )
}

/// Tokenize with the whitespace-splitting lexer used by shlex.split().
fn shell_words(source: &str, comments: bool, posix: bool) -> Result<Words> {
    let mut parts = Words::new(3);
    let mut word = Str::default();
    let mut quote = None;
    let mut started = false;
    let mut chars = source.chars();
    while let Some(c) = chars.next() {
        match c {
            c if quote == Some(c) => {
                quote = None;
                if !posix {
                    word.push(c);
                    parts
                        .values
                        .push(StrOwner::into_raw(StrOwner::new(std::mem::take(&mut word))) as usize);
                    started = false;
                }
            }
            '\'' | '"' if quote.is_none() && (posix || !started) => {
                quote = Some(c);
                started = true;
                if !posix {
                    word.push(c);
                }
            }
            '\\' if posix && quote != Some('\'') => {
                let escaped = chars
                    .next()
                    .ok_or_else(|| Error::new(Kind::ValueError, "No escaped character"))?;
                if quote == Some('"') && !matches!(escaped, '"' | '\\') {
                    word.push('\\');
                }
                word.push(escaped);
                started = true;
            }
            '#' if comments && quote.is_none() => {
                // readline() consumes the newline. The legacy lexer stays
                // inside a current word, so text on the next line can join it.
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
                if posix && started {
                    parts
                        .values
                        .push(StrOwner::into_raw(StrOwner::new(std::mem::take(&mut word))) as usize);
                    started = false;
                }
            }
            ' ' | '\r' | '\n' | '\t' if quote.is_none() => {
                if started {
                    parts
                        .values
                        .push(StrOwner::into_raw(StrOwner::new(std::mem::take(&mut word))) as usize);
                    started = false;
                }
            }
            other => {
                word.push(other);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err(Error::new(Kind::ValueError, "No closing quotation"));
    }
    if started {
        parts
            .values
            .push(StrOwner::into_raw(StrOwner::new(word)) as usize);
    }
    Ok(parts)
}

/// # Safety
/// The caller roots the input string. Success transfers a fresh list of strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shlex_split(value: *const Str) -> *const Object {
    // SAFETY: preserve the original one-argument ABI with Python's defaults.
    unsafe { imre_shlex_split_options(value, 0, 1) }
}

/// # Safety
/// The optional string stays rooted; a null pointer represents Python None.
/// Success transfers the list and its strings; failure sets a pending error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shlex_split_options(
    value: *const Str,
    comments: u32,
    posix: u32,
) -> *const Object {
    // SAFETY: a nonnull input is a rooted, immutable Str.
    let parts = unsafe { value.as_ref() }
        .ok_or_else(|| Error::new(Kind::ValueError, "s argument must not be None"))
        .and_then(|source| shell_words(source, comments != 0, posix != 0));
    finish(
        parts.map(|parts| crate::ObjectOwner::into_raw(parts.into_object())),
        std::ptr::null(),
    )
}

/// Quotes a POSIX shell word using shlex's ASCII safe set and apostrophe escape.
///
/// # Safety
/// The input string stays rooted for the call. The returned string is owned;
/// null indicates a pending allocation error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shlex_quote(value: *const Str) -> *const Str {
    // SAFETY: the caller roots this immutable string.
    let source = unsafe { &*value };
    if source.is_empty() {
        return StrOwner::into_raw(owned_string("''"));
    }
    if source
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        // SAFETY: transfer a new reference to the existing safe string.
        unsafe { StrOwner::increment_strong_count(value) };
        return value;
    }
    let capacity = source
        .matches('\'')
        .count()
        .checked_mul(4)
        .and_then(|extra| source.len().checked_add(extra))
        .and_then(|n| n.checked_add(2));
    let result = capacity.and_then(|n| {
        let mut quoted = Str::try_with_capacity(n).ok()?;
        quoted.push('\'');
        for (index, piece) in source.split('\'').enumerate() {
            if index != 0 {
                quoted.push_str("'\"'\"'");
            }
            quoted.push_str(piece);
        }
        quoted.push('\'');
        Some(quoted)
    });
    owned_text(result)
}

/// Selects the final field of an ephemeral explicit-separator right split.
///
/// # Safety
/// Both inputs must be nonnull, immutable strings rooted for this call. Returns
/// an owned string, or null with a pending separator or allocation error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_rsplit_last(
    value: *const Str,
    separator: *const Str,
) -> *const Str {
    // SAFETY: the caller roots both strings for the duration of the split.
    unsafe { rsplit_last(value, separator, false) }
}

/// Percent-decodes the borrowed final field of a right split into its result.
///
/// # Safety
/// Both inputs must be nonnull, immutable strings rooted for this call. Returns
/// an owned string, or null with a pending separator or allocation error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_unquote_rsplit_last(
    value: *const Str,
    separator: *const Str,
) -> *const Str {
    // SAFETY: both source strings remain live through decoding.
    unsafe { rsplit_last(value, separator, true) }
}

/// # Safety
/// Both inputs are rooted immutable strings. The returned reference is owned.
unsafe fn rsplit_last(value: *const Str, separator: *const Str, decode: bool) -> *const Str {
    // SAFETY: both string owners remain live throughout the operation.
    let (source, separator) = unsafe { (&*value, &*separator) };
    if separator.is_empty() {
        return finish(
            Err(Error::new(Kind::ValueError, "empty separator")),
            std::ptr::null(),
        );
    }
    let boundary = if separator.len() == 1 {
        memchr::memrchr(separator.as_bytes()[0], source.as_bytes())
    } else {
        source.rfind(separator.as_str())
    };
    let tail = &source[boundary.map_or(0, |index| index + separator.len())..];
    if decode && let Some(first) = memchr::memchr(b'%', tail.as_bytes()) {
        owned_text(unquote(tail, first))
    } else if boundary.is_some() {
        owned_text(Str::try_new(tail).ok())
    } else {
        // SAFETY: the caller's live owner permits acquiring the result reference.
        unsafe { StrOwner::increment_strong_count(value) };
        value
    }
}

/// Trims a character set or Python whitespace from the selected ends.
///
/// # Safety
/// The caller roots both strings; chars may be null. Mode is 0 (both ends),
/// 1 (leading), or 2 (trailing). The result owns an independent string reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_strip(
    value: *const Str,
    chars: *const Str,
    mode: u32,
) -> *const Str {
    // SAFETY: immutable string inputs remain rooted throughout the call.
    let (source, chars) = unsafe { (&*value, chars.as_ref()) };
    let matches = |character| {
        chars.map_or_else(
            || python_whitespace(character),
            |chars| chars.contains(character),
        )
    };
    let result = match mode {
        0 => source.trim_matches(matches),
        1 => source.trim_start_matches(matches),
        2 => source.trim_end_matches(matches),
        _ => fail("SystemError", "invalid string strip mode"),
    };
    if result.len() == source.len() {
        // SAFETY: the caller's live owner permits acquiring the result reference.
        unsafe { StrOwner::increment_strong_count(value) };
        value
    } else {
        StrOwner::into_raw(StrOwner::new(Str::new(result)))
    }
}

/// Tests one literal prefix or suffix.
///
/// # Safety
/// Both inputs are rooted strings. A nonzero reverse flag selects endswith.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_affix(value: *const Str, affix: *const Str, reverse: u32) -> u32 {
    // SAFETY: this immutable comparison retains no borrowed input references.
    let (source, affix) = unsafe { (&*value, &*affix) };
    u32::from(if reverse != 0 {
        source.ends_with(affix.as_str())
    } else {
        source.starts_with(affix.as_str())
    })
}

/// Finds the byte boundary for a Python scalar offset without truncating big ints.
/// Starts beyond the end remain invalid even when searching for an empty string.
fn search_start(source: &str, start: Option<&Int>) -> Option<(usize, usize)> {
    let start = if let Some(start) = start {
        if let Some(value) = start.as_small() {
            value as isize
        } else {
            let value = start.big();
            match value.to_isize() {
                Some(value) => value,
                None if value.is_negative() => 0,
                None => return None,
            }
        }
    } else {
        0
    };
    let start = if start < 0 {
        (source.chars().count() as isize + start).max(0) as usize
    } else {
        start as usize
    };
    if start == 0 {
        return Some((0, 0));
    }
    if source.is_ascii() {
        return (start <= source.len()).then_some((start, start));
    }
    let mut characters = source.char_indices();
    for _ in 0..start {
        characters.next()?;
    }
    Some((characters.next().map_or(source.len(), |(at, _)| at), start))
}

/// Returns the first matching Unicode-scalar position, or -1 when absent.
///
/// # Safety
/// Both strings and the optional Int remain rooted; raw zero means start=None.
/// The returned canonical Int is owned and reports Unicode-scalar offsets.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_find(
    value: *const Str,
    needle: *const Str,
    start: usize,
) -> usize {
    // SAFETY: strings and a nonzero optional integer are rooted ABI arguments.
    let (source, needle) = unsafe { (&*value, &*needle) };
    // SAFETY: a nonzero start is an exact canonical integer handle.
    let start = (start != 0).then(|| unsafe { Int::borrow(start) });
    find_text(source, needle, start.as_deref(), None, false).into_raw()
}

/// Search a Python slice without converting its Unicode-scalar bounds to byte indices twice.
pub(crate) fn find_text(
    source: &str,
    needle: &str,
    start: Option<&Int>,
    end: Option<&Int>,
    reverse: bool,
) -> Int {
    let found = search_start(source, start).and_then(|(begin, scalar)| {
        let end = end
            .and_then(|i| search_start(source, Some(i)))
            .map_or(source.len(), |(b, _)| b);
        let window = source.get(begin..end)?;
        let offset = if reverse {
            window.rfind(needle)
        } else {
            window.find(needle)
        }?;
        Some(scalar + window[..offset].chars().count())
    });
    found.map_or_else(|| Int::small(-1).unwrap(), Int::from_usize)
}

pub(crate) fn partition_text<'a>(
    source: &'a str,
    separator: &str,
    reverse: bool,
) -> Result<Option<(&'a str, &'a str)>> {
    if separator.is_empty() {
        return Err(Error::new(Kind::ValueError, "empty separator"));
    }
    Ok(if reverse {
        source.rsplit_once(separator)
    } else {
        source.split_once(separator)
    })
}

/// Partitions around the first separator into three independently owned fields.
///
/// # Safety
/// Strings remain rooted and output is a fresh zeroed tuple[str,str,str] payload.
/// Successful writes transfer three string owners; failure leaves output zeroed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_partition(
    value: *const Str,
    separator: *const Str,
    output: *mut *const Str,
) {
    // SAFETY: immutable input strings remain rooted through every output write.
    let (source, separator_value) = unsafe { (&*value, &*separator) };
    let parts = match partition_text(source, separator_value, false) {
        Ok(Some((before, after))) => {
            // SAFETY: separator is rooted; the tuple acquires an independent owner.
            unsafe { StrOwner::increment_strong_count(separator) };
            [
                StrOwner::into_raw(StrOwner::new(Str::new(before))),
                separator,
                StrOwner::into_raw(StrOwner::new(Str::new(after))),
            ]
        }
        Ok(None) => {
            // SAFETY: an unsuccessful partition retains the complete original string.
            unsafe { StrOwner::increment_strong_count(value) };
            [
                value,
                StrOwner::into_raw(StrOwner::new(Str::new(""))),
                StrOwner::into_raw(StrOwner::new(Str::new(""))),
            ]
        }
        Err(error) => {
            error.raise();
            return;
        }
    };
    for (index, part) in parts.into_iter().enumerate() {
        // SAFETY: output has three fresh pointer-typed slots, owned by the caller.
        unsafe { output.add(index).write(part) };
    }
}

/// Decodes one ASCII hexadecimal digit without accepting Unicode lookalikes.
fn hexadecimal(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decodes valid percent pairs, retaining malformed escapes and literal '+'.
/// `first` identifies a percent byte; unchanged spans are copied in one chunk.
fn percent_bytes(input: &[u8], first: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(input.len()).ok()?;
    bytes.extend_from_slice(&input[..first]);
    let mut index = first;
    loop {
        if let Some(pair) = input.get(index + 1..index + 3)
            && let (Some(high), Some(low)) = (hexadecimal(pair[0]), hexadecimal(pair[1]))
        {
            bytes.push(high * 16 + low);
            index += 3;
        } else {
            bytes.push(b'%');
            index += 1;
        }
        let Some(offset) = memchr::memchr(b'%', &input[index..]) else {
            bytes.extend_from_slice(&input[index..]);
            break;
        };
        bytes.extend_from_slice(&input[index..index + offset]);
        index += offset;
    }
    Some(bytes)
}

fn unquote(source: &str, first: usize) -> Option<Str> {
    let bytes = percent_bytes(source.as_bytes(), first)?;
    match String::from_utf8(bytes) {
        Ok(decoded) => Some(Str::from(decoded)),
        Err(error) => {
            // Each ill-formed sequence contains a decoded three-byte percent escape.
            // Replacing it with U+FFFD therefore fits in the encoded input length.
            let mut result = String::new();
            result.try_reserve_exact(source.len()).ok()?;
            for chunk in error.as_bytes().utf8_chunks() {
                result.push_str(chunk.valid());
                if !chunk.invalid().is_empty() {
                    result.push('\u{fffd}');
                }
            }
            Some(Str::from(result))
        }
    }
}

fn owned_text(result: Option<Str>) -> *const Str {
    if let Some(text) = result {
        StrOwner::into_raw(StrOwner::new(text))
    } else {
        super::exceptions::sequence_allocation_failed();
        std::ptr::null()
    }
}

/// Decodes percent escapes with UTF-8 replacement while preserving literal '+'.
///
/// # Safety
/// Input is a rooted scalar string. Returns one owned string reference with the
/// default urllib.parse.unquote UTF-8/errors='replace' semantics, or null on allocation failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_unquote(value: *const Str) -> *const Str {
    // SAFETY: input remains immutable and rooted throughout the operation.
    let source = unsafe { &*value };
    if let Some(first) = memchr::memchr(b'%', source.as_bytes()) {
        owned_text(unquote(source, first))
    } else {
        // SAFETY: the unchanged result acquires its own live reference.
        unsafe { StrOwner::increment_strong_count(value) };
        value
    }
}

#[cfg(test)]
mod tests {
    use super::super::exceptions::{
        imre_error_take, imre_exception_matches, imre_exception_message,
    };
    use super::*;
    use num_bigint::BigInt;

    /// Copies a native split result into ordinary strings for readable assertions.
    fn parts(source: &str, separator: Option<&str>, maxsplit: i64, reverse: bool) -> Vec<String> {
        let result = split_text(
            &StrOwner::new(Str::new(source)),
            separator,
            &Int::new(BigInt::from(maxsplit)),
            reverse,
        )
        .unwrap_or_else(|_| panic!("valid split unexpectedly failed"));
        result
            .values
            .iter()
            .map(|&word| {
                // SAFETY: result owns every string word until the copies are complete.
                unsafe { (&*(word as *const Str)).to_string() }
            })
            .collect()
    }

    /// Checks the public exception payload and consumes both acquired owners.
    fn check_error(error: Error, kind: Kind, message: &str) {
        error.raise();
        // SAFETY: raising creates one pending owner; the message getter returns
        // a second independent owner, both consumed by their Rc wrappers.
        unsafe {
            let error = crate::ObjectOwner::from_raw(imre_error_take());
            assert_eq!(
                imre_exception_matches(crate::ObjectOwner::as_ptr(&error), kind as u32),
                1
            );
            let actual =
                StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(&error)));
            assert_eq!(actual.as_str(), message);
        }
    }

    #[test]
    fn unchanged_normalization_retains_the_input() {
        let form = StrOwner::new(Str::new("NFC"));
        let input = StrOwner::new(Str::new("already normalized"));
        let raw = StrOwner::as_ptr(&input);
        // SAFETY: both input owners outlive the call.
        let normalized =
            unsafe { normalization::imre_unicode_normalize(StrOwner::as_ptr(&form), raw) };
        assert_eq!(normalized, raw);
        assert_eq!(StrOwner::strong_count(&input), 2);
        // SAFETY: normalization transferred the second strong reference.
        unsafe { StrOwner::decrement_strong_count(normalized) };
        assert_eq!(StrOwner::strong_count(&input), 1);
    }

    #[test]
    fn refused_short_string_does_not_poison_the_cache() {
        SHORT_STRINGS.with_borrow_mut(|strings| strings[0xe9] = None);
        assert!(owned_string_with("é", |_| Err(())).is_err());
        let owner = try_owned_string("é").ok().unwrap();
        let cached = owned_string_with("é", |_| Err(())).unwrap();
        assert!(StrOwner::ptr_eq(&owner, &cached));
    }

    #[test]
    fn dedent_preserves_content_and_normalizes_whitespace_only_lines() {
        for (source, expected) in [
            ("  x\r\n \t\u{a0}\u{1c}\r\n   y\r\n", "x\r\n\n y\r\n"),
            ("\tx\n \ty\n\u{2003}\n", "\tx\n \ty\n\n"),
            ("\t\u{a0}x\n\t x", "\u{a0}x\n x"),
            ("\n\u{2002}\n\u{85}", "\n\n"),
        ] {
            let source = Str::from(source);
            // SAFETY: the input lives through the call and the result transfers one owner.
            let result = unsafe { StrOwner::from_raw(imre_str_dedent(&source)) };
            assert_eq!(result.as_str(), expected);
        }
    }

    #[test]
    fn dedent_keeps_small_results_inline_after_removing_large_prefixes() {
        for (text, expected) in [
            (format!("{}x", " ".repeat(65_536)), "x"),
            (format!("{}\n", "\u{2003}".repeat(21_000)), "\n"),
        ] {
            let source = Str::from(text);
            // SAFETY: the source lives through the call and the result transfers one owner.
            let result = unsafe { StrOwner::from_raw(imre_str_dedent(&source)) };
            assert_eq!(result.as_str(), expected);
            assert!(!result.is_heap_allocated());
        }
    }

    #[test]
    fn explicit_splits_preserve_empty_fields_and_directional_overlap() {
        assert_eq!(parts("aaa", Some("aa"), -1, false), ["", "a"]);
        assert_eq!(parts("aaa", Some("aa"), -1, true), ["a", ""]);
        assert_eq!(parts("🙂a🙂🙂", Some("🙂"), 2, false), ["", "a", "🙂"]);
        assert_eq!(parts("🙂a🙂🙂", Some("🙂"), 2, true), ["🙂a", "", ""]);
        assert_eq!(parts("", Some(","), 0, false), [""]);
    }

    #[test]
    fn no_match_split_retains_source_without_consuming_its_caller() {
        let source = StrOwner::new(Str::new("an independently owned source without separators"));
        let separator = StrOwner::new(Str::new("::"));
        // SAFETY: inputs remain rooted; both native getters transfer owned handles.
        unsafe {
            let parts = crate::ObjectOwner::from_raw(imre_str_split(
                StrOwner::as_ptr(&source),
                StrOwner::as_ptr(&separator),
                Int::small(-1).unwrap().0,
                0,
            ));
            assert_eq!(StrOwner::strong_count(&source), 2);
            let selected = StrOwner::from_raw(super::super::imre_seq_get_index(
                crate::ObjectOwner::as_ptr(&parts),
                0,
            ) as *const Str);
            assert!(StrOwner::ptr_eq(&source, &selected));
            drop(source);
            drop(parts);
            assert_eq!(StrOwner::strong_count(&selected), 1);
            assert_eq!(
                selected.as_str(),
                "an independently owned source without separators"
            );
        }
    }

    #[test]
    fn rsplit_last_keeps_an_independent_owner_and_reports_empty_separators() {
        let source = StrOwner::new(Str::new("an independently owned string without separators"));
        let separator = StrOwner::new(Str::new("::"));
        // SAFETY: both inputs are rooted; the successful result transfers one owner.
        let result = unsafe {
            StrOwner::from_raw(imre_str_rsplit_last(
                StrOwner::as_ptr(&source),
                StrOwner::as_ptr(&separator),
            ))
        };
        assert!(StrOwner::ptr_eq(&source, &result));
        drop(source);
        assert_eq!(StrOwner::strong_count(&result), 1);
        assert_eq!(
            result.as_str(),
            "an independently owned string without separators"
        );
        let empty = StrOwner::new(Str::new(""));
        // SAFETY: inputs remain rooted; failure returns null and one pending error.
        unsafe {
            assert!(
                imre_str_rsplit_last(StrOwner::as_ptr(&result), StrOwner::as_ptr(&empty)).is_null()
            );
            let error = imre_error_take();
            assert!(!error.is_null());
            let error = crate::ObjectOwner::from_raw(error);
            assert_eq!(
                imre_exception_matches(crate::ObjectOwner::as_ptr(&error), Kind::ValueError as u32),
                1,
            );
            let message =
                StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(&error)));
            assert_eq!(message.as_str(), "empty separator");
        }
    }

    #[test]
    fn whitespace_split_limits_preserve_the_unsplit_end() {
        assert_eq!(
            parts("\u{1c}  a\u{a0}b \u{3000}", None, 0, false),
            ["a\u{a0}b \u{3000}"]
        );
        assert_eq!(
            parts("\u{1c}  a\u{a0}b \u{3000}", None, 0, true),
            ["\u{1c}  a\u{a0}b"]
        );
        assert_eq!(parts(" a  b c ", None, 1, false), ["a", "b c "]);
        assert_eq!(parts(" a  b c ", None, 1, true), [" a  b", "c"]);
        assert!(parts(" \t\u{1f}", None, 0, false).is_empty());
        assert_eq!(parts("a\u{200b}b", None, -1, false), ["a\u{200b}b"]);
    }

    #[test]
    fn maxsplit_conversion_precedes_empty_separator_validation() {
        let source = StrOwner::new(Str::new("x"));
        let huge = Int::new(BigInt::from(1) << 100_u32);
        let error = split_text(&source, Some(""), &huge, false).err().unwrap();
        check_error(
            error,
            Kind::OverflowError,
            "Python int too large to convert to C ssize_t",
        );
        check_error(
            split_text(&source, Some(""), &Int::small(0).unwrap(), true)
                .err()
                .unwrap(),
            Kind::ValueError,
            "empty separator",
        );
    }

    #[test]
    fn search_offsets_are_scalars_and_do_not_clamp_past_end() {
        let source = "aλ🙂b";
        assert_eq!(
            search_start(source, Some(&Int::small(2).unwrap())),
            Some((3, 2))
        );
        assert_eq!(
            search_start(source, Some(&Int::small(-2).unwrap())),
            Some((3, 2))
        );
        assert_eq!(
            search_start(source, Some(&Int::small(4).unwrap())),
            Some((8, 4))
        );
        assert_eq!(search_start(source, Some(&Int::small(5).unwrap())), None);
        assert_eq!(
            search_start(source, Some(&Int::new(BigInt::from(1) << 100_u32))),
            None
        );
        assert_eq!(
            search_start(source, Some(&Int::new(-(BigInt::from(1) << 100_u32)))),
            Some((0, 0))
        );
    }

    #[test]
    fn url_decoding_keeps_malformed_escapes_and_replaces_utf8_subparts() {
        for (source, expected) in [
            ("", ""),
            ("unchanged+λ🙂", "unchanged+λ🙂"),
            ("λ+space%20%2B", "λ+space +"),
            ("%zz%2%", "%zz%2%"),
            ("%F0%9F%99%82", "🙂"),
            ("%E1%80", "�"),
            ("%ED%A0%80", "���"),
            ("%E9é%80", "�é�"),
            ("%00tail", "\0tail"),
        ] {
            let original = StrOwner::new(Str::new(source));
            // SAFETY: the input is rooted; the returned independent owner is
            // consumed by the Rc wrapper, including when the handle is reused.
            let actual =
                unsafe { StrOwner::from_raw(imre_str_unquote(StrOwner::as_ptr(&original))) };
            assert_eq!(actual.as_str(), expected, "{source}");
        }
    }
}
