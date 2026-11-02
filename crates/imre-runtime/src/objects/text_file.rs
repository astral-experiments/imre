//! Owning access to the same buffered text file used by typed native code.

use super::{exceptions::Result, files};

/// A boxed snapshot uses UTF-8 unless its code points include a surrogate.
pub(crate) enum MemoryText {
    Utf8(crate::Str),
    Surrogates(Vec<u32>),
}

#[derive(Clone)]
pub(crate) struct TextFile(crate::ObjectOwner);

impl TextFile {
    pub(crate) fn standard(stderr: bool) -> Self {
        Self(files::standard_stream(stderr))
    }

    pub(crate) fn string_io(initial: Option<&str>, newline: Option<&str>) -> Result<Self> {
        files::FileState::string_io(initial, newline).map(Self)
    }

    pub(crate) fn memory_value(&self) -> Result<MemoryText> {
        // SAFETY: the snapshot owns its text and the memory stream cannot reenter Python.
        unsafe {
            self.0
                .0
                .with_mut(|storage| storage.file_mut().string_io_points())
        }
    }

    pub(crate) fn read_memory(&self, size: Option<usize>, line: bool) -> Result<MemoryText> {
        // SAFETY: the stream remains owned and reading characters runs no Python callbacks.
        unsafe {
            self.0
                .0
                .with_mut(|storage| storage.file_mut().read_string_io_points(size, line))
        }
    }

    pub(crate) fn write_memory(&self, points: &[u32]) -> Result<()> {
        // SAFETY: writing the given code points does not call Python code.
        unsafe {
            self.0
                .0
                .with_mut(|s| s.file_mut().write_string_io_points(points))
        }
    }

    pub(crate) fn seek_memory(&self, pos: i64, whence: i32) -> Result<usize> {
        // SAFETY: the offset and whence have already been converted outside this borrow.
        unsafe {
            self.0
                .0
                .with_mut(|s| s.file_mut().seek_string_io(pos, whence))
        }
    }

    pub(crate) fn truncate_memory(&self, size: Option<isize>) -> Result<usize> {
        // SAFETY: the stream remains owned and changing its length cannot reenter Python.
        unsafe { self.0.0.with_mut(|s| s.file_mut().truncate_string_io(size)) }
    }

    pub(crate) fn memory_newlines(&self) -> Result<u8> {
        // SAFETY: this access only copies the memory stream's newline mask.
        unsafe { self.0.0.with_mut(|s| s.file_mut().string_io_newlines()) }
    }

    pub(crate) fn memory_position(&self) -> Result<usize> {
        // SAFETY: only a copy of the native character offset escapes this borrow.
        unsafe {
            self.0
                .0
                .with_mut(|storage| storage.file_mut().string_io_position())
        }
    }

    pub(crate) fn closed(&self) -> bool {
        // SAFETY: this nonreentrant access only copies the file's closed flag.
        unsafe { self.0.0.with(|storage| storage.file().closed()) }
    }

    /// Consumes one decoded line or the remaining text from the shared cursor.
    pub(crate) fn read(&self, line: bool) -> Result<crate::Str> {
        // SAFETY: reading the owned file runs no Python callbacks.
        unsafe {
            self.0.0.with_mut(|storage| {
                let file = storage.file_mut();
                if line { file.readline() } else { file.read() }
            })
        }
    }

    pub(crate) fn fileno(&self) -> Result<std::os::fd::RawFd> {
        // SAFETY: reading the owned file's descriptor runs no Python callbacks.
        unsafe { self.0.0.with(|storage| storage.file().fileno()) }
    }

    pub(crate) fn isatty(&self) -> Result<bool> {
        Ok(super::os::isatty(self.fileno()?))
    }

    pub(crate) fn write(&self, value: &str) -> Result<()> {
        // SAFETY: the text writer uses no Python callbacks during this borrow.
        unsafe { self.0.0.with_mut(|storage| storage.file_mut().write(value)) }
    }

    pub(crate) fn encoding(&self) -> Option<crate::StrOwner> {
        // SAFETY: the retained encoding is cloned without running Python code.
        unsafe { self.0.0.with(|storage| storage.file().encoding.clone()) }
    }

    /// Identify the still-open original stream, even when Python retained an alias.
    pub(crate) fn standard_writer(&self) -> Result<bool> {
        // SAFETY: checking the file state does not reenter Python.
        unsafe {
            self.0
                .0
                .with_mut(|storage| storage.file_mut().standard_writer())
        }
    }

    /// Closing the binary layer flushes only its bytes and closes the enclosing text view.
    pub(crate) fn close_standard_buffer(&self) -> Result<()> {
        // SAFETY: this owned file state is a standard output stream; flushing runs no Python.
        unsafe { self.0.0.with_mut(|s| s.file_mut().close_standard_buffer()) }
    }

    pub(crate) fn flush(&self) -> Result<()> {
        // SAFETY: flushing the owned stream does not reenter native Python code.
        unsafe { self.0.0.with_mut(|storage| storage.file_mut().flush()) }
    }

    pub(crate) fn close(&self) -> Result<()> {
        files::close_stream(&self.0)
    }
}
