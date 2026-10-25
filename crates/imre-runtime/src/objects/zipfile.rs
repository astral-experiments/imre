//! Read-only ZIP archives. Closing releases the file; directory names remain available.

use std::fs::File;
use std::rc::Rc;

use super::exceptions::{Error, Result, finish};
use super::{Object, Storage, StorageCell, Str, Words, files, object};

mod directory;

pub(super) struct Archive {
    file: Option<File>,
    names: Vec<Rc<Str>>,
}

impl Archive {
    fn open(path: &str) -> Result<Self> {
        let (mut file, _) = files::open_file(path, 0)?;
        let names = directory::names(&mut file)?;
        Ok(Self {
            file: Some(file),
            names,
        })
    }

    fn namelist(&self) -> Result<Rc<Object>> {
        let mut names = Words::new(3);
        names
            .values
            .try_reserve_exact(self.names.len())
            .map_err(|_| Error::allocation())?;
        names.values.extend(
            self.names
                .iter()
                .map(|name| Rc::into_raw(Rc::clone(name)) as usize),
        );
        Ok(names.into_object())
    }
}

/// Open the archive and index its central directory without inflating any member.
///
/// # Safety
/// Root the exact path string and clear pending errors. The result transfers one object owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_open(path: *const Str) -> *const Object {
    // SAFETY: the path string remains live for the opening operation.
    let result = Archive::open(unsafe { &*path }).map(|archive| {
        Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::ZipFile(
            Box::new(archive),
        )))))
    });
    finish(result, std::ptr::null())
}

/// Return a fresh list in central-directory order, including repeated filenames.
///
/// # Safety
/// Root an exact archive object; the returned list owns its string references.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_namelist(archive: *const Object) -> *const Object {
    // SAFETY: no Python code can run while directory references are acquired.
    let result = unsafe {
        object(archive).0.with(|storage| {
            let Storage::ZipFile(archive) = storage else {
                unreachable!()
            };
            archive.namelist().map(Rc::into_raw)
        })
    };
    finish(result, std::ptr::null())
}

/// Close only the underlying file; repeated close and later namelist are valid.
///
/// # Safety
/// Root an exact archive object for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_close(archive: *const Object) {
    // SAFETY: taking the descriptor cannot reenter user code.
    unsafe {
        object(archive).0.with_mut(|storage| {
            let Storage::ZipFile(archive) = storage else {
                unreachable!()
            };
            archive.file.take();
        })
    };
}
