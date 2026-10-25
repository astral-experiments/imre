//! The process's two UTF-8 text output streams use the same buffering as print.
use super::{Error, Int, Kind, LazyLock, Mutex, OUTPUT, Output, Str, flush_stdout, initialize_io};
use crate::StrOwner;
use std::io::{self, Write};

static STDERR: LazyLock<Mutex<Output<io::Stderr>>> = LazyLock::new(|| {
    initialize_io();
    Mutex::new(Output::with_stream(io::stderr(), true))
});

fn write_stream<W: Write>(stream: &Mutex<Output<W>>, text: &str, newline: bool) -> io::Result<()> {
    let mut stream = stream.lock().unwrap_or_else(|poison| poison.into_inner());
    stream.write_all(text.as_bytes())?;
    if newline {
        stream.write_all(b"\n")?;
    }
    if stream.line_buffered && (newline || text.contains(['\r', '\n'])) {
        stream.flush()?;
    }
    Ok(())
}

pub(super) fn write_stderr(text: &str, newline: bool) -> io::Result<()> {
    write_stream(&STDERR, text, newline)
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

fn write_error(stderr: bool) -> Error {
    Error::new(
        Kind::OSError,
        if stderr {
            "could not write standard error"
        } else {
            "could not write standard output"
        },
    )
}

/// Return the Python platform name of the target that built this runtime.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_platform() -> *const Str {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    StrOwner::into_raw(StrOwner::new(Str::new(platform)))
}

/// Return the OS interface Python exposes on the target that built this runtime.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_name() -> *const Str {
    StrOwner::into_raw(StrOwner::new(Str::new(if cfg!(windows) {
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
        write_stream(&OUTPUT, text, false)
    };
    match result {
        Ok(()) => Int::from_usize(text.chars().count()).into_raw(),
        Err(_) => write_error(stderr != 0).returning(0),
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
    if result.is_err() {
        write_error(stderr != 0).raise();
    }
}
