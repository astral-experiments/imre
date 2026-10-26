//! Owning access to the same buffered text file used by typed native code.

use super::{exceptions::Result, files, text};

#[derive(Clone)]
pub(crate) struct TextFile(crate::ObjectOwner);

impl TextFile {
    pub(crate) fn standard(stderr: bool) -> Self {
        Self(files::standard_stream(stderr))
    }

    pub(crate) fn create(path: &str) -> Result<Self> {
        let path = text::try_owned_string(path)?;
        let file = files::open(&path, 1, false, false)?;
        // SAFETY: open transfers one strong owner of an initialized file object.
        Ok(Self(unsafe { crate::ObjectOwner::from_raw(file) }))
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
