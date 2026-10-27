//! POSIX shell patterns treat path separators and leading dots as ordinary characters.
use crate::StrOwner;
use std::borrow::Cow;

use super::exceptions::{Error, Kind, finish};
use super::paths::{Pattern, class_characters, class_end};
use super::words::Words;
use super::{Bytes, Object, Str, object};

pub(crate) fn compile(pattern: &str) -> impl Fn(&str) -> bool + use<> {
    let pattern = Pattern::component(pattern);
    move |name| pattern.matches(name)
}

pub(crate) fn byte_spelling(bytes: &[u8]) -> Cow<'_, str> {
    if bytes.is_ascii() {
        Cow::Borrowed(std::str::from_utf8(bytes).expect("ASCII is UTF-8"))
    } else {
        Cow::Owned(bytes.iter().copied().map(char::from).collect())
    }
}

/// Bytes patterns match single bytes, including values that also form valid UTF-8.
unsafe fn spelling<'a>(raw: *const (), bytes: bool) -> Cow<'a, str> {
    if bytes {
        // SAFETY: the caller passes a rooted Bytes owner with the indicated tag.
        byte_spelling(unsafe { &*raw.cast::<Bytes>() })
    } else {
        // SAFETY: the caller passes a rooted Str owner with the indicated tag.
        Cow::Borrowed(unsafe { &*raw.cast::<Str>() }.as_str())
    }
}

pub(crate) fn mixed_message(pattern_bytes: bool) -> &'static str {
    if pattern_bytes {
        "cannot use a bytes pattern on a string-like object"
    } else {
        "cannot use a string pattern on a bytes-like object"
    }
}

/// # Safety
/// Each input is an independently rooted Str or Bytes owner selected by its tag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_fnmatch(
    name: *const (),
    pat: *const (),
    name_bytes: u32,
    pat_bytes: u32,
) -> u32 {
    if name_bytes != pat_bytes {
        return Error::new(Kind::TypeError, mixed_message(pat_bytes != 0)).returning(0);
    }
    // SAFETY: both tags choose the actual immutable owner representations.
    let (name, pat) = unsafe {
        (
            spelling(name, name_bytes != 0),
            spelling(pat, pat_bytes != 0),
        )
    };
    u32::from(compile(&pat)(&name))
}

/// # Safety
/// The sequence stores rooted str, bytes, or their compiler-described union.
/// The pattern is an independently rooted immutable owner with the indicated tag.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_fnmatch_filter(
    names: *const Object,
    pat: *const (),
    pat_bytes: u32,
    inverse: u32,
) -> *const Object {
    // SAFETY: the pattern's tag and the list layout were checked at the call site.
    let matches = compile(&unsafe { spelling(pat, pat_bytes != 0) });
    let filter = |storage: &super::Storage| {
        let source = storage.sequence()?;
        let mut output = Words::new(source.kind.raw());
        for &word in &source.values {
            let (name_bytes, pointer) = match source.kind.raw() {
                3 => (false, word),
                8 => (true, word),
                9 => (word & 7 == 1, word & !7), // the closed str | bytes union
                _ => unreachable!("fnmatch filter input must contain str or bytes"),
            };
            if name_bytes != (pat_bytes != 0) {
                return Err(Error::new(Kind::TypeError, mixed_message(pat_bytes != 0)));
            }
            // SAFETY: the immutable list roots every element throughout this scan.
            let name = unsafe { spelling(pointer as *const (), name_bytes) };
            if matches(&name) != (inverse != 0) {
                output
                    .values
                    .try_reserve(1)
                    .map_err(|_| Error::allocation())?;
                // SAFETY: the fresh result acquires the very same Python object.
                output
                    .values
                    .push(unsafe { source.kind.acquire(word) }.into_word());
            }
        }
        Ok(crate::ObjectOwner::into_raw(output.into_object()))
    };
    // SAFETY: names remains rooted; the scan does not call back into Python.
    let result = unsafe { object(names).0.with(filter) };
    finish(result, std::ptr::null())
}

fn translate_class(chars: &[char]) -> String {
    let characters = class_characters(chars);
    match characters.as_slice() {
        [] => return "(?!)".to_owned(),
        [('!', _)] => return ".".to_owned(),
        _ => {}
    }
    let mut text = String::from("[");
    for (i, (c, range_hyphen)) in characters.into_iter().enumerate() {
        if i == 0 && c == '!' {
            text.push('^');
            continue;
        }
        if matches!(c, '\\' | '&' | '~' | '|')
            || (c == '-' && !range_hyphen)
            || (i == 0 && matches!(c, '^' | '['))
        {
            text.push('\\');
        }
        text.push(c);
    }
    text.push(']');
    text
}

fn translated_parts(pat: &str, star: &str, question: &str) -> (Vec<String>, Vec<usize>) {
    let chars: Vec<_> = pat.chars().collect();
    let (mut parts, mut stars) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        let part = match c {
            '*' => {
                stars.push(parts.len());
                while chars.get(i) == Some(&'*') {
                    i += 1;
                }
                star.to_owned()
            }
            '?' => question.to_owned(),
            '[' if let Some(end) = class_end(&chars, i) => {
                let text = translate_class(&chars[i..end]);
                i = end + 1;
                text
            }
            c if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) => format!("\\{c}"),
            c => c.to_string(),
        };
        parts.push(part);
    }
    (parts, stars)
}

fn translate(pat: &str) -> String {
    let (parts, stars) = translated_parts(pat, "*", ".");
    let mut regex = String::from("(?s:");
    let mut start = 0;
    for (i, star) in stars.iter().copied().enumerate() {
        for part in &parts[start..star] {
            regex.push_str(part);
        }
        if i != 0 {
            regex.push(')');
        }
        regex.push_str(if i + 1 == stars.len() { ".*" } else { "(?>.*?" });
        start = star + 1;
    }
    for part in &parts[start..] {
        regex.push_str(part);
    }
    regex.push_str(")\\z");
    regex
}

/// # Safety
/// The input borrows a live str. A successful return transfers a new str owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_fnmatch_translate(pat: *const Str) -> *const Str {
    // SAFETY: the caller roots the checked string for the duration of the translation.
    let regex = translate(unsafe { &*pat }.as_str());
    finish(
        Str::try_new(&regex)
            .map(|s| StrOwner::into_raw(StrOwner::new(s)))
            .map_err(|_| Error::allocation()),
        std::ptr::null(),
    )
}

/// # Safety
/// `pat` is a rooted bytes owner. Only the empty bytes input has no integer
/// elements to pass to CPython's regular expression escaping operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_fnmatch_translate_bytes(pat: *const Bytes) -> *const Str {
    // SAFETY: the caller keeps the exact bytes allocation alive for this check.
    if unsafe { &*pat }.is_empty() {
        return StrOwner::into_raw(super::text::owned_string("(?s:)\\z"));
    }
    Error::new(
        Kind::TypeError,
        "decoding to str: need a bytes-like object, int found",
    )
    .returning(std::ptr::null())
}

/// # Safety
/// Root all strings and a new zeroed tuple for the parts and star-index lists.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_fnmatch_parts(
    pat: *const Str,
    star: *const Str,
    question: *const Str,
    out: *mut *const (),
) {
    // SAFETY: the compiler keeps all three exact string handles alive.
    let (parts, stars) =
        unsafe { translated_parts((&*pat).as_str(), (&*star).as_str(), (&*question).as_str()) };
    let mut words = Words::new(3);
    for part in parts {
        words
            .values
            .push(StrOwner::into_raw(StrOwner::new(Str::from(part))) as usize);
    }
    let indices = crate::new_list(stars.into_iter().map(crate::Int::from_usize).collect());
    // SAFETY: both slots in the fresh tuple now receive their independent owners.
    unsafe {
        out.write(crate::ObjectOwner::into_raw(words.into_object()).cast());
        out.add(1).write(indices.cast());
    }
}
