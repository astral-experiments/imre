//! Process text streams and their native encoding and buffering.
use super::{Int, LazyLock, Mutex, OUTPUT, Output, Rc, Str, flush_stdout, initialize_io};
use crate::objects::{Codec, errno_error};
use std::io::{self, Write};
use std::sync::OnceLock;

static STDERR_CODEC: OnceLock<Codec> = OnceLock::new();

/// Snapshot the encoder before user code can change the environment.
pub(super) fn stderr_codec(isolated: bool) -> Codec {
    *STDERR_CODEC.get_or_init(|| {
        let utf8 = Codec::Utf8 { signature: false };
        if isolated {
            return utf8;
        }
        let setting = match std::env::var("PYTHONIOENCODING") {
            Ok(setting) => setting,
            Err(std::env::VarError::NotPresent) => return utf8,
            Err(std::env::VarError::NotUnicode(_)) => {
                let _ = writeln!(io::stderr().lock(), "PYTHONIOENCODING is not valid UTF-8");
                std::process::exit(1);
            }
        };
        let name = setting
            .split_once(':')
            .map_or(setting.as_str(), |(name, _)| name);
        if name.is_empty() {
            return utf8;
        }
        Codec::lookup(name).unwrap_or_else(|_| {
            let _ = writeln!(io::stderr().lock(), "LookupError: unknown encoding: {name}");
            std::process::exit(1);
        })
    })
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
        self.stream.write_all(&encoded)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

static STDERR: LazyLock<Mutex<StderrOutput>> = LazyLock::new(|| {
    initialize_io();
    Mutex::new(StderrOutput {
        stream: Output::with_stream(io::stderr(), super::STDIO_SETTINGS.stderr_capacity, true),
        codec: stderr_codec(false),
    })
});

/// Return the canonical Python codec name captured at native program startup.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stderr_encoding() -> *const Str {
    Rc::into_raw(stderr_codec(false).name_owner())
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
    let result = if stderr != 0 {
        write_stderr(text, false)
    } else {
        write_stream(&OUTPUT, text, false)
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
