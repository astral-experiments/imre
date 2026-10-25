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

fn write_stream<W: Write>(stream: &Mutex<Output<W>>, text: &str, newline: bool) -> io::Result<()> {
    let mut stream = stream.lock().unwrap_or_else(|poison| poison.into_inner());
    stream.write_all(text.as_bytes())?;
    if newline {
        stream.write_all(b"\n")?;
    }
    Ok(())
}

pub(super) fn write_stderr(text: &str, newline: bool) -> io::Result<()> {
    write_stream(&STDERR, text, newline)
}

/// sys stream methods and default warning display share the buffered destinations.
pub(super) fn write_standard_stream(stderr: bool, text: &str) -> io::Result<()> {
    if stderr {
        write_stderr(text, false)
    } else {
        write_stream(&OUTPUT, text, false)
    }
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
    match write_stream(&OUTPUT, text, true) {
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
    let result = write_standard_stream(stderr != 0, text);
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
