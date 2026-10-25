//! Capture Python standard output at the stream shared by print and sys.stdout.
//! Nested examples restore their caller's stream; stderr is never captured.

use super::{Error, Kind, OUTPUT, Rc, Str};
use std::fmt::Write;

/// Python's ASCII backslash replacement used by doctest.OutputChecker.
///
/// # Safety
/// The caller roots the Unicode input for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_doctest_ascii(value: *const Str) -> *const Str {
    // SAFETY: the generated caller owns this immutable string.
    let text = unsafe { &*value };
    if text.is_ascii() {
        // SAFETY: transfer another ownership share to the checked caller.
        unsafe { Rc::increment_strong_count(value) };
        return value;
    }
    let mut ascii = String::new();
    for ch in text.chars() {
        let n = ch as u32;
        match n {
            ..=127 => ascii.push(ch),
            ..=255 => write!(ascii, "\\x{n:02x}").unwrap(),
            ..=65535 => write!(ascii, "\\u{n:04x}").unwrap(),
            _ => write!(ascii, "\\U{n:08x}").unwrap(),
        }
    }
    Rc::into_raw(Rc::new(Str::from(ascii)))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_doctest_capture_start() {
    let mut output = OUTPUT.lock().unwrap_or_else(|poison| poison.into_inner());
    if output.captures.try_reserve(1).is_err() {
        drop(output);
        Error::allocation().raise();
    } else {
        output.captures.push(Vec::new());
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_doctest_capture_take() -> *const Str {
    let bytes = OUTPUT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .captures
        .pop();
    let Some(bytes) = bytes else {
        return Error::new(Kind::RuntimeError, "no active doctest output capture")
            .returning(std::ptr::null());
    };
    // All Python text output is UTF-8. Like doctest's stream, supply a final
    // newline when an example wrote a partial line.
    let mut text = String::from_utf8(bytes).expect("standard output is Unicode");
    if !text.is_empty() && !text.ends_with('\n') {
        if text.try_reserve(1).is_err() {
            return Error::allocation().returning(std::ptr::null());
        }
        text.push('\n');
    }
    Rc::into_raw(Rc::new(Str::from(text)))
}
