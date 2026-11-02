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

/// An explicit Python hash seed, or None for the default random key.
#[cfg(unix)]
pub fn environment_hash_seed() -> Result<Option<u32>, &'static str> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Some(raw) = std::env::var_os("PYTHONHASHSEED") else {
        return Ok(None);
    };
    if raw.is_empty() || raw == "random" {
        return Ok(None);
    }
    let text = CString::new(raw.as_bytes()).expect("environment values contain no NUL");
    let mut end = std::ptr::null_mut();
    // SAFETY: text is NUL-terminated. strtoul sets end inside text or at its terminator.
    let (seed, consumed) = unsafe {
        let seed = libc::strtoul(text.as_ptr(), &mut end, 10);
        (seed, *end == 0)
    };
    if !consumed || seed > u32::MAX.into() {
        return Err(
            "Fatal Python error: config_init_hash_seed: PYTHONHASHSEED must be \"random\" or an integer in range [0; 4294967295]\nPython runtime state: preinitialized\n",
        );
    }
    Ok(Some(seed as u32))
}

/// Read the decimal digit limit, whose validation precedes the CPU-count startup setting.
pub fn environment_digit_limit() -> Result<Option<u32>, &'static str> {
    let Some(raw) = std::env::var_os("PYTHONINTMAXSTRDIGITS").filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let limit = raw.to_str()
        .and_then(|s| s.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']).parse::<i32>().ok())
        .filter(|&n| n == 0 || n >= 640)
        .ok_or("Fatal Python error: config_init_int_max_str_digits: PYTHONINTMAXSTRDIGITS: invalid limit; must be >= 640 or 0 for unlimited.\nPython runtime state: preinitialized\n")?;
    Ok(Some(limit as u32))
}

/// Resolve a CPython CPU count setting; zero denotes the host's default.
pub fn parse_cpu_count(raw: &OsStr) -> Option<i32> {
    if raw == "default" {
        return Some(0);
    }
    raw.to_str()?
        .trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r'])
        .parse::<i32>()
        .ok()
        .filter(|n| *n > 0)
}

/// Select the default or the positive count configured in the environment.
pub fn environment_cpu_count() -> Option<i32> {
    std::env::var_os("PYTHON_CPU_COUNT")
        .filter(|raw| !raw.is_empty())
        .map_or(Some(0), |raw| parse_cpu_count(&raw))
}

pub const CPU_COUNT_ERROR: &str = "Fatal Python error: config_init_cpu_count: -X cpu_count=n option: n is missing or an invalid number, n must be greater than 0\nPython runtime state: preinitialized\n";
