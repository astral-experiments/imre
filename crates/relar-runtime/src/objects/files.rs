//! Sealed UTF-8 text streams. Explicit operations report language errors;
//! final ownership cleanup only closes resources and never changes error state.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::os::fd::IntoRawFd;
use std::rc::Rc;

use super::exceptions::{Error, Kind, Result, finish};
use super::{Int, Object, Storage, StorageCell, Str, fail, object};

pub(super) struct TextFile {
    stream: Option<Stream>,
}

enum Stream {
    Read(File),
    Write(BufWriter<File>),
}

impl Stream {
    /// Closes exactly once even when flushing fails. `into_parts` prevents a
    /// second implicit flush; a close error takes precedence over a flush error.
    fn close(self) -> io::Result<()> {
        let (file, flushed) = match self {
            Self::Read(file) => (file, Ok(())),
            Self::Write(mut writer) => {
                let flushed = writer.flush();
                let (file, buffered) = writer.into_parts();
                drop(buffered);
                (file, flushed)
            }
        };
        let descriptor = file.into_raw_fd();
        // SAFETY: into_raw_fd transfers our unique descriptor owner. Do not
        // retry close after EINTR: the descriptor may already have been reused.
        let closed = if unsafe { libc::close(descriptor) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
        closed.and(flushed)
    }
}

impl Drop for TextFile {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            // Final release must remain infallible even during error cleanup.
            let _ = stream.close();
        }
    }
}

impl TextFile {
    fn stream(&mut self) -> Result<&mut Stream> {
        self.stream
            .as_mut()
            .ok_or_else(|| Error::new(Kind::ValueError, "I/O operation on closed file."))
    }

    fn read(&mut self) -> Result<Str> {
        let Stream::Read(file) = self.stream()? else {
            return Err(Error::new(Kind::UnsupportedOperation, "not readable"));
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(os_error)?;
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

    fn write(&mut self, text: &str) -> Result<usize> {
        let Stream::Write(writer) = self.stream()? else {
            return Err(Error::new(Kind::UnsupportedOperation, "not writable"));
        };
        writer.write_all(text.as_bytes()).map_err(os_error)?;
        Ok(Int::from_usize(text.chars().count()).into_raw())
    }

    fn flush(&mut self) -> Result<()> {
        match self.stream()? {
            Stream::Read(_) => Ok(()),
            Stream::Write(writer) => writer.flush().map_err(os_error),
        }
    }
}

impl Storage {
    fn text_file(&self) -> &TextFile {
        match self {
            Self::TextIO(file) => file,
            _ => fail("SystemError", "expected a text file object"),
        }
    }

    fn text_file_mut(&mut self) -> &mut TextFile {
        match self {
            Self::TextIO(file) => file,
            _ => fail("SystemError", "expected a text file object"),
        }
    }
}

fn os_error(error: io::Error) -> Error {
    let kind = match error.kind() {
        io::ErrorKind::NotFound => Kind::FileNotFoundError,
        io::ErrorKind::PermissionDenied => Kind::PermissionError,
        io::ErrorKind::IsADirectory => Kind::IsADirectoryError,
        io::ErrorKind::NotADirectory => Kind::NotADirectoryError,
        _ => Kind::OSError,
    };
    Error::new(kind, error.to_string())
}

/// Opens a UTF-8 text stream: mode 0 reads, mode 1 creates/truncates for writing.
/// Returns an owned Object or null with a pending error.
///
/// # Safety
/// `path` must be a rooted string. `mode` must be compiler-validated as 0 or 1.
/// Like other fallible entries, pending state must be clear on entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_text_open(path: *const Str, mode: u32) -> *const Object {
    super::super::initialize_io();
    // SAFETY: the caller roots this immutable string for the entire open.
    let path = unsafe { &*path };
    if path.contains('\0') {
        return Error::new(Kind::ValueError, "embedded null byte").returning(std::ptr::null());
    }
    let result = (|| {
        let file = match mode {
            0 => OpenOptions::new().read(true).open(path.as_str()),
            1 => OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path.as_str()),
            _ => fail("SystemError", "invalid text file mode"),
        }
        .map_err(os_error)?;
        if file.metadata().map_err(os_error)?.is_dir() {
            return Err(Error::new(Kind::IsADirectoryError, "is a directory"));
        }
        let stream = match mode {
            0 => Stream::Read(file),
            _ => Stream::Write(BufWriter::with_capacity(8192, file)),
        };
        Ok(Rc::into_raw(Rc::new(Object(StorageCell::new(
            Storage::TextIO(Box::new(TextFile {
                stream: Some(stream),
            })),
        )))))
    })();
    finish(result, std::ptr::null())
}

/// Checks an already rooted context manager without acquiring another owner.
///
/// # Safety
/// `value` must be a rooted TextIO object and pending state must be clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_text_enter(value: *const Object) -> u8 {
    // SAFETY: the rooted storage is accessed without callbacks or escaping references.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.text_file_mut().stream().map(|_| 1))
    };
    finish(result, 0)
}

/// Reads to EOF with strict UTF-8 decoding and universal newline conversion.
///
/// # Safety
/// `value` must be rooted TextIO and pending state must be clear. Success returns
/// one owned string, including an empty string at EOF; failure returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_text_read(value: *const Object) -> *const Str {
    // SAFETY: file I/O has no callbacks; the owned result outlives storage access.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.text_file_mut().read())
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
pub unsafe extern "C" fn relar_text_write(value: *const Object, text: *const Str) -> usize {
    // SAFETY: text is immutable, both handles are rooted, and no storage escapes.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.text_file_mut().write(&*text))
    };
    finish(result, 0)
}

/// # Safety
/// `value` must be rooted TextIO and pending state clear. Returns 1 on success,
/// or 0 with a pending error. Flushing an open read stream is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_text_flush(value: *const Object) -> u8 {
    // SAFETY: file I/O has no callbacks or escaped storage references.
    let result = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.text_file_mut().flush())
    };
    finish(result.map(|()| 1), 0)
}

/// Detaches the stream before closing, so all aliases become closed even on error.
///
/// # Safety
/// `value` must be rooted TextIO and pending state clear. Returns 1 on success
/// (including repeated close), or 0 with a pending error after releasing the fd.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_text_close(value: *const Object) -> u8 {
    // SAFETY: only an independent stream owner escapes the nonreentrant access.
    let stream = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.text_file_mut().stream.take())
    };
    let result = stream.map_or(Ok(()), Stream::close).map_err(os_error);
    finish(result.map(|()| 1), 0)
}

/// # Safety
/// `value` must be a rooted TextIO object. This infallible query is valid while
/// an error is pending and never changes pending state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_text_closed(value: *const Object) -> u8 {
    // SAFETY: this nonreentrant read returns only a copied boolean.
    unsafe {
        object(value)
            .0
            .with(|storage| u8::from(storage.text_file().stream.is_none()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::exceptions::{
        relar_error_pending, relar_error_set, relar_error_take, relar_exception_matches,
        relar_exception_new,
    };
    use super::super::{relar_object_release, relar_seq_append, relar_seq_new};

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "relar-runtime-files-{}-{}",
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
        let result = unsafe { relar_text_open(&path, mode) };
        assert!(!result.is_null());
        // SAFETY: open transfers this one owned reference to the Rc.
        unsafe { Rc::from_raw(result) }
    }

    fn take_error(kind: Kind) {
        assert_eq!(relar_error_pending(), 1);
        let error = relar_error_take();
        // SAFETY: take transferred the exception, which is inspected and released once.
        unsafe {
            assert_eq!(relar_exception_matches(error, kind as u32), 1);
            relar_object_release(error);
        }
        assert_eq!(relar_error_pending(), 0);
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
            assert_eq!(relar_text_enter(Rc::as_ptr(&file)), 1);
            let text = Rc::from_raw(relar_text_read(Rc::as_ptr(&file)));
            assert_eq!(text.as_str(), "a\nβ\nc\n\0tail");
            assert_eq!(
                Rc::from_raw(relar_text_read(Rc::as_ptr(&file))).as_str(),
                ""
            );
            assert_eq!(relar_text_flush(Rc::as_ptr(&file)), 1);
            assert_eq!(relar_text_write(Rc::as_ptr(&file), &Str::new("x")), 0);
            let error = relar_error_take();
            for kind in [Kind::UnsupportedOperation, Kind::OSError, Kind::ValueError] {
                assert_eq!(relar_exception_matches(error, kind as u32), 1);
            }
            relar_object_release(error);
            assert_eq!(relar_text_close(Rc::as_ptr(&file)), 1);
            drop(file);
            assert_eq!(relar_text_closed(Rc::as_ptr(&alias)), 1);
            assert_eq!(relar_text_enter(Rc::as_ptr(&alias)), 0);
            take_error(Kind::ValueError);
            assert!(relar_text_read(Rc::as_ptr(&alias)).is_null());
            take_error(Kind::ValueError);
            assert_eq!(relar_text_close(Rc::as_ptr(&alias)), 1);
        }
        std::fs::write(&path, [b'a', 0xf0, 0x9f]).unwrap();
        let invalid = open(path.to_str().unwrap(), 0);
        // SAFETY: the file remains rooted; the failed read transfers no string.
        unsafe {
            assert!(relar_text_read(Rc::as_ptr(&invalid)).is_null());
            let error = relar_error_take();
            for kind in [
                Kind::UnicodeDecodeError,
                Kind::UnicodeError,
                Kind::ValueError,
            ] {
                assert_eq!(relar_exception_matches(error, kind as u32), 1);
            }
            relar_object_release(error);
            assert_eq!(
                Rc::from_raw(relar_text_read(Rc::as_ptr(&invalid))).as_str(),
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
        let values = relar_seq_new(5);
        // SAFETY: all values are rooted with their declared kinds and form no cycle.
        unsafe {
            assert_eq!(relar_text_write(Rc::as_ptr(&file), &Str::new("")), 1);
            assert_eq!(
                Int(relar_text_write(Rc::as_ptr(&file), &Str::new("é🦀\r\n"))).as_small(),
                Some(4)
            );
            assert!(relar_text_read(Rc::as_ptr(&file)).is_null());
            take_error(Kind::UnsupportedOperation);
            relar_seq_append(values, Rc::as_ptr(&file) as usize, 5);
            drop(file);
            assert!(weak.upgrade().is_some());
            relar_object_release(values);
        }
        assert!(weak.upgrade().is_none());
        assert_eq!(std::fs::read(&path).unwrap(), "é🦀\r\n".as_bytes());
        assert_eq!(relar_error_pending(), 0);
    }

    #[test]
    fn failed_close_closes_aliases_and_final_drop_preserves_pending() {
        let file = open("/dev/full", 1);
        // SAFETY: the file and strings stay rooted; each failure is consumed before reuse.
        unsafe {
            assert_ne!(
                relar_text_write(Rc::as_ptr(&file), &Str::new("buffered")),
                0
            );
            assert_eq!(relar_text_flush(Rc::as_ptr(&file)), 0);
            take_error(Kind::OSError);
            assert_eq!(relar_text_close(Rc::as_ptr(&file)), 0);
            assert_eq!(relar_text_closed(Rc::as_ptr(&file)), 1);
            take_error(Kind::OSError);
            assert_eq!(relar_text_close(Rc::as_ptr(&file)), 1);
            assert_eq!(relar_text_write(Rc::as_ptr(&file), &Str::new("")), 0);
            take_error(Kind::ValueError);
        }

        let dropped = open("/dev/full", 1);
        // SAFETY: the exception and file handles are rooted; pending owns an extra
        // exception reference, and both references are released exactly once.
        unsafe {
            assert_ne!(
                relar_text_write(Rc::as_ptr(&dropped), &Str::new("buffered")),
                0
            );
            let error = relar_exception_new(Kind::RuntimeError as u32, std::ptr::null());
            relar_error_set(error);
            drop(dropped);
            assert_eq!(relar_error_pending(), 1);
            assert_eq!(relar_error_take(), error);
            relar_object_release(error);
            relar_object_release(error);
        }
        assert_eq!(relar_error_pending(), 0);
        let ignored_failure = open("/dev/full", 1);
        // SAFETY: the file and string remain rooted through the buffered write.
        unsafe {
            assert_ne!(
                relar_text_write(Rc::as_ptr(&ignored_failure), &Str::new("buffered")),
                0
            );
        }
        drop(ignored_failure);
        assert_eq!(relar_error_pending(), 0);
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
            assert!(unsafe { relar_text_open(&Str::from(path), 0) }.is_null());
            take_error(kind);
        }
        let denied = os_error(io::Error::from(io::ErrorKind::PermissionDenied));
        denied.raise();
        take_error(Kind::PermissionError);
    }
}
