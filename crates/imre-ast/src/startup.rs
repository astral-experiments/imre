//! Parsing shared by the Python launcher and the entry point of a native program.
use std::ffi::OsStr;

/// Parse the numeric argument accepted by CPython's context-aware warnings setting.
pub fn parse_context_flag(raw: &OsStr) -> Option<bool> {
    let text = raw.to_str()?;
    if text.is_empty() {
        return Some(false);
    }
    let digits = text.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    match digits.parse::<i32>().ok()? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}
