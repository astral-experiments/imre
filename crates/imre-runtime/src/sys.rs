//! The process's two UTF-8 text output streams use the same buffering as print.
use super::{Int, LazyLock, Mutex, OUTPUT, Output, Rc, Str, flush_stdout, initialize_io};
use crate::objects::errno_error;
use std::io::{self, Write};

static STDERR: LazyLock<Mutex<Output<io::Stderr>>> = LazyLock::new(|| {
    initialize_io();
    Mutex::new(Output::with_stream(
        io::stderr(),
        super::STDIO_SETTINGS.stderr_capacity,
        true,
    ))
});

fn write_stream<W: Write>(stream: &Mutex<Output<W>>, text: &[u8], newline: bool) -> io::Result<()> {
    let mut stream = stream.lock().unwrap_or_else(|poison| poison.into_inner());
    stream.write_all(text)?;
    if newline {
        stream.write_all(b"\n")?;
    }
    Ok(())
}

pub(crate) fn write_standard(stderr: bool, bytes: &[u8]) -> io::Result<()> {
    if stderr {
        write_stream(&STDERR, bytes, false)
    } else {
        write_stream(&OUTPUT, bytes, false)
    }
}

pub(crate) fn flush_standard(stderr: bool) -> io::Result<()> {
    if stderr {
        flush_stderr()
    } else {
        flush_stdout()
    }
}

pub(super) fn write_stderr(text: &str, newline: bool) -> io::Result<()> {
    write_stream(&STDERR, text.as_bytes(), newline)
}

pub(super) fn stderr_writer() -> std::sync::MutexGuard<'static, Output<io::Stderr>> {
    STDERR.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub(super) fn flush_stderr() -> io::Result<()> {
    if let Some(stream) = LazyLock::get(&STDERR) {
        stream
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .flush()?;
    }
    Ok(())
}

/// Write the default display hook's repr followed by a newline to stdout.
pub(super) fn write_display_line(text: &str) -> bool {
    if let Some(result) = crate::objects::stdio::write_line(text.as_bytes(), true) {
        if let Err(error) = result {
            error.raise();
            return false;
        }
        return true;
    }
    match write_stream(&OUTPUT, text.as_bytes(), true) {
        Ok(()) => true,
        Err(error) => errno_error(error).returning(false),
    }
}

/// Return the Python platform name of the target that built this runtime.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_platform() -> *const Str {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    Rc::into_raw(Rc::new(Str::new(platform)))
}

/// Return the OS interface Python exposes on the target that built this runtime.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_name() -> *const Str {
    Rc::into_raw(Rc::new(Str::new(if cfg!(windows) {
        "nt"
    } else {
        "posix"
    })))
}

/// Write text without a newline and return the number of Unicode characters.
///
/// # Safety
/// The caller roots the UTF-8 string throughout the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sys_stream_write(stderr: u32, text: *const Str) -> usize {
    // SAFETY: the caller roots the string until the stream write finishes.
    let text = unsafe { &*text };
    let result = if stderr != 0 {
        write_stderr(text, false)
    } else {
        write_stream(&OUTPUT, text.as_bytes(), false)
    };
    match result {
        Ok(()) => Int::from_usize(text.chars().count()).into_raw(),
        Err(error) => errno_error(error).returning(0),
    }
}

/// Flush only the selected stream, even if the other stream has buffered data.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stream_flush(stderr: u32) {
    let result = if stderr != 0 {
        flush_stderr()
    } else {
        flush_stdout()
    };
    if let Err(error) = result {
        errno_error(error).raise();
    }
}
