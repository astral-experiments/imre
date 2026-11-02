//! Warning options are ordered from the environment first, then the command line.
use std::ffi::{CStr, c_char};

pub(crate) fn startup_warning_options(
    ignore_environment: bool,
    command: Vec<String>,
) -> Vec<String> {
    let mut options = Vec::new();
    let environment = if ignore_environment {
        None
    } else {
        std::env::var("PYTHONWARNINGS").ok()
    };
    for option in environment
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .chain(command.iter().map(String::as_str))
    {
        if !options.iter().any(|old| old == option) {
            options.push(option.to_owned());
        }
    }
    options
}

/// Apply startup filters before a statically compiled entry begins.
///
/// # Safety
/// A non-null pointer must identify the compiler's NUL-terminated UTF-8 JSON array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_warning_options_init(
    ignore_environment: u32,
    options: *const c_char,
) {
    let command = if options.is_null() {
        Vec::new()
    } else {
        // SAFETY: the compiler emits a static, NUL-terminated JSON constant.
        serde_json::from_slice(unsafe { CStr::from_ptr(options) }.to_bytes())
            .expect("compiler warning options")
    };
    for option in startup_warning_options(ignore_environment != 0, command) {
        if let Err(reason) = crate::objects::warnings::startup_filter(&option) {
            eprintln!("Invalid -W option ignored: {reason}");
        }
    }
}
