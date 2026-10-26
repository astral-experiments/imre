//! Process text streams and their native encoding and buffering.
mod recursion;
use super::{Int, LazyLock, Mutex, OUTPUT, Output, Str, flush_stdout, initialize_io};
mod intern;
use crate::StrOwner;
use crate::objects::{Codec, errno_error};
use std::io::{self, Write};
mod encoding;
mod input;
pub(super) use input::read_input_line;

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
            .encode(text, Some("backslashreplace"), || error_text(text))
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

/// Each standard stream retains its own metadata. Single-character names share the
/// same Latin-1 owners as other Python strings.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stream_text(descriptor: u32, errors: u32) -> *const Str {
    thread_local! {
        static FIELDS: [(StrOwner, StrOwner); 3] = {
            let settings = encoding::settings(false, None);
            let name = settings.codec.name_owner();
            std::array::from_fn(|fd| {
                let errors = if fd == 2 { "backslashreplace" } else { settings.errors.as_ref() };
                (crate::objects::literal_string(&name), crate::objects::literal_string(errors))
            })
        };
    }
    FIELDS.with(|fields| {
        let (name, handler) = &fields[descriptor as usize];
        StrOwner::into_raw(if errors != 0 { handler } else { name }.clone())
    })
}

// Preserve the ABI of earlier generated stream accesses.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stream_encoding() -> *const Str {
    imre_sys_stream_text(1, 0)
}
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stderr_encoding() -> *const Str {
    imre_sys_stream_text(2, 0)
}
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stdout_errors() -> *const Str {
    imre_sys_stream_text(1, 1)
}

/// Allocate exception input only when formatting produced borrowed text.
fn error_text(text: &str) -> super::Result<StrOwner> {
    StrOwner::try_new(Str::new(text)).map_err(|_| crate::Error::allocation())
}

fn stdout_chunk(out: &mut Output, text: &str, error_input: impl FnOnce() -> super::Result<StrOwner>) -> super::Result<()> {
    let settings = encoding::settings(false, None);
    if !out.captures.is_empty()
        || matches!(settings.codec, Codec::Utf8 { signature: false })
        || (matches!(settings.codec, Codec::Utf8 { signature: true }) && out.signature_emitted)
    {
        return out.write_all(text.as_bytes()).map_err(errno_error);
    }
    let encoded = settings
        .codec
        .encode(text, Some(settings.errors.as_ref()), error_input)?;
    if matches!(settings.codec, Codec::Utf8 { signature: true }) {
        out.signature_emitted = true;
    }
    out.write_encoded(&encoded, text.as_bytes())
        .map_err(errno_error)
}

/// Encode before changing pending bytes, preserving the Python error on failure.
pub(super) fn write_stdout(text: &str, newline: bool) -> super::Result<()> {
    write_stdout_with(text, newline, || error_text(text))
}

/// Keep the original Python string when a direct stream write fails to encode it.
pub(super) fn write_stdout_with(text: &str, newline: bool, error_input: impl FnOnce() -> super::Result<StrOwner>) -> super::Result<()> {
    let mut out = OUTPUT.lock().unwrap_or_else(|poison| poison.into_inner());
    stdout_chunk(&mut out, text, error_input)?;
    if newline {
        stdout_chunk(&mut out, "\n", || error_text("\n"))?;
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

/// sys stream methods and default warning display share the buffered destinations.
pub(super) fn write_standard_stream(stderr: bool, text: &str) -> super::Result<()> {
    if stderr {
        write_stderr(text, false).map_err(errno_error)
    } else {
        write_stdout(text, false)
    }
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
    StrOwner::into_raw(StrOwner::new(Str::new(platform)))
}

/// Return a field of sys.thread_info from the target that runs the program.
/// A null pointer represents an unknown library version or thread provider.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_thread_text(version: u32) -> *const Str {
    if version != 0 {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            let mut buffer = [0u8; 256];
            // SAFETY: confstr writes at most buffer.len() bytes, including the terminating nul.
            let length = unsafe {
                libc::confstr(
                    libc::_CS_GNU_LIBPTHREAD_VERSION,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if (2..=buffer.len()).contains(&length)
                && let Ok(value) = std::str::from_utf8(&buffer[..length - 1])
            {
                return StrOwner::into_raw(StrOwner::new(Str::new(value)));
            }
        }
        return std::ptr::null();
    }

    let name = if cfg!(windows) {
        "nt"
    } else if cfg!(all(unix, not(target_family = "wasm")))
        || cfg!(all(target_os = "emscripten", target_feature = "atomics"))
    {
        "pthread"
    } else {
        return std::ptr::null();
    };
    StrOwner::into_raw(StrOwner::new(Str::new(name)))
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
        write_stderr(text, false).map_err(errno_error)
    } else {
        write_stdout_with(text, false, || {
            // SAFETY: the caller roots this exact string throughout the write.
            Ok(unsafe { StrOwner::acquire_word(text as *const Str as usize) })
        })
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
