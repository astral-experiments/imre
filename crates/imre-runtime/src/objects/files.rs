//! Sealed UTF-8 text and byte streams. Explicit operations report language errors;
//! final ownership cleanup only closes resources and never changes error state.

use std::fs::OpenOptions;
use std::io;
use std::os::fd::IntoRawFd;
use std::os::unix::fs::MetadataExt;
use std::rc::Rc;

use super::exceptions::{Error, Kind, Result, finish};
use super::{
    Bytes, Int, Object, Storage, StorageCell, Str, StringPart, concat_parts, fail, object,
};
mod buffered;
mod gzip;

use buffered::{Input, Output};

pub(super) struct FileState {
    stream: Option<Stream>,
    // Preserve access mode after close: binary wrong-direction errors take precedence.
    kind: FileKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Text,
    BinaryRead,
    GzipBinaryRead,
    BinaryWrite,
}

enum Stream {
    Read(Input),
    Write(Output),
}

impl Stream {
    /// Closes exactly once even when flushing fails. `into_parts` prevents a
    /// second implicit flush; a close error takes precedence over a flush error.
    fn close(self) -> io::Result<()> {
        let (file, flushed) = match self {
            Self::Read(input) => (input.into_file(), Ok(())),
            Self::Write(writer) => writer.finish(),
        };
        close_file(file).and(flushed)
    }
}

/// Close once, observing delayed I/O errors without retrying a possibly reused descriptor.
pub(super) fn close_file(file: std::fs::File) -> io::Result<()> {
    let descriptor = file.into_raw_fd();
    // SAFETY: into_raw_fd transfers our unique descriptor owner to close.
    if unsafe { libc::close(descriptor) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

impl Drop for FileState {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            // Final release must remain infallible even during error cleanup.
            let _ = stream.close();
        }
    }
}

impl FileState {
    fn stream(&mut self) -> Result<&mut Stream> {
        self.stream
            .as_mut()
            .ok_or_else(|| Error::new(Kind::ValueError, "I/O operation on closed file."))
    }

    fn read(&mut self) -> Result<Str> {
        self.input()?.read_text()
    }

    fn input(&mut self) -> Result<&mut Input> {
        if self.kind == FileKind::BinaryWrite {
            return Err(Error::new(Kind::UnsupportedOperation, "read"));
        }
        let closed = if self.kind != FileKind::Text {
            "read of closed file"
        } else {
            "I/O operation on closed file."
        };
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| Error::new(Kind::ValueError, closed))?;
        let Stream::Read(file) = stream else {
            return Err(Error::new(Kind::UnsupportedOperation, "not readable"));
        };
        Ok(file)
    }

    fn read_size(&mut self, value: &Int) -> Result<Bytes> {
        // GzipFile checks its own closed state before forwarding the argument
        // to BufferedReader. A plain BufferedWriter validates a C int first.
        if self.kind == FileKind::GzipBinaryRead {
            self.stream()?;
        }
        let size = super::bytes::native_size(value)?;
        if self.kind == FileKind::BinaryWrite {
            i32::try_from(size).map_err(|_| {
                Error::new(
                    Kind::OverflowError,
                    "Python int too large to convert to C int",
                )
            })?;
            return Err(Error::new(Kind::UnsupportedOperation, "read"));
        }
        if size < -1 {
            return Err(Error::new(
                Kind::ValueError,
                "read length must be non-negative or -1",
            ));
        }
        self.input()?.read_bytes(usize::try_from(size).ok())
    }

    pub(super) fn write_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        if self.kind == FileKind::BinaryRead {
            return Err(Error::new(Kind::UnsupportedOperation, "write"));
        }
        if self.kind == FileKind::GzipBinaryRead {
            self.stream()?;
            return Err(Error::new(
                Kind::OSError,
                "write() on read-only GzipFile object",
            ));
        }
        let closed = if self.kind != FileKind::Text {
            "write to closed file"
        } else {
            "I/O operation on closed file."
        };
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| Error::new(Kind::ValueError, closed))?;
        let Stream::Write(writer) = stream else {
            return Err(Error::new(Kind::UnsupportedOperation, "not writable"));
        };
        writer.write(bytes).map_err(os_error)
    }

    pub(super) fn write(&mut self, text: &str) -> Result<()> {
        self.write_bytes(text.as_bytes())
    }

    fn flush(&mut self) -> Result<()> {
        if self.kind == FileKind::BinaryWrite && self.stream.is_none() {
            return Err(Error::new(Kind::ValueError, "flush of closed file"));
        }
        match self.stream()? {
            Stream::Read(_) => Ok(()),
            Stream::Write(writer) => writer.flush().map_err(os_error),
        }
    }
}

/// Decodes a complete text stream strictly before normalizing CRLF and lone CR.
/// File reads and captured subprocess streams share these text-mode semantics.
pub(super) fn decode_utf8_text(bytes: Vec<u8>) -> Result<Str> {
    let text = String::from_utf8(bytes).map_err(|error| {
        Error::new(
            Kind::UnicodeDecodeError,
            format!("invalid UTF-8 at byte {}", error.utf8_error().valid_up_to()),
        )
    })?;
    if !text.contains('\r') {
        return Ok(Str::from(text));
    }
    let mut normalized = Str::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(value) = chars.next() {
        if value == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(value);
        }
    }
    Ok(normalized)
}

impl Storage {
    fn file(&self) -> &FileState {
        match self {
            Self::File(file) => file,
            _ => fail("SystemError", "expected a file stream object"),
        }
    }

    pub(super) fn file_mut(&mut self) -> &mut FileState {
        match self {
            Self::File(file) => file,
            _ => fail("SystemError", "expected a file stream object"),
        }
    }
}

pub(super) fn os_error(error: io::Error) -> Error {
    if let Some(gzip) = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<gzip::GzipError>())
    {
        return Error::new(gzip.0, gzip.to_string());
    }
    Error::new(os_error_kind(error.kind()), error.to_string())
}

fn os_error_kind(kind: io::ErrorKind) -> Kind {
    match kind {
        io::ErrorKind::AlreadyExists => Kind::FileExistsError,
        io::ErrorKind::NotFound => Kind::FileNotFoundError,
        io::ErrorKind::PermissionDenied => Kind::PermissionError,
        io::ErrorKind::IsADirectory => Kind::IsADirectoryError,
        io::ErrorKind::NotADirectory => Kind::NotADirectoryError,
        _ => Kind::OSError,
    }
}

/// Format a POSIX errno without a filename, as in OSError(errno, strerror).
pub(super) fn errno_error(error: io::Error) -> Error {
    errno_with_filename(error, None)
}

fn errno_with_filename(error: io::Error, path: Option<&str>) -> Error {
    errno_with_paths(error, |message| {
        if let Some(path) = path {
            message.push_str(": ");
            super::exceptions::push_string_repr(path, message);
        }
    })
}

pub(super) fn errno_with_paths(error: io::Error, paths: impl FnOnce(&mut String)) -> Error {
    let Some(errno) = error.raw_os_error() else {
        return os_error(error);
    };
    let mut buffer = [0; 512];
    // SAFETY: strerror_r writes a terminated string to this private buffer on success.
    if unsafe { libc::strerror_r(errno, buffer.as_mut_ptr(), buffer.len()) } != 0 {
        return os_error(error);
    }
    // SAFETY: the successful call initialized a terminated string.
    let description = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy();
    let mut message = format!("[Errno {errno}] {description}");
    paths(&mut message);
    Error::new(os_error_kind(error.kind()), message)
}

/// Opens a UTF-8 text stream in read, write, append, or exclusive-create mode.
/// Returns an owned Object or null with a pending error.
///
/// # Safety
/// `path` must be a rooted string. `mode` must be compiler-validated as 0 through 3.
/// Like other fallible entries, pending state must be clear on entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_open(path: *const Str, mode: u32) -> *const Object {
    // SAFETY: the caller roots the immutable path throughout opening the stream.
    finish(
        open(unsafe { &*path }, mode, false, false),
        std::ptr::null(),
    )
}

/// Opens an immutable-byte stream in rb, wb, ab, or xb mode (0 through 3).
///
/// # Safety
/// Root the UTF-8 path; the compiler validates the mode and clear pending state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binary_open(path: *const Str, mode: u32) -> *const Object {
    // SAFETY: the caller roots the immutable path throughout opening the stream.
    finish(open(unsafe { &*path }, mode, true, false), std::ptr::null())
}

/// Opens a gzip source without consuming a header until the first read.
///
/// # Safety
/// Root the UTF-8 path. The compiler passes 0 for text or 1 for binary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_gzip_open(path: *const Str, binary: u32) -> *const Object {
    if binary > 1 {
        fail("SystemError", "invalid gzip mode");
    }
    // SAFETY: the caller roots the immutable path throughout the open.
    finish(
        open(unsafe { &*path }, 0, binary == 1, true),
        std::ptr::null(),
    )
}

/// Shares descriptor ownership and buffering while preserving text/binary access errors.
fn open(path: &str, mode: u32, binary: bool, gzip: bool) -> Result<*const Object> {
    let state = open_state(path, mode, binary, gzip)?;
    Ok(Rc::into_raw(Rc::new(Object(StorageCell::new(
        Storage::File(Box::new(state)),
    )))))
}

fn open_state(path: &str, mode: u32, binary: bool, gzip: bool) -> Result<FileState> {
    let (file, metadata) = open_file(path, mode)?;
    // CPython 3.14 bounds the device's preferred block size for buffered I/O.
    let capacity = metadata.blksize().clamp(128 * 1024, 8 * 1024 * 1024) as usize;
    let stream = match mode {
        0 => Stream::Read(Input::new(file, gzip, binary, capacity)),
        _ => Stream::Write(Output::new(file, capacity, binary)),
    };
    Ok(FileState {
        stream: Some(stream),
        kind: match (binary, mode) {
            (false, _) => FileKind::Text,
            (true, 0) if gzip => FileKind::GzipBinaryRead,
            (true, 0) => FileKind::BinaryRead,
            (true, _) => FileKind::BinaryWrite,
        },
    })
}

/// Open a raw descriptor with the same pathname and directory errors as Python open().
pub(super) fn open_file(path: &str, mode: u32) -> Result<(std::fs::File, std::fs::Metadata)> {
    super::super::initialize_io();
    if path.contains('\0') {
        return Err(Error::new(Kind::ValueError, "embedded null byte"));
    }
    let file = match mode {
        0 => OpenOptions::new().read(true).open(path),
        1 => OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path),
        2 => OpenOptions::new().append(true).create(true).open(path),
        3 => OpenOptions::new().write(true).create_new(true).open(path),
        _ => fail("SystemError", "invalid file mode"),
    }
    .map_err(|error| errno_with_filename(error, Some(path)))?;
    let metadata = file.metadata().map_err(errno_error)?;
    if metadata.is_dir() {
        return Err(errno_with_filename(
            io::Error::from_raw_os_error(libc::EISDIR),
            Some(path),
        ));
    }
    Ok((file, metadata))
}

/// A close failure takes precedence over a read or write failure, matching
/// the stream context manager used by pathlib for a whole-file operation.
fn with_path_file<T>(
    path: &str,
    mode: u32,
    binary: bool,
    operation: impl FnOnce(&mut FileState) -> Result<T>,
) -> Result<T> {
    let mut state = open_state(path, mode, binary, false)?;
    let result = operation(&mut state);
    let closed = state.stream.take().unwrap().close().map_err(os_error);
    closed.and(result)
}

pub(super) fn read_path_text(path: &str) -> Result<Str> {
    with_path_file(path, 0, false, FileState::read)
}

pub(super) fn read_path_bytes(path: &str) -> Result<Bytes> {
    with_path_file(path, 0, true, |state| state.input()?.read_bytes(None))
}

pub(super) fn write_path_bytes(path: &str, bytes: &[u8]) -> Result<usize> {
    with_path_file(path, 1, true, |state| state.write_bytes(bytes))?;
    Ok(Int::from_usize(bytes.len()).into_raw())
}

/// Completes buffered output and closes the descriptor before returning the
/// character count, so a final flush error remains a catchable language error.
pub(super) fn write_path_text(path: &str, text: &str) -> Result<usize> {
    with_path_file(path, 1, false, |state| state.write(text))?;
    Ok(Int::from_usize(text.chars().count()).into_raw())
}

/// Reads raw bytes through EOF without decoding or newline conversion.
///
/// # Safety
/// Root an exact BinaryIO object; clear pending error state before the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binary_read(value: *const Object) -> *const Bytes {
    // SAFETY: file I/O has no callbacks and the result owns its bytes before returning.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().input()?.read_bytes(None))
    };
    finish(
        result.map(|bytes| Rc::into_raw(Rc::new(bytes))),
        std::ptr::null(),
    )
}

/// Reads at most a signed, index-sized count from the shared binary cursor.
///
/// # Safety
/// Root an exact BinaryIO and the canonical size integer with clear pending state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binary_read_size(value: *const Object, size: usize) -> *const Bytes {
    // SAFETY: both inputs remain rooted throughout the nonreentrant read.
    let result = unsafe {
        let size = Int::borrow(size);
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().read_size(&size))
    };
    finish(
        result.map(|bytes| Rc::into_raw(Rc::new(bytes))),
        std::ptr::null(),
    )
}

/// Reads through the next LF, retaining that byte. Empty bytes indicate EOF.
///
/// # Safety
/// Root an exact BinaryIO with clear pending state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binary_readline(value: *const Object) -> *const Bytes {
    // SAFETY: reading has no callbacks and the result is independently owned.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().input()?.binary_line())
    };
    finish(
        result.map(|bytes| Rc::into_raw(Rc::new(bytes))),
        std::ptr::null(),
    )
}

/// Writes exact raw bytes and returns their count as a canonical integer.
///
/// # Safety
/// Root an exact BinaryIO object and immutable bytes; pending state must be clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_binary_write(value: *const Object, bytes: *const Bytes) -> usize {
    // SAFETY: both inputs remain rooted and file I/O cannot reenter language code.
    let result = unsafe {
        let bytes = &*bytes;
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().write_bytes(bytes))
            .map(|()| Int::from_usize(bytes.len()).into_raw())
    };
    finish(result, 0)
}

/// Checks an already rooted context manager without acquiring another owner.
///
/// # Safety
/// `value` must be rooted TextIO or BinaryIO and pending state must be clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_enter(value: *const Object) -> u32 {
    // SAFETY: the rooted storage is accessed without callbacks or escaping references.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().stream().map(|_| 1))
    };
    finish(result, 0)
}

/// Reads to EOF with strict UTF-8 decoding and universal newline conversion.
///
/// # Safety
/// `value` must be rooted TextIO and pending state must be clear. Success returns
/// one owned string, including an empty string at EOF; failure returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_read(value: *const Object) -> *const Str {
    // SAFETY: file I/O has no callbacks; the owned result outlives storage access.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().read())
    };
    finish(
        result.map(|text| Rc::into_raw(Rc::new(text))),
        std::ptr::null(),
    )
}

/// Reads through one universal newline, returning an owned string or empty at EOF.
///
/// # Safety
/// Root an exact TextIO with clear pending state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_readline(value: *const Object) -> *const Str {
    // SAFETY: reading has no callbacks and the decoded result is independently owned.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().input()?.text_line())
    };
    finish(
        result.map(|text| Rc::into_raw(Rc::new(text))),
        std::ptr::null(),
    )
}

/// Writes UTF-8 bytes and returns a canonical Int count of Unicode characters.
///
/// # Safety
/// Both handles must be rooted with matching types and pending state clear.
/// Raw zero signals failure; a successful empty write returns tagged zero (1).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_write(value: *const Object, text: *const Str) -> usize {
    // SAFETY: the caller roots this immutable string throughout the write and count.
    let text = unsafe { &*text };
    // SAFETY: text is immutable, both handles are rooted, and no storage escapes.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().write(text))
    };
    finish(
        result.map(|()| Int::from_usize(text.chars().count()).into_raw()),
        0,
    )
}

/// Writes exactly as `imre_text_write` when the compiler discards its return
/// value, avoiding a Unicode character-count scan. Returns 1 on success or 0
/// with a pending error, including the same closed/unwritable-file checks.
///
/// # Safety
/// Both handles must be rooted with matching types and pending state clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_write_discard(value: *const Object, text: *const Str) -> u32 {
    // SAFETY: file I/O cannot reenter; only the write result leaves storage access.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().write(&*text))
    };
    finish(result.map(|()| 1), 0)
}

/// Builds the whole concatenation before checking the stream, then performs the
/// same single write as `imre_text_write_discard`, without a temporary Rc owner.
/// Returns 1 on success or 0 with a pending error; an empty result still checks
/// closed and unwritable stream state.
///
/// # Safety
/// `value` is rooted TextIO and pending is clear. The descriptor array and all
/// fragments satisfy `imre_str_concat_parts`'s lifetime and UTF-8 requirements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_write_parts(
    value: *const Object,
    parts: *const StringPart,
    count: usize,
) -> u32 {
    // SAFETY: all fragments remain rooted and immutable through assembly.
    let text = unsafe { concat_parts(parts, count) };
    // SAFETY: the file remains rooted; no callback or storage reference escapes.
    // Assembly precedes even the stream-state check, preserving evaluation order.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().write(&text))
    };
    finish(result.map(|()| 1), 0)
}

/// # Safety
/// `value` must be rooted TextIO or BinaryIO and pending state clear. Returns 1 on success,
/// or 0 with a pending error. Flushing an open read stream is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_flush(value: *const Object) -> u32 {
    // SAFETY: file I/O has no callbacks or escaped storage references.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().flush())
    };
    finish(result.map(|()| 1), 0)
}

/// Detaches the stream before closing, so all aliases become closed even on error.
///
/// # Safety
/// `value` must be rooted TextIO or BinaryIO and pending state clear. Returns 1 on success
/// (including repeated close), or 0 with a pending error after releasing the fd.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_close(value: *const Object) -> u32 {
    // SAFETY: only an independent stream owner escapes the nonreentrant access.
    let stream = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.file_mut().stream.take())
    };
    let result = stream.map_or(Ok(()), Stream::close).map_err(os_error);
    finish(result.map(|()| 1), 0)
}

/// # Safety
/// `value` must be rooted TextIO or BinaryIO. This infallible query is valid while
/// an error is pending and never changes pending state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_text_closed(value: *const Object) -> u32 {
    // SAFETY: this nonreentrant read returns only a copied boolean.
    unsafe {
        object(value)
            .0
            .with(|storage| u32::from(storage.file().stream.is_none()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::exceptions::{
        imre_error_pending, imre_error_set, imre_error_take, imre_exception_matches,
        imre_exception_new,
    };
    use super::super::{imre_object_release, imre_seq_append, imre_seq_new};

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "imre-runtime-files-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn open(path: &str, mode: u32) -> Rc<Object> {
        let path = Str::new(path);
        // SAFETY: the local string is rooted; this helper requires a successful open.
        let result = unsafe { imre_text_open(&path, mode) };
        assert!(!result.is_null());
        // SAFETY: open transfers this one owned reference to the Rc.
        unsafe { Rc::from_raw(result) }
    }

    fn take_error(kind: Kind) {
        assert_eq!(imre_error_pending(), 1);
        let error = imre_error_take();
        // SAFETY: take transferred the exception, which is inspected and released once.
        unsafe {
            assert_eq!(imre_exception_matches(error, kind as u32), 1);
            imre_object_release(error);
        }
        assert_eq!(imre_error_pending(), 0);
    }

    fn broken_pipe() -> Rc<Object> {
        use std::net::Shutdown;
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;

        let (_peer, writer) = UnixStream::pair().unwrap();
        // A concurrent fork can keep a pipe reader alive until exec. Shutting
        // down the writer guarantees EPIPE even while the child holds the peer.
        writer.shutdown(Shutdown::Write).unwrap();
        let file = File::from(OwnedFd::from(writer));
        Rc::new(Object(StorageCell::new(Storage::File(Box::new(
            FileState {
                stream: Some(Stream::Write(Output::new(file, 8192, false))),
                kind: FileKind::Text,
            },
        )))))
    }

    fn member(bytes: &[u8]) -> Vec<u8> {
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(bytes).unwrap();
        gzip.finish().unwrap()
    }

    #[test]
    fn mixed_text_reads_share_decoder_state_across_buffer_and_member_boundaries() {
        let directory = Directory::new();
        let path = directory.0.join("index");
        let first = "a".repeat(8191) + "\r";
        let second = "\nβ\r🦀\nlast";
        for gzip in [false, true] {
            let bytes = if gzip {
                [
                    member(first.as_bytes()),
                    vec![0; 7],
                    member(second.as_bytes()),
                    vec![0; 9],
                ]
                .concat()
            } else {
                [first.as_bytes(), second.as_bytes()].concat()
            };
            std::fs::write(&path, bytes).unwrap();
            let mut state = open_state(path.to_str().unwrap(), 0, false, gzip)
                .ok()
                .unwrap();
            let input = state.input().ok().unwrap();
            assert_eq!(
                input.text_line().ok().unwrap().as_str(),
                "a".repeat(8191) + "\n"
            );
            assert_eq!(input.text_line().ok().unwrap().as_str(), "β\n");
            assert_eq!(input.read_text().ok().unwrap().as_str(), "🦀\nlast");
            assert_eq!(input.text_line().ok().unwrap().as_str(), "");
        }

        // The read-ahead decoder validates the entire chunk, including bytes
        // after a newline, and retains an incomplete multibyte prefix for retry.
        let mut bytes = b"first\n".to_vec();
        bytes.resize(8191, b'a');
        bytes.extend(b"\xf0\x9f\xff");
        std::fs::write(&path, bytes).unwrap();
        let mut state = open_state(path.to_str().unwrap(), 0, false, false)
            .ok()
            .unwrap();
        let input = state.input().ok().unwrap();
        assert_eq!(input.text_line().ok().unwrap().as_str(), "first\n");
        for _ in 0..2 {
            input.text_line().unwrap_err().raise();
            take_error(Kind::UnicodeDecodeError);
        }
    }

    #[test]
    fn gzip_reads_check_framing_lazily_and_accept_python_header_metadata() {
        let directory = Directory::new();
        let path = directory.0.join("data.gz");
        let mut gzip = member(b"abc\ndef");
        gzip[3] = 0xff;
        let metadata = [
            b"\x03\0abc".as_slice(),
            &vec![b'x'; 70000],
            b"\0comment\0\0\0",
        ]
        .concat();
        gzip.splice(10..10, metadata);
        std::fs::write(&path, &gzip).unwrap();
        let mut state = open_state(path.to_str().unwrap(), 0, true, true)
            .ok()
            .unwrap();
        let input = state.input().ok().unwrap();
        assert_eq!(input.read_bytes(Some(2)).ok().unwrap(), b"ab");
        assert_eq!(input.binary_line().ok().unwrap(), b"c\n");
        let short = input.read_bytes(Some(1024 * 1024)).ok().unwrap();
        assert_eq!(short, b"def");
        assert!(short.capacity() < 1024);
        let empty = input.read_bytes(Some(1024 * 1024)).ok().unwrap();
        assert_eq!(empty.capacity(), 0);

        let end = gzip.len();
        gzip[end - 8] ^= 1;
        std::fs::write(&path, &gzip).unwrap();
        let mut state = open_state(path.to_str().unwrap(), 0, true, true)
            .ok()
            .unwrap();
        let input = state.input().ok().unwrap();
        assert_eq!(input.read_bytes(Some(7)).ok().unwrap(), b"abc\ndef");
        input.read_bytes(None).unwrap_err().raise();
        take_error(Kind::BadGzipFile);

        for (bytes, kind) in [
            (b"x".to_vec(), Kind::BadGzipFile),
            (b"\x1f\x8b".to_vec(), Kind::EOFError),
            (member(b"data")[..12].to_vec(), Kind::EOFError),
            (
                b"\x1f\x8b\x08\0\0\0\0\0\0\xff\x07".to_vec(),
                Kind::ZlibError,
            ),
            // A length/distance pair before the first literal is invalid. A
            // lenient inflater would emit three zeros and accept this trailer.
            (
                b"\x1f\x8b\x08\0\0\0\0\0\0\xff\x03\x02\x00\x12\xd9\x41\xff\x03\0\0\0".to_vec(),
                Kind::ZlibError,
            ),
        ] {
            std::fs::write(&path, bytes).unwrap();
            let mut state = open_state(path.to_str().unwrap(), 0, true, true)
                .ok()
                .unwrap();
            let input = state.input().ok().unwrap();
            assert!(input.read_bytes(Some(0)).ok().unwrap().is_empty());
            input.read_bytes(None).unwrap_err().raise();
            take_error(kind);
        }

        // A stored DEFLATE block followed by an invalid block. The bad byte
        // lies beyond a text chunk but within a binary reader's read-ahead.
        let mut invalid = member(b"")[..10].to_vec();
        invalid.push(0);
        invalid.extend(9000u16.to_le_bytes());
        invalid.extend((!9000u16).to_le_bytes());
        invalid.resize(invalid.len() + 9000, b'a');
        invalid.push(7);
        std::fs::write(&path, invalid).unwrap();
        let mut state = open_state(path.to_str().unwrap(), 0, true, true)
            .ok()
            .unwrap();
        state
            .input()
            .ok()
            .unwrap()
            .read_bytes(Some(1))
            .unwrap_err()
            .raise();
        take_error(Kind::ZlibError);
    }

    #[test]
    fn text_reads_decode_normalize_recover_and_share_closed_state() {
        let directory = Directory::new();
        let path = directory.0.join("input");
        std::fs::write(&path, "a\r\nβ\rc\n\0tail").unwrap();
        let file = open(path.to_str().unwrap(), 0);
        let alias = file.clone();
        // SAFETY: all file and string handles remain rooted; reads transfer ownership.
        unsafe {
            assert_eq!(imre_text_enter(Rc::as_ptr(&file)), 1);
            let text = Rc::from_raw(imre_text_read(Rc::as_ptr(&file)));
            assert_eq!(text.as_str(), "a\nβ\nc\n\0tail");
            assert_eq!(Rc::from_raw(imre_text_read(Rc::as_ptr(&file))).as_str(), "");
            assert_eq!(imre_text_flush(Rc::as_ptr(&file)), 1);
            assert_eq!(imre_text_write(Rc::as_ptr(&file), &Str::new("x")), 0);
            let error = imre_error_take();
            for kind in [Kind::UnsupportedOperation, Kind::OSError, Kind::ValueError] {
                assert_eq!(imre_exception_matches(error, kind as u32), 1);
            }
            imre_object_release(error);
            assert_eq!(imre_text_close(Rc::as_ptr(&file)), 1);
            drop(file);
            assert_eq!(imre_text_closed(Rc::as_ptr(&alias)), 1);
            assert_eq!(imre_text_enter(Rc::as_ptr(&alias)), 0);
            take_error(Kind::ValueError);
            assert!(imre_text_read(Rc::as_ptr(&alias)).is_null());
            take_error(Kind::ValueError);
            assert_eq!(imre_text_close(Rc::as_ptr(&alias)), 1);
        }
        std::fs::write(&path, [b'a', 0xf0, 0x9f]).unwrap();
        let invalid = open(path.to_str().unwrap(), 0);
        // SAFETY: the file remains rooted; the failed read transfers no string.
        unsafe {
            assert!(imre_text_read(Rc::as_ptr(&invalid)).is_null());
            let error = imre_error_take();
            for kind in [
                Kind::UnicodeDecodeError,
                Kind::UnicodeError,
                Kind::ValueError,
            ] {
                assert_eq!(imre_exception_matches(error, kind as u32), 1);
            }
            imre_object_release(error);
            assert_eq!(
                Rc::from_raw(imre_text_read(Rc::as_ptr(&invalid))).as_str(),
                ""
            );
        }
    }

    #[test]
    fn text_writes_count_characters_and_last_container_owner_flushes() {
        let directory = Directory::new();
        let path = directory.0.join("output");
        let file = open(path.to_str().unwrap(), 1);
        let weak = Rc::downgrade(&file);
        let values = imre_seq_new(5);
        // SAFETY: all values are rooted with their declared kinds and form no cycle.
        unsafe {
            assert_eq!(imre_text_write(Rc::as_ptr(&file), &Str::new("")), 1);
            assert_eq!(
                Int(imre_text_write(Rc::as_ptr(&file), &Str::new("é🦀\r\n"))).as_small(),
                Some(4)
            );
            assert!(imre_text_read(Rc::as_ptr(&file)).is_null());
            take_error(Kind::UnsupportedOperation);
            imre_seq_append(values, Rc::as_ptr(&file) as usize, 5);
            drop(file);
            assert!(weak.upgrade().is_some());
            imre_object_release(values);
        }
        assert!(weak.upgrade().is_none());
        assert_eq!(std::fs::read(&path).unwrap(), "é🦀\r\n".as_bytes());
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn discarded_writes_preserve_bytes_and_report_state_and_io_failures() {
        let directory = Directory::new();
        let path = directory.0.join("output");
        let file = open(path.to_str().unwrap(), 1);
        let alias = file.clone();
        let text = Rc::new(Str::new("partsé\0"));
        let ending = "🦀\n";
        let parts = [
            StringPart {
                data: Rc::as_ptr(&text).cast(),
                length: usize::MAX,
            },
            StringPart {
                data: ending.as_ptr(),
                length: ending.len(),
            },
        ];
        // SAFETY: file aliases and temporary immutable strings remain rooted for
        // each operation, and every pending failure is consumed before recovery.
        unsafe {
            assert_eq!(imre_text_write_discard(Rc::as_ptr(&file), &Str::new("")), 1);
            assert_eq!(
                imre_text_write_discard(Rc::as_ptr(&file), &Str::new("é\0🦀\n")),
                1
            );
            assert_eq!(
                imre_text_write_parts(Rc::as_ptr(&file), std::ptr::null(), 0),
                1
            );
            assert_eq!(
                imre_text_write_parts(Rc::as_ptr(&file), parts.as_ptr(), parts.len()),
                1
            );
            assert_eq!(imre_text_close(Rc::as_ptr(&alias)), 1);
            assert_eq!(imre_text_write_discard(Rc::as_ptr(&file), &Str::new("")), 0);
            take_error(Kind::ValueError);
            assert_eq!(
                imre_text_write_parts(Rc::as_ptr(&file), std::ptr::null(), 0),
                0
            );
            take_error(Kind::ValueError);
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            "é\0🦀\npartsé\0🦀\n".as_bytes()
        );
        let readonly = open(path.to_str().unwrap(), 0);
        let full = broken_pipe();
        let unbuffered = Str::from("x".repeat(8193));
        let large_parts = [
            StringPart {
                data: unbuffered.as_ptr(),
                length: 4096,
            },
            StringPart {
                data: unbuffered[4096..].as_ptr(),
                length: 4097,
            },
        ];
        // SAFETY: each failed write leaves its rooted stream available for the
        // following recovery; a large write reaches the broken pipe immediately.
        unsafe {
            assert_eq!(
                imre_text_write_discard(Rc::as_ptr(&readonly), &Str::new("")),
                0
            );
            take_error(Kind::UnsupportedOperation);
            assert_eq!(
                imre_text_write_parts(Rc::as_ptr(&readonly), std::ptr::null(), 0),
                0
            );
            take_error(Kind::UnsupportedOperation);
            assert_eq!(imre_text_write_discard(Rc::as_ptr(&full), &unbuffered), 0);
            take_error(Kind::OSError);
            assert_eq!(
                imre_text_write_parts(Rc::as_ptr(&full), large_parts.as_ptr(), large_parts.len()),
                0
            );
            take_error(Kind::OSError);
            assert_eq!(imre_text_write_discard(Rc::as_ptr(&full), &Str::new("")), 1);
            assert_eq!(
                imre_text_write_parts(Rc::as_ptr(&full), std::ptr::null(), 0),
                1
            );
            // One assembled write bypasses the buffer on this failure. Fragmented
            // writes would leave the first 4096 bytes buffered and make close fail.
            assert_eq!(imre_text_close(Rc::as_ptr(&full)), 1);
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn failed_close_closes_aliases_and_final_drop_preserves_pending() {
        let file = broken_pipe();
        // SAFETY: the file and strings stay rooted; each failure is consumed before reuse.
        unsafe {
            assert_ne!(imre_text_write(Rc::as_ptr(&file), &Str::new("buffered")), 0);
            assert_eq!(imre_text_flush(Rc::as_ptr(&file)), 0);
            take_error(Kind::OSError);
            assert_eq!(imre_text_close(Rc::as_ptr(&file)), 0);
            assert_eq!(imre_text_closed(Rc::as_ptr(&file)), 1);
            take_error(Kind::OSError);
            assert_eq!(imre_text_close(Rc::as_ptr(&file)), 1);
            assert_eq!(imre_text_write(Rc::as_ptr(&file), &Str::new("")), 0);
            take_error(Kind::ValueError);
        }

        let dropped = broken_pipe();
        // SAFETY: the exception and file handles are rooted; pending owns an extra
        // exception reference, and both references are released exactly once.
        unsafe {
            assert_ne!(
                imre_text_write(Rc::as_ptr(&dropped), &Str::new("buffered")),
                0
            );
            let error = imre_exception_new(Kind::RuntimeError as u32, std::ptr::null());
            imre_error_set(error);
            drop(dropped);
            assert_eq!(imre_error_pending(), 1);
            assert_eq!(imre_error_take(), error);
            imre_object_release(error);
            imre_object_release(error);
        }
        assert_eq!(imre_error_pending(), 0);
        let ignored_failure = broken_pipe();
        // SAFETY: the file and string remain rooted through the buffered write.
        unsafe {
            assert_ne!(
                imre_text_write(Rc::as_ptr(&ignored_failure), &Str::new("buffered")),
                0
            );
        }
        drop(ignored_failure);
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn open_reports_paths_before_publishing_a_resource() {
        let directory = Directory::new();
        let regular = directory.0.join("regular");
        std::fs::write(&regular, "").unwrap();
        for (path, kind) in [
            (
                directory.0.join("missing").to_string_lossy().into_owned(),
                Kind::FileNotFoundError,
            ),
            (
                directory.0.to_string_lossy().into_owned(),
                Kind::IsADirectoryError,
            ),
            (
                regular.join("child").to_string_lossy().into_owned(),
                Kind::NotADirectoryError,
            ),
            ("nul\0path".to_owned(), Kind::ValueError),
        ] {
            // SAFETY: the temporary path remains rooted for this failed call.
            assert!(unsafe { imre_text_open(&Str::from(path), 0) }.is_null());
            take_error(kind);
        }
        let denied = os_error(io::Error::from(io::ErrorKind::PermissionDenied));
        denied.raise();
        take_error(Kind::PermissionError);
    }
}
