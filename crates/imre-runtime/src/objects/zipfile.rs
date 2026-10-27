//! Read-only ZIP archives, their ordered central-directory metadata, and
//! independent member handles. Closing drops the archive's source owner only.

use crate::ObjectOwner;
use crate::StrOwner;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt::Write;

use super::exceptions::{Error, Kind, Result, finish, string_repr};
use super::{Bytes, Int, Object, Storage, StorageCell, Str, Words, files, object};

mod directory;
mod member;
mod source;
pub(in crate::objects) use member::Member;
use source::Source;

pub(super) struct Info {
    filename: StrOwner,
    original: StrOwner,
    method: u16,
    flags: u16,
    crc: u32,
    file_size: u64,
    compress_size: u64,
    external_attr: u32,
    local_offset: u64,
    offset: i128,
    end: i128,
}

impl Info {
    /// ZipFile includes the complete ZipInfo repr when an encrypted entry is
    /// opened with metadata as its key.
    fn repr(&self) -> Str {
        let mut out = format!("<ZipInfo filename={}", string_repr(&self.filename));
        if self.method != 0 {
            let method = match self.method {
                1 => "shrink",
                2..=5 => "reduce",
                6 | 10 => "implode",
                7 => "tokenize",
                8 => "deflate",
                9 => "deflate64",
                12 => "bzip2",
                14 => "lzma",
                18 => "terse",
                19 => "lz77",
                93 => "zstd",
                97 => "wavpack",
                98 => "ppmd",
                _ => "",
            };
            if method.is_empty() {
                write!(out, " compress_type={}", self.method).unwrap();
            } else {
                write!(out, " compress_type={method}").unwrap();
            }
        }
        let mode = self.external_attr >> 16;
        if mode != 0 {
            let ty = match mode & 0o170000 {
                0o040000 => 'd',
                0o020000 => 'c',
                0o100000 => '-',
                0o060000 => 'b',
                0o010000 => 'p',
                0o120000 => 'l',
                0o140000 => 's',
                _ => '?',
            };
            write!(out, " filemode='{ty}").unwrap();
            for shift in [6, 3, 0] {
                out.push(if mode & (4 << shift) != 0 { 'r' } else { '-' });
                out.push(if mode & (2 << shift) != 0 { 'w' } else { '-' });
                let special = mode & (1 << (shift / 3 + 9)) != 0;
                let executable = mode & (1 << shift) != 0;
                out.push(match (special, executable, shift == 0) {
                    (false, true, _) => 'x',
                    (false, false, _) => '-',
                    (true, true, true) => 't',
                    (true, false, true) => 'T',
                    (true, true, false) => 's',
                    (true, false, false) => 'S',
                });
            }
            out.push('\'');
        }
        let low = self.external_attr & 0xffff;
        if low != 0 {
            write!(out, " external_attr={low:#x}").unwrap();
        }
        let dir = self.filename.ends_with('/');
        if !dir || self.file_size != 0 {
            write!(out, " file_size={}", self.file_size).unwrap();
        }
        if (!dir || self.compress_size != 0)
            && (self.method != 0 || self.file_size != self.compress_size)
        {
            write!(out, " compress_size={}", self.compress_size).unwrap();
        }
        out.push('>');
        out.into()
    }
}

impl Object {
    fn zip_info(&self) -> crate::Shared<Info> {
        // SAFETY: all callers supply an exact ZipInfo with a live owner.
        unsafe {
            self.0.with(|storage| match storage {
                Storage::ZipInfo(info) => crate::Shared::clone(info),
                _ => unreachable!(),
            })
        }
    }
}

pub(crate) struct Archive {
    source: Option<Source>,
    filename: Option<StrOwner>,
    filelist: ObjectOwner,
    by_name: HashMap<StrOwner, ObjectOwner>,
}

fn store(storage: Storage) -> Result<ObjectOwner> {
    ObjectOwner::try_new(Object(StorageCell::new(storage))).map_err(|_| Error::allocation())
}

impl Archive {
    /// Retain the central directory after closing the descriptor used to index it.
    pub(crate) fn index_import(path: &str) -> Result<Self> {
        let mut archive = Self::open(Source::open(path)?, Some(StrOwner::new(path.into())))?;
        archive.source = None;
        Ok(archive)
    }

    /// Reopen the archive to read one exact resource from this cached directory.
    pub(crate) fn import_data(&self, name: &Str) -> Result<Bytes> {
        if let Some(entry) = self.by_name.get(name) {
            let path = self.filename.as_ref().ok_or_else(|| {
                Error::new(Kind::ValueError, "zipimport requires a path-backed archive")
            })?;
            return Member::open(Source::open(path)?, entry.zip_info(), || string_repr(name))?
                .read_import_data();
        }
        if name.ends_with('/')
            && self
                .by_name
                .keys()
                .any(|member| member.starts_with(name.as_str()))
        {
            return Ok(Bytes::new());
        }
        Err(Error::from_errno(
            Kind::OSError,
            0,
            "",
            Some(super::exceptions::Filename::Str(StrOwner::new(
                name.clone(),
            ))),
            None,
        ))
    }

    fn open(source: Source, filename: Option<StrOwner>) -> Result<Self> {
        let entries = source.directory()?;
        let mut list = Words::new(5);
        let mut by_name = HashMap::new();
        list.values
            .try_reserve_exact(entries.len())
            .map_err(|_| Error::allocation())?;
        by_name
            .try_reserve(entries.len())
            .map_err(|_| Error::allocation())?;
        for entry in entries {
            let name = entry.filename.clone();
            let info = crate::Shared::try_new(entry).map_err(|_| Error::allocation())?;
            let object = store(Storage::ZipInfo(info))?;
            by_name.insert(name, object.clone());
            list.values.push(ObjectOwner::into_raw(object) as usize);
        }
        Ok(Self {
            source: Some(source),
            filename,
            filelist: store(list.into_storage())?,
            by_name,
        })
    }

    fn get(&self, name: &Str) -> Result<&ObjectOwner> {
        self.by_name.get(name).ok_or_else(|| {
            Error::string_key(
                format!(
                    "There is no item named {} in the archive",
                    string_repr(name)
                )
                .into(),
            )
        })
    }

    fn namelist(&self) -> Result<ObjectOwner> {
        let mut names = Words::new(3);
        self.for_infos(|list| {
            names
                .values
                .try_reserve_exact(list.len())
                .map_err(|_| Error::allocation())?;
            for &info in list {
                // SAFETY: the rooted filelist owns initialized ZipInfo handles.
                let info = unsafe { object(info as *const Object) }.zip_info();
                names
                    .values
                    .push(StrOwner::into_raw(info.filename.clone()) as usize);
            }
            Ok(())
        })?;
        store(names.into_storage())
    }

    fn for_infos<T>(&self, f: impl FnOnce(&[usize]) -> T) -> T {
        // SAFETY: the sealed operations here never invoke user code.
        unsafe { self.filelist.0.with(|s| f(&s.sequence_storage().values)) }
    }

    fn source(&self) -> Result<Source> {
        self.source.clone().ok_or_else(|| {
            Error::new(
                Kind::ValueError,
                "Attempt to use ZIP archive that was already closed",
            )
        })
    }

    fn member_by_name(&self, name: &Str) -> Result<Member> {
        let source = self.source()?;
        Member::open(source, self.get(name)?.zip_info(), || string_repr(name))
    }

    unsafe fn member(&self, key: *const c_void, is_info: u32) -> Result<Member> {
        if is_info == 0 {
            // SAFETY: the compiler selected the exact rooted string representation.
            self.member_by_name(unsafe { &*key.cast::<Str>() })
        } else {
            let source = self.source()?;
            // SAFETY: the discriminator selects an exact rooted ZipInfo object.
            let info = unsafe { object(key.cast()) }.zip_info();
            Member::open(source, info.clone(), || info.repr())
        }
    }

    fn test(&self) -> Result<Option<StrOwner>> {
        self.for_infos(|list| {
            for &word in list {
                // SAFETY: the filelist owns every entry throughout iteration.
                let info = unsafe { object(word as *const Object) }.zip_info();
                let checked = self.member_by_name(&info.filename).and_then(|mut file| {
                    while !file.read(Some(1 << 20))?.is_empty() {}
                    Ok(())
                });
                match checked {
                    Err(error) if error.is_kind(Kind::BadZipFile) => {
                        return Ok(Some(info.filename.clone()));
                    }
                    Err(error) => return Err(error),
                    Ok(()) => {}
                }
            }
            Ok(None)
        })
    }
}

fn opened(result: Result<Archive>) -> *const Object {
    finish(
        result.and_then(|archive| {
            let archive =
                allocator_api2::boxed::Box::try_new(archive).map_err(|_| Error::allocation())?;
            store(Storage::ZipFile(archive)).map(ObjectOwner::into_raw)
        }),
        std::ptr::null(),
    )
}

unsafe fn with_archive<T>(archive: *const Object, f: impl FnOnce(&Archive) -> T) -> T {
    // SAFETY: all callers preserve the exact archive for the sealed operation.
    unsafe {
        object(archive).0.with(|s| match s {
            Storage::ZipFile(archive) => f(archive),
            _ => unreachable!(),
        })
    }
}

/// Open a POSIX path and index the central directory without inflating any member.
///
/// # Safety
/// The exact path string is rooted and the result transfers one object owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_open(path: *const Str) -> *const Object {
    // SAFETY: the path is a live string owner for the entire open operation.
    let path: StrOwner = unsafe { StrOwner::acquire_word(path as usize) };
    opened(Source::open(&path).and_then(|source| Archive::open(source, Some(path))))
}

/// Open a caller-supplied binary stream; closing the archive never closes it.
///
/// # Safety
/// The exact input stream remains rooted throughout the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_open_file(file: *const Object) -> *const Object {
    // SAFETY: a caller-supplied BinaryIO transfers a new owner to the archive.
    let file: ObjectOwner = unsafe { ObjectOwner::acquire_word(file as usize) };
    // SAFETY: our new file owner keeps the sealed state live while its name is cloned.
    let name = unsafe { file.0.with(|s| s.file().name.clone()) };
    opened(Archive::open(Source::Borrowed(file), name))
}

/// Return a fresh name list, including repeated filenames, even after close.
///
/// # Safety
/// Root an exact archive object; the returned list owns its strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_namelist(archive: *const Object) -> *const Object {
    // SAFETY: with_archive holds the rooted archive during list construction.
    let result = unsafe { with_archive(archive, Archive::namelist) };
    finish(result.map(ObjectOwner::into_raw), std::ptr::null())
}

/// Return the archive's shared list of metadata in directory order.
///
/// # Safety
/// The archive remains live and the result owns a reference to the list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_infolist(archive: *const Object) -> *const Object {
    // SAFETY: the archive owns the filelist and we transfer another owner.
    unsafe { with_archive(archive, |a| ObjectOwner::into_raw(a.filelist.clone())) }
}

/// Look up the last entry of a given name, preserving all duplicates in infolist.
///
/// # Safety
/// The exact archive and filename remain rooted and the result is owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_getinfo(
    archive: *const Object,
    name: *const Str,
) -> *const Object {
    // SAFETY: the caller roots the exact archive and key for this operation.
    let result = unsafe {
        with_archive(archive, |a| {
            a.get(&*name)
                .map(|entry| ObjectOwner::into_raw(entry.clone()))
        })
    };
    finish(result, std::ptr::null())
}

/// Return the input's name, or None for an anonymous memory stream.
///
/// # Safety
/// The archive remains live; a non-null result transfers a string owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_filename(archive: *const Object) -> *const Str {
    // SAFETY: the caller supplies an archive that stays live for the metadata query.
    unsafe {
        with_archive(archive, |a| {
            a.filename
                .clone()
                .map_or(std::ptr::null(), StrOwner::into_raw)
        })
    }
}

/// Open an independent binary cursor; an existing cursor outlives archive.close().
///
/// # Safety
/// Root both handles. The discriminator selects a str (0) or ZipInfo (1) key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_member(
    archive: *const Object,
    key: *const c_void,
    is_info: u32,
) -> *const Object {
    // SAFETY: the caller roots the exact archive and key for this operation.
    let result = unsafe { with_archive(archive, |a| a.member(key, is_info)) };
    finish(
        result.map(|m| ObjectOwner::into_raw(files::FileState::member(m))),
        std::ptr::null(),
    )
}

/// Read one complete member, including validation of its full uncompressed CRC.
///
/// # Safety
/// Root both handles. The discriminator selects a str (0) or ZipInfo (1) key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_read(
    archive: *const Object,
    key: *const c_void,
    is_info: u32,
) -> *const Bytes {
    let result =
        // SAFETY: the caller supplies an archive that stays live for the metadata query.
        unsafe { with_archive(archive, |a| a.member(key, is_info)) }.and_then(|mut m| m.read(None));
    finish(
        result.map(|b| crate::BytesOwner::into_raw(crate::BytesOwner::new(b))),
        std::ptr::null(),
    )
}

/// Read a zipimport resource using its cached directory and a fresh file descriptor.
///
/// # Safety
/// The caller roots a path-backed ZIP index and a string for the entire call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zipimport_data(
    archive: *const Object,
    name: *const Str,
) -> *const Bytes {
    // SAFETY: the archive and name are rooted by the sealed zipimport operation.
    let result = unsafe { with_archive(archive, |a| a.import_data(&*name)) };
    finish(
        result.map(|bytes| crate::BytesOwner::into_raw(crate::BytesOwner::new(bytes))),
        std::ptr::null(),
    )
}

/// Return the first member with a BadZipFile error, or None for a healthy archive.
///
/// # Safety
/// The archive is rooted; a non-null result transfers a string owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_test(archive: *const Object) -> *const Str {
    // SAFETY: the caller roots the exact archive and key for this operation.
    let result = unsafe { with_archive(archive, Archive::test) };
    finish(
        result.map(|n| n.map_or(std::ptr::null(), StrOwner::into_raw)),
        std::ptr::null(),
    )
}

/// Release only the archive's source owner. Metadata and previously opened members survive.
///
/// # Safety
/// Root an exact archive object for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_close(archive: *const Object) {
    // SAFETY: releasing a sealed file or caller-owned stream cannot reenter Python.
    unsafe {
        object(archive).0.with_mut(|s| {
            let Storage::ZipFile(a) = s else {
                unreachable!()
            };
            a.source.take();
        })
    };
}

/// Return the visible member filename (preserving the original name separately for validation).
///
/// # Safety
/// The exact ZipInfo remains live and the result transfers a string owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_info_name(info: *const Object) -> *const Str {
    // SAFETY: the caller roots the metadata throughout string acquisition.
    StrOwner::into_raw(unsafe { object(info) }.zip_info().filename.clone())
}

macro_rules! info_integer {
    ($name:ident,$field:ident) => {
        #[doc = concat!("Return ZipInfo.",stringify!($field)," as an owned Python integer.\n\n# Safety\nRoot an exact ZipInfo for this operation.")]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(info:*const Object)->usize{
            // SAFETY: every getter receives a rooted ZipInfo and copies its numeric field.
            Int::from_usize(unsafe{object(info)}.zip_info().$field as usize).into_raw()
        }
    };
}
info_integer!(imre_zip_info_size, file_size);
info_integer!(imre_zip_info_compress_size, compress_size);
info_integer!(imre_zip_info_crc, crc);
info_integer!(imre_zip_info_method, method);
info_integer!(imre_zip_info_external_attr, external_attr);

/// Check the ZIP directory convention (a trailing POSIX slash).
///
/// # Safety
/// The caller supplies a live, exact ZipInfo.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_zip_info_is_dir(info: *const Object) -> u32 {
    // SAFETY: the caller keeps the metadata and its name live.
    u32::from(unsafe { object(info) }.zip_info().filename.ends_with('/'))
}
