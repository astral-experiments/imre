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

    pub(crate) fn replace_memory_value(&self, text: &str) -> Result<()> {
        // SAFETY: decoding borrowed UTF-8 does not call Python code.
        unsafe {
            self.0.0.with_mut(|storage| {
                storage
                    .file_mut()
                    .replace_string_io_points(text.chars().map(u32::from))
            })
        }
    }

    pub(crate) fn replace_memory_points(&self, points: &[u32]) -> Result<()> {
        // SAFETY: copying borrowed code points does not call Python code.
        unsafe {
            self.0.0.with_mut(|storage| {
                storage
                    .file_mut()
                    .replace_string_io_points(points.iter().copied())
            })
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

    pub(crate) fn memory_newline(&self) -> Result<Option<&'static str>> {
        // SAFETY: only a static spelling of the configured newline escapes the borrow.
        unsafe { self.0.0.with_mut(|s| s.file_mut().string_io_newline()) }
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

    pub(crate) fn write(&self, value: &str) -> Result<()> {
        // SAFETY: the text writer uses no Python callbacks during this borrow.
        unsafe { self.0.0.with_mut(|storage| storage.file_mut().write(value)) }
    }

    pub(crate) fn flush(&self) -> Result<()> {
        // SAFETY: flushing the owned stream does not reenter native Python code.
        unsafe { self.0.0.with_mut(|storage| storage.file_mut().flush()) }
    }

    pub(crate) fn close(&self) -> Result<()> {
        files::close_stream(&self.0)
    }
}
