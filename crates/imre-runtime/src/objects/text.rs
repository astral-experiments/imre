//! Native text operations preserve Python's Unicode-scalar and error semantics.

mod lowercase;

use num_traits::{Signed, ToPrimitive};
use std::mem::ManuallyDrop;
use std::rc::Rc;

use super::exceptions::{Error, Kind, Result, finish};
use super::{Int, Object, Str, Words, fail, python_whitespace};

/// Returns Python's substring membership for two exact strings.
///
/// # Safety
/// Both operands are rooted for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_contains(needle: *const Str, text: *const Str) -> u32 {
    // SAFETY: the caller roots both immutable strings through the search.
    unsafe { u32::from((&*text).contains((&*needle).as_str())) }
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
    Rc::into_raw(Rc::new(result))
}

thread_local! {
    static SHORT_STRINGS: std::cell::RefCell<[Option<Rc<Str>>; 257]> =
        const { std::cell::RefCell::new([const { None }; 257]) };
}

/// Reuses immutable owners for empty and single Latin-1 strings.
pub(super) fn owned_string(value: &str) -> Rc<Str> {
    let index = match value.as_bytes() {
        [] => 256,
        [byte] => usize::from(*byte),
        // These UTF-8 lead bytes encode U+0080 through U+00FF.
        [0xc2 | 0xc3, _] => value.chars().next().unwrap() as usize,
        _ => return Rc::new(Str::new(value)),
    };
    SHORT_STRINGS.with_borrow_mut(|strings| {
        Rc::clone(strings[index].get_or_insert_with(|| Rc::new(Str::new(value))))
    })
}

/// Appends an owned slice of source, retaining its allocation for a full slice.
pub(super) fn push_part(parts: &mut Words, source: &Rc<Str>, value: &str) {
    parts
        .values
        .try_reserve(1)
        .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
    let value = if value.len() == source.len() {
        Rc::clone(source)
    } else {
        owned_string(value)
    };
    parts.values.push(Rc::into_raw(value) as usize);
}

/// Whitespace runs count as one split; a zero limit only strips the starting end.
fn whitespace_parts(source: &Rc<Str>, limit: usize, reverse: bool, parts: &mut Words) {
    let mut remaining = source.as_str();
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
            push_part(parts, source, remaining);
            break;
        }
        let boundary = if reverse {
            remaining.rfind(python_whitespace)
        } else {
            remaining.find(python_whitespace)
        };
        let Some(boundary) = boundary else {
            push_part(parts, source, remaining);
            break;
        };
        if reverse {
            let width = remaining[boundary..].chars().next().unwrap().len_utf8();
            push_part(parts, source, &remaining[boundary + width..]);
            remaining = &remaining[..boundary];
        } else {
            push_part(parts, source, &remaining[..boundary]);
            remaining = &remaining[boundary..];
        }
        splits += 1;
    }
}

/// A one-byte UTF-8 separator is ASCII, so each match is a scalar boundary.
fn byte_parts(source: &Rc<Str>, separator: u8, limit: usize, reverse: bool, parts: &mut Words) {
    debug_assert!(separator.is_ascii());
    let boundaries = memchr::memchr_iter(separator, source.as_bytes());
    if reverse {
        let mut end = source.len();
        for index in boundaries.rev().take(limit) {
            push_part(parts, source, &source[index + 1..end]);
            end = index;
        }
        push_part(parts, source, &source[..end]);
    } else {
        let mut start = 0;
        for index in boundaries.take(limit) {
            push_part(parts, source, &source[start..index]);
            start = index + 1;
        }
        push_part(parts, source, &source[start..]);
    }
}

/// Converts maxsplit before testing the separator, matching CPython precedence.
fn split_text(
    source: &Rc<Str>,
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
    if let Some(separator) = separator {
        if separator.is_empty() {
            return Err(Error::new(Kind::ValueError, "empty separator"));
        }
        let count = limit.saturating_add(1);
        if separator.len() == 1 {
            byte_parts(source, separator.as_bytes()[0], limit, reverse, &mut parts);
        } else if reverse {
            for part in source.rsplitn(count, separator) {
                push_part(&mut parts, source, part);
            }
        } else {
            for part in source.splitn(count, separator) {
                push_part(&mut parts, source, part);
            }
        }
    } else {
        whitespace_parts(source, limit, reverse, &mut parts);
    }
    if reverse {
        parts.values.reverse();
    }
    Ok(parts)
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
            ManuallyDrop::new(Rc::from_raw(value)),
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
        .map(|parts| Rc::into_raw(parts.into_object())),
        std::ptr::null(),
    )
}

/// Splits POSIX shell words with Python's default comments=False. Inside double
/// quotes only another double quote or a backslash loses its preceding escape.
fn shell_words(source: &str) -> Result<Words> {
    let mut parts = Words::new(3);
    let mut word = Str::default();
    let mut quote = None;
    let mut started = false;
    let mut chars = source.chars();
    while let Some(c) = chars.next() {
        match c {
            c if quote == Some(c) => quote = None,
            '\'' | '"' if quote.is_none() => {
                quote = Some(c);
                started = true;
            }
            '\\' if quote != Some('\'') => {
                let escaped = chars
                    .next()
                    .ok_or_else(|| Error::new(Kind::ValueError, "No escaped character"))?;
                if quote == Some('"') && !matches!(escaped, '"' | '\\') {
                    word.push('\\');
                }
                word.push(escaped);
                started = true;
            }
            ' ' | '\r' | '\n' | '\t' if quote.is_none() => {
                if started {
                    parts
                        .values
                        .push(Rc::into_raw(Rc::new(std::mem::take(&mut word))) as usize);
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
        parts.values.push(Rc::into_raw(Rc::new(word)) as usize);
    }
    Ok(parts)
}

/// # Safety
/// The caller roots the input string. Success transfers a fresh list of strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shlex_split(value: *const Str) -> *const Object {
    // SAFETY: the input is immutable and rooted throughout tokenization.
    let parts = shell_words(unsafe { &*value });
    finish(
        parts.map(|parts| Rc::into_raw(parts.into_object())),
        std::ptr::null(),
    )
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
        unsafe { Rc::increment_strong_count(value) };
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
        unsafe { Rc::increment_strong_count(value) };
        value
    } else {
        Rc::into_raw(Rc::new(Str::new(result)))
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
    let found = search_start(source, start.as_deref()).and_then(|(byte, scalar)| {
        source[byte..]
            .find(needle.as_str())
            .map(|offset| scalar + source[byte..byte + offset].chars().count())
    });
    found
        .map_or_else(|| Int::small(-1).unwrap(), Int::from_usize)
        .into_raw()
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
    if separator_value.is_empty() {
        Error::new(Kind::ValueError, "empty separator").raise();
        return;
    }
    let parts = if let Some(at) = source.find(separator_value.as_str()) {
        // SAFETY: separator is rooted; the tuple acquires an independent owner.
        unsafe { Rc::increment_strong_count(separator) };
        [
            Rc::into_raw(Rc::new(Str::new(&source[..at]))),
            separator,
            Rc::into_raw(Rc::new(Str::new(&source[at + separator_value.len()..]))),
        ]
    } else {
        // SAFETY: an unsuccessful partition retains the complete original string.
        unsafe { Rc::increment_strong_count(value) };
        [
            value,
            Rc::into_raw(Rc::new(Str::new(""))),
            Rc::into_raw(Rc::new(Str::new(""))),
        ]
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
fn unquote(source: &str, first: usize) -> Option<Str> {
    let input = source.as_bytes();
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
        Rc::into_raw(Rc::new(text))
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
        unsafe { Rc::increment_strong_count(value) };
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
            &Rc::new(Str::new(source)),
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
            let error = Rc::from_raw(imre_error_take());
            assert_eq!(imre_exception_matches(Rc::as_ptr(&error), kind as u32), 1);
            let actual = Rc::from_raw(imre_exception_message(Rc::as_ptr(&error)));
            assert_eq!(actual.as_str(), message);
        }
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
            let result = unsafe { Rc::from_raw(imre_str_dedent(&source)) };
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
            let result = unsafe { Rc::from_raw(imre_str_dedent(&source)) };
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
        let source = Rc::new(Str::new("an independently owned source without separators"));
        let separator = Rc::new(Str::new("::"));
        // SAFETY: inputs remain rooted; both native getters transfer owned handles.
        unsafe {
            let parts = Rc::from_raw(imre_str_split(
                Rc::as_ptr(&source),
                Rc::as_ptr(&separator),
                Int::small(-1).unwrap().0,
                0,
            ));
            assert_eq!(Rc::strong_count(&source), 2);
            let selected =
                Rc::from_raw(super::super::imre_seq_get_index(Rc::as_ptr(&parts), 0) as *const Str);
            assert!(Rc::ptr_eq(&source, &selected));
            drop(source);
            drop(parts);
            assert_eq!(Rc::strong_count(&selected), 1);
            assert_eq!(
                selected.as_str(),
                "an independently owned source without separators"
            );
        }
    }

    #[test]
    fn rsplit_last_keeps_an_independent_owner_and_reports_empty_separators() {
        let source = Rc::new(Str::new("an independently owned string without separators"));
        let separator = Rc::new(Str::new("::"));
        // SAFETY: both inputs are rooted; the successful result transfers one owner.
        let result = unsafe {
            Rc::from_raw(imre_str_rsplit_last(
                Rc::as_ptr(&source),
                Rc::as_ptr(&separator),
            ))
        };
        assert!(Rc::ptr_eq(&source, &result));
        drop(source);
        assert_eq!(Rc::strong_count(&result), 1);
        assert_eq!(
            result.as_str(),
            "an independently owned string without separators"
        );
        let empty = Rc::new(Str::new(""));
        // SAFETY: inputs remain rooted; failure returns null and one pending error.
        unsafe {
            assert!(imre_str_rsplit_last(Rc::as_ptr(&result), Rc::as_ptr(&empty)).is_null());
            let error = imre_error_take();
            assert!(!error.is_null());
            let error = Rc::from_raw(error);
            assert_eq!(
                imre_exception_matches(Rc::as_ptr(&error), Kind::ValueError as u32),
                1,
            );
            let message = Rc::from_raw(imre_exception_message(Rc::as_ptr(&error)));
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
        let source = Rc::new(Str::new("x"));
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
            let original = Rc::new(Str::new(source));
            // SAFETY: the input is rooted; the returned independent owner is
            // consumed by the Rc wrapper, including when the handle is reused.
            let actual = unsafe { Rc::from_raw(imre_str_unquote(Rc::as_ptr(&original))) };
            assert_eq!(actual.as_str(), expected, "{source}");
        }
    }
}
