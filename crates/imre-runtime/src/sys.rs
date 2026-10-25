//! Process text streams and their native encoding and buffering.
mod intern;
use super::{Int, LazyLock, Mutex, OUTPUT, Output, Rc, Str, flush_stdout, initialize_io};
use crate::objects::{Codec, errno_error};
pub(crate) use intern::canonical;
use std::io::{self, Write};
mod encoding;
mod encoding_names;

pub(super) fn initialize_encoding(isolated: bool, utf8: Option<bool>) {
    encoding::settings(isolated, utf8);
}

pub(super) struct StderrOutput {
    stream: Output<io::Stderr>,
    codec: Codec,
}

impl Write for StderrOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_all(bytes)?;
        Ok(bytes.len())
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if matches!(self.codec, Codec::Utf8 { signature: false }) {
            return self.stream.write_all(bytes);
        }
        let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
        let encoded = self
            .codec
            .encode(text, Some("backslashreplace"))
            .map_err(|_| {
                io::Error::other("could not encode standard error with backslashreplace")
            })?;
        if matches!(self.codec, Codec::Utf8 { signature: true }) {
            // The incremental encoder emits its signature once, even for an empty first write.
            self.codec = Codec::Utf8 { signature: false };
        }
        self.stream.write_encoded(&encoded, text.as_bytes())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

static STDERR: LazyLock<Mutex<StderrOutput>> = LazyLock::new(|| {
    initialize_io();
    Mutex::new(StderrOutput {
        stream: Output::with_stream(io::stderr(), super::STDIO_SETTINGS.stderr_capacity, true),
        codec: encoding::settings(false, None).codec,
    })
});

/// Return the canonical Python codec name captured at native program startup.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stream_encoding() -> *const Str {
    Rc::into_raw(encoding::settings(false, None).codec.name_owner())
}

// Preserve the ABI of existing native stderr callers.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stderr_encoding() -> *const Str {
    imre_sys_stream_encoding()
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stdout_errors() -> *const Str {
    thread_local! {
        static ERRORS: Rc<Str> = Rc::new(Str::new(encoding::settings(false, None).errors.as_ref()));
    }
    ERRORS.with(|errors| Rc::into_raw(errors.clone()))
}

fn stdout_chunk(out: &mut Output, text: &str) -> super::Result<()> {
    let settings = encoding::settings(false, None);
    if !out.captures.is_empty()
        || matches!(settings.codec, Codec::Utf8 { signature: false })
        || (matches!(settings.codec, Codec::Utf8 { signature: true }) && out.signature_emitted)
    {
        return out.write_all(text.as_bytes()).map_err(errno_error);
    }
    let encoded = settings
        .codec
        .encode(text, Some(settings.errors.as_ref()))?;
    if matches!(settings.codec, Codec::Utf8 { signature: true }) {
        out.signature_emitted = true;
    }
    out.write_encoded(&encoded, text.as_bytes())
        .map_err(errno_error)
}

/// Encode before changing pending bytes, preserving the Python error on failure.
pub(super) fn write_stdout(text: &str, newline: bool) -> super::Result<()> {
    let mut out = OUTPUT.lock().unwrap_or_else(|poison| poison.into_inner());
    stdout_chunk(&mut out, text)?;
    if newline {
        stdout_chunk(&mut out, "\n")?;
    }
    Ok(())
}

fn write_stream<W: Write>(stream: &Mutex<W>, text: &str, newline: bool) -> io::Result<()> {
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

pub(super) fn stderr_writer() -> std::sync::MutexGuard<'static, StderrOutput> {
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
    match write_stdout(text, true) {
        Ok(()) => true,
        Err(error) => error.returning(false),
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
        write_stderr(text, false).map_err(errno_error)
    } else {
        write_stdout(text, false)
    };
    match result {
        Ok(()) => Int::from_usize(text.chars().count()).into_raw(),
        Err(error) => error.returning(0),
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
