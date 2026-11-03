//! Process text streams and their native encoding and buffering.
pub(crate) mod context_warnings;
#[cfg(target_os = "linux")]
pub(crate) mod cpu_count;
mod recursion;

use super::{Int, LazyLock, Mutex, OUTPUT, Output, Str, flush_stdout, initialize_io};
pub(crate) use recursion::{recursion_limit, set_recursion_limit};
mod intern;
use crate::StrOwner;
use crate::objects::{Codec, errno_error};
pub(crate) use intern::{canonical, intern_owner, try_intern_owner};
use std::io::{self, Write};
mod encoding;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) use encoding::langinfo;
mod warnoptions;
pub(crate) use encoding::{
    file_encoding, imre_sys_utf8_mode, set_locale, strerror, with_errno_message,
    with_process_locale,
};
pub(crate) use warnoptions::startup_warning_options;
mod input;
pub(super) use input::read_input_line;
mod encoding_names;
pub(crate) use encoding_names::{default_encoding, filesystem_encoding, filesystem_errors};

/// Apply startup settings in CPython's validation order before any user code can run.
#[unsafe(no_mangle)]
pub extern "C" fn imre_process_config_init(
    ignore_environment: u32,
    cpus: i32,
    context_warnings: i32,
) {
    crate::python_hash::initialize(ignore_environment != 0);
    crate::integer_text::imre_integer_digits_init(ignore_environment);
    #[cfg(target_os = "linux")]
    cpu_count::initialize(cpus);
    #[cfg(not(target_os = "linux"))]
    let _ = cpus;
    self::context_warnings::imre_context_warnings_init(context_warnings);
}

pub(super) fn initialize_encoding(isolated: bool, utf8: Option<bool>) {
    encoding::initialize(isolated, utf8);
}

pub(super) struct StderrOutput {
    stream: Output,
    codec: Codec,
}

impl Write for StderrOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_all(bytes)?;
        Ok(bytes.len())
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if matches!(self.codec, Codec::Utf8 { signature: false }) || self.stream.signature_emitted {
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
            self.stream.signature_emitted = true;
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
        stream: Output::with_stream(
            super::RawOutput::Stderr,
            super::stdio_settings().stderr_capacity,
            true,
        ),
        codec: encoding::settings(false, None).codec,
    })
});

/// Each standard stream retains its own metadata. Single-character names share the
/// same Latin-1 owners as other Python strings.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stream_text(descriptor: u32, errors: u32) -> *const Str {
    StrOwner::into_raw(standard_field(descriptor, errors != 0))
}

pub(super) fn standard_field(descriptor: u32, errors: bool) -> StrOwner {
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
        if errors { handler } else { name }.clone()
    })
}

/// Allocate exception input only when formatting produced borrowed text.
pub(super) fn error_text(text: &str) -> super::Result<StrOwner> {
    StrOwner::try_new(Str::new(text)).map_err(|_| crate::Error::allocation())
}

fn stdout_chunk(
    out: &mut Output,
    text: &str,
    error_input: impl FnOnce() -> super::Result<StrOwner>,
) -> super::Result<()> {
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
pub(super) fn write_stdout_with(
    text: &str,
    newline: bool,
    error_input: impl FnOnce() -> super::Result<StrOwner>,
) -> super::Result<()> {
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

pub(super) fn flush_standard(stderr: bool) -> io::Result<()> {
    if stderr {
        flush_stderr()
    } else {
        flush_stdout()
    }
}

/// The incremental UTF-8-SIG encoder consumes the signature on its first
/// attempt, even if that write subsequently raises UnicodeEncodeError.
pub(crate) fn standard_write_encoding(stderr: bool) -> &'static str {
    let codec = encoding::settings(false, None).codec;
    if !matches!(codec, Codec::Utf8 { signature: true }) {
        return codec.name();
    }
    let emitted = if stderr {
        let mut err = STDERR.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::replace(&mut err.stream.signature_emitted, true)
    } else {
        let mut out = OUTPUT.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::replace(&mut out.signature_emitted, true)
    };
    if emitted { "utf-8" } else { codec.name() }
}

/// Non-scalar Python strings are encoded by the boxed codec. Successful writes
/// still share the original text buffer and its line buffering.
pub(crate) fn write_standard_encoded(
    stderr: bool,
    bytes: &[u8],
    newline: bool,
) -> super::Result<()> {
    fn write<W: Write>(out: &mut Output<W>, bytes: &[u8], newline: bool) -> io::Result<()> {
        if !out.captures.is_empty() {
            return out.write_all(bytes);
        }
        out.write_chunk(bytes, out.line_buffered && newline)
    }
    if stderr {
        let mut err = STDERR.lock().unwrap_or_else(|p| p.into_inner());
        write(&mut err.stream, bytes, newline)
    } else {
        let mut out = OUTPUT.lock().unwrap_or_else(|p| p.into_inner());
        write(&mut out, bytes, newline)
    }
    .map_err(errno_error)
}

fn with_standard_output<T>(stderr: bool, operation: impl FnOnce(&mut Output) -> T) -> T {
    if stderr {
        operation(&mut stderr_writer().stream)
    } else {
        operation(&mut OUTPUT.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

/// Write bytes without flushing the text that precedes them.
pub(crate) fn write_standard_buffer(stderr: bool, bytes: &[u8]) -> super::Result<()> {
    with_standard_output(stderr, |out| out.raw.write_all(bytes)).map_err(errno_error)
}

pub(crate) fn flush_standard_buffer(stderr: bool) -> io::Result<()> {
    with_standard_output(stderr, |out| out.raw.flush())
}

/// Text close attempts both flushes; either view discards any bytes left after close.
pub(crate) fn close_standard_output(stderr: bool, flush_text: bool) -> io::Result<()> {
    with_standard_output(stderr, |out| {
        let text = if flush_text { out.flush() } else { Ok(()) };
        let binary = out.raw.flush();
        out.text = Default::default();
        out.raw.buffer.clear();
        out.raw.written = 0;
        binary.and(text)
    })
}

/// Seek after draining binary writes, or report their logical position without flushing.
pub(crate) fn standard_buffer_position(
    stderr: bool,
    seek: Option<(i64, i32)>,
) -> super::Result<i64> {
    let fd = if stderr { 2 } else { 1 };
    with_standard_output(stderr, |out| {
        if let Some((offset, whence)) = seek {
            out.raw.flush().map_err(errno_error)?;
            crate::objects::os::seek(fd, offset, whence)
        } else {
            let offset = crate::objects::os::seek(fd, 0, libc::SEEK_CUR)?;
            let buffered = out.raw.buffer.len() - out.raw.written;
            offset
                .checked_add(buffered as i64)
                .ok_or_else(|| errno_error(io::Error::from_raw_os_error(libc::EOVERFLOW)))
        }
    })
}

/// Escaped displayhook bytes go to stdout.buffer ahead of any previously buffered text.
pub(super) fn write_escaped_stdout(text: &str) -> super::Result<()> {
    let settings = encoding::settings(false, None);
    let bytes = settings
        .codec
        .encode(text, Some("backslashreplace"), || error_text(text))?;
    write_standard_buffer(false, &bytes)
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

/// Field selectors 0 and 1 retain the original name and version ABI.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_thread_text(field: u32) -> *const Str {
    let text = match field {
        0 => thread_name().map(Str::new),
        1 => thread_version().map(Str::new),
        2 => thread_lock_backend().map(Str::new),
        _ => None,
    };
    text.map_or(std::ptr::null(), |text| {
        StrOwner::into_raw(StrOwner::new(text))
    })
}

pub(crate) fn thread_name() -> Option<&'static str> {
    if cfg!(windows) {
        Some("nt")
    } else if cfg!(all(unix, not(target_family = "wasm")))
        || cfg!(all(target_os = "emscripten", target_feature = "atomics"))
    {
        Some("pthread")
    } else {
        None
    }
}

pub(crate) fn thread_lock_backend() -> Option<&'static str> {
    // PrimitiveLock waits on a condition variable while holding its state mutex.
    (thread_name() == Some("pthread")).then_some("mutex+cond")
}

pub(crate) fn thread_version() -> Option<String> {
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
            return Some(value.to_owned());
        }
    }
    None
}

pub(crate) const OS_NAME: &str = if cfg!(windows) { "nt" } else { "posix" };

/// Return the OS interface Python exposes on the target that built this runtime.
#[unsafe(no_mangle)]
pub extern "C" fn imre_os_name() -> *const Str {
    StrOwner::into_raw(canonical(OS_NAME))
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
