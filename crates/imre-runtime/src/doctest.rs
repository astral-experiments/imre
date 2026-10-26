//! Capture Python standard output at the stream shared by print and sys.stdout.
//! Nested examples restore their caller's stream; stderr is never captured.

use super::{Error, Kind, OUTPUT, Str};
use crate::StrOwner;

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
    let Ok(mut text) = String::from_utf8(bytes) else {
        return Error::new(Kind::UnicodeError, "captured stdout is not UTF-8")
            .returning(std::ptr::null());
    };
    if !text.is_empty() && !text.ends_with('\n') {
        if text.try_reserve(1).is_err() {
            return Error::allocation().returning(std::ptr::null());
        }
        text.push('\n');
    }
    StrOwner::into_raw(StrOwner::new(Str::from(text)))
}
