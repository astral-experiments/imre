//! Direct-with temporary workspaces. Creation is exclusive and private; cleanup
//! walks held directory descriptors and never follows a descendant symlink.

use crate::StrOwner;
use std::cell::RefCell;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::os::fd::{IntoRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(any(target_os = "linux", test))]
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use rustix::fs::{self as rfs, AtFlags, Dir, FileType, Mode, OFlags};
use rustix::io::Errno;

use super::Str;
use super::exceptions::{Error, Kind, Result, finish};
use super::files::{errno_with_filename, os_error};

const ATTEMPTS: usize = 20;

thread_local! {
    static DEFAULT_DIRECTORY: RefCell<Option<Str>> = const { RefCell::new(None) };
}

/// Eight unbiased characters provide over forty random bits; O_EXCL/mkdir is
/// the final collision guard. No process-global PRNG or predictable counter is used.
fn random_name() -> io::Result<String> {
    const CHARACTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut result = String::with_capacity(8);
    while result.len() != 8 {
        let mut entropy = [0_u8; 16];
        getrandom::fill(&mut entropy)?;
        for byte in entropy {
            if byte < 222 {
                result.push(CHARACTERS[usize::from(byte) % CHARACTERS.len()] as char);
                if result.len() == 8 {
                    break;
                }
            }
        }
    }
    Ok(result)
}

/// Python's abspath is lexical: collapse `..` without resolving symlinks and
/// retain exactly two leading slashes. Preserve native bytes until the API boundary.
fn absolute(path: &Path) -> io::Result<PathBuf> {
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let bytes = full.as_os_str().as_bytes();
    let mut components = Vec::new();
    for component in bytes.split(|byte| *byte == b'/') {
        match component {
            b"" | b"." => {}
            b".." => {
                components.pop();
            }
            value => components.push(value),
        }
    }
    let mut result = if bytes.starts_with(b"//") && !bytes.starts_with(b"///") {
        b"//".to_vec()
    } else {
        b"/".to_vec()
    };
    for component in components {
        if result.last() != Some(&b'/') {
            result.push(b'/');
        }
        result.extend_from_slice(component);
    }
    Ok(PathBuf::from(OsString::from_vec(result)))
}

fn path_text(path: PathBuf) -> Result<Str> {
    path.into_os_string()
        .into_string()
        .map(Str::from)
        .map_err(|_| {
            Error::new(
                Kind::UnicodeDecodeError,
                "temporary directory path is not UTF-8",
            )
        })
}

/// The default-directory probe writes and removes a private file, matching
/// tempfile's usability test rather than accepting an existing but unwritable dir.
fn probe(directory: &Path) -> io::Result<bool> {
    for _ in 0..ATTEMPTS {
        let path = directory.join(random_name()?);
        let mut file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Ok(false),
        };
        let written = file.write_all(b"blat");
        let descriptor = file.into_raw_fd();
        // SAFETY: into_raw_fd transferred one descriptor; close exactly once,
        // including EINTR, because retrying might close a reused descriptor.
        let closed = if unsafe { libc::close(descriptor) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
        let removed = fs::remove_file(path);
        return Ok(removed.and(closed).and(written).is_ok());
    }
    Ok(false)
}

fn default_directory() -> Result<Str> {
    DEFAULT_DIRECTORY.with(|cached| {
        if let Some(directory) = cached.borrow().as_ref() {
            return Ok(directory.clone());
        }
        let candidates = ["TMPDIR", "TEMP", "TMP"]
            .into_iter()
            .filter_map(std::env::var_os)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .chain(["/tmp", "/var/tmp", "/usr/tmp"].map(PathBuf::from))
            .chain(std::iter::once(
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            ));
        for candidate in candidates {
            let candidate = if candidate == Path::new(".") {
                candidate
            } else {
                absolute(&candidate).map_err(os_error)?
            };
            if probe(&candidate).map_err(os_error)? {
                let directory = path_text(candidate)?;
                *cached.borrow_mut() = Some(directory.clone());
                return Ok(directory);
            }
        }
        Err(Error::new(
            Kind::FileNotFoundError,
            "No usable temporary directory found",
        ))
    })
}

fn create(directory: Option<&str>, prefix: &str, suffix: &str) -> Result<Str> {
    let directory = directory.map_or_else(default_directory, |value| Ok(Str::from(value)))?;
    for _ in 0..ATTEMPTS {
        let path = Path::new(directory.as_str()).join(format!(
            "{prefix}{}{suffix}",
            random_name().map_err(os_error)?
        ));
        if path.as_os_str().as_bytes().contains(&0) {
            return Err(Error::new(Kind::ValueError, "embedded null byte"));
        }
        // Validate the representable result before creating anything. Creation
        // still uses the original spelling, preserving OS traversal through `..`.
        let name = path_text(absolute(&path).map_err(os_error)?)?;
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(name),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(errno_with_filename(error, path.to_str())),
        }
    }
    Err(Error::new(
        Kind::FileExistsError,
        "No usable temporary directory name found",
    ))
}

/// Repair the held inode. Linux O_PATH descriptors need a separate chmod path.
fn reset_permissions(descriptor: &OwnedFd) -> io::Result<()> {
    match rfs::fchmod(descriptor, Mode::RWXU) {
        #[cfg(target_os = "linux")]
        Err(Errno::BADF) => reset_path_permissions(descriptor),
        result => Ok(result?),
    }
}

#[cfg(target_os = "linux")]
fn reset_path_permissions(held: &OwnedFd) -> io::Result<()> {
    let descriptor = held.as_raw_fd();
    // SAFETY: AT_EMPTY_PATH addresses the held O_PATH inode directly. The empty
    // string is terminated and no lookup or symlink traversal is performed.
    if unsafe {
        libc::syscall(
            linux_raw_sys::general::__NR_fchmodat2 as libc::c_long,
            descriptor,
            c"".as_ptr(),
            0o700_u32,
            libc::AT_EMPTY_PATH,
        )
    } == 0
    {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if !matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP)
    ) {
        return Err(error);
    }
    fs::set_permissions(
        format!("/proc/self/fd/{descriptor}"),
        fs::Permissions::from_mode(0o700),
    )
}

fn open_directory(parent: &OwnedFd, name: &CStr) -> io::Result<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    match rfs::openat(parent, name, flags, Mode::empty()) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            #[cfg(target_os = "macos")]
            {
                reset_entry_permissions(parent, name)?;
                Ok(rfs::openat(parent, name, flags, Mode::empty())?)
            }
            #[cfg(target_os = "linux")]
            {
                let held = rfs::openat(
                    parent,
                    name,
                    OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                reset_permissions(&held)?;
                Ok(rfs::openat(&held, c".", flags, Mode::empty())?)
            }
        }
        result => Ok(result?),
    }
}

pub(super) struct Entry {
    pub(super) name: CString,
    pub(super) directory: bool,
}

fn file_type_at(parent: &OwnedFd, name: &CStr) -> rustix::io::Result<FileType> {
    let stat = rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
    Ok(FileType::from_raw_mode(stat.st_mode))
}

/// Inspect the held inode so a concurrently substituted symlink is skipped.
#[cfg(target_os = "linux")]
fn reset_entry_permissions(parent: &OwnedFd, name: &CStr) -> io::Result<()> {
    let flags = OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let held = match rfs::openat(parent, name, flags, Mode::empty()) {
        Err(Errno::NOENT) => return Ok(()),
        result => result?,
    };
    if FileType::from_raw_mode(rfs::fstat(&held)?.st_mode) != FileType::Symlink {
        reset_permissions(&held)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn reset_entry_permissions(parent: &OwnedFd, name: &CStr) -> io::Result<()> {
    let result = match file_type_at(parent, name) {
        Ok(FileType::Symlink) => return Ok(()),
        // NOFOLLOW protects the target if another process replaces the entry.
        Ok(_) => rfs::chmodat(parent, name, Mode::RWXU, AtFlags::SYMLINK_NOFOLLOW),
        Err(error) => Err(error),
    };
    match result {
        Err(Errno::NOENT) => Ok(()),
        result => Ok(result?),
    }
}

/// Snapshot names as native bytes; cleanup must remove undecodable filenames too.
/// Close the scan descriptor before descending, while retaining the directory fd.
fn entries(descriptor: &OwnedFd) -> io::Result<std::vec::IntoIter<Entry>> {
    scan_entries(descriptor, Dir::read_from(descriptor)?)
}

pub(super) fn scan_entries(
    descriptor: &OwnedFd,
    scan: Dir,
) -> io::Result<std::vec::IntoIter<Entry>> {
    let mut result = Vec::new();
    for item in scan_names(scan) {
        let (name, kind) = item?;
        let directory = kind == FileType::Directory
            || kind == FileType::Unknown
                && file_type_at(descriptor, &name) == Ok(FileType::Directory);
        result.push(Entry { name, directory });
    }
    Ok(result.into_iter())
}

/// Yield the kernel's names and type hints without resolving unknown entries.
pub(super) fn scan_names(scan: Dir) -> impl Iterator<Item = io::Result<(CString, FileType)>> {
    scan.filter_map(|entry| match entry {
        Err(error) => Some(Err(error.into())),
        Ok(entry) if matches!(entry.file_name().to_bytes(), b"." | b"..") => None,
        Ok(entry) => Some(Ok((entry.file_name().to_owned(), entry.file_type()))),
    })
}

fn remove_at(parent: &OwnedFd, name: &CStr, flags: AtFlags) -> io::Result<()> {
    match rfs::unlinkat(parent, name, flags) {
        Err(Errno::NOENT) => Ok(()),
        result => Ok(result?),
    }
}

/// Permission recovery may discover a directory whose initial DT_UNKNOWN stat
/// failed. Return true for that retry case so the caller descends, as tempfile's
/// onexc handler does after its repaired unlink raises IsADirectoryError.
fn unlink_entry(parent: &OwnedFd, name: &CStr) -> io::Result<bool> {
    match remove_at(parent, name, AtFlags::empty()) {
        Ok(()) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            reset_permissions(parent)?;
            reset_entry_permissions(parent, name)?;
            match remove_at(parent, name, AtFlags::empty()) {
                Err(error) if error.kind() == io::ErrorKind::IsADirectory => Ok(true),
                // Darwin reports EPERM when unlinkat encounters a directory.
                #[cfg(target_os = "macos")]
                Err(error) if error.raw_os_error() == Some(libc::EPERM) => {
                    match file_type_at(parent, name) {
                        Ok(FileType::Directory) => Ok(true),
                        Err(Errno::NOENT) => Ok(false),
                        _ => Err(error),
                    }
                }
                result => result.map(|()| false),
            }
        }
        Err(error) => Err(error),
    }
}

struct Frame {
    descriptor: OwnedFd,
    name: CString,
    entries: std::vec::IntoIter<Entry>,
}

impl Frame {
    fn open(parent: &OwnedFd, name: CString) -> io::Result<Self> {
        let descriptor = open_directory(parent, &name)?;
        let entries = match entries(&descriptor) {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                reset_permissions(&descriptor)?;
                entries(&descriptor)?
            }
            result => result?,
        };
        Ok(Self {
            descriptor,
            name,
            entries,
        })
    }
}

/// Preserve the entry a cleanup syscall failed on in the Python exception.
fn cleanup(path: &str) -> Result<()> {
    let mut entry = PathBuf::from(path);
    remove_tree(&mut entry).map_err(|error| errno_with_filename(error, entry.to_str()))
}

/// Remove entries relative to held directory descriptors. On failure, `path`
/// names the current entry even if a descendant was exchanged for a symlink.
fn remove_tree(path: &mut PathBuf) -> io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let name = CString::new(path.file_name().unwrap_or(OsStr::new(".")).as_bytes())?;
    #[cfg(target_os = "linux")]
    let search = OFlags::PATH | OFlags::DIRECTORY;
    #[cfg(target_os = "macos")]
    let search = OFlags::from_bits_retain(libc::O_SEARCH as _);
    let parent = match rfs::open(parent, search | OFlags::CLOEXEC, Mode::empty()) {
        Err(Errno::NOENT) => return Ok(()),
        result => result?,
    };
    match file_type_at(&parent, &name) {
        Ok(FileType::Symlink) => {
            return Err(io::Error::other("Cannot call rmtree on a symbolic link"));
        }
        Err(Errno::NOENT) => return Ok(()),
        result => {
            result?;
        }
    }
    let root = match Frame::open(&parent, name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    };
    let mut stack = vec![root];
    while let Some(frame) = stack.last_mut() {
        let descriptor = &frame.descriptor;
        if let Some(entry) = frame.entries.next() {
            path.push(OsStr::from_bytes(entry.name.as_bytes()));
            if entry.directory || unlink_entry(descriptor, &entry.name)? {
                let child = Frame::open(descriptor, entry.name.clone()).or_else(|error| {
                    if error.kind() == io::ErrorKind::PermissionDenied {
                        reset_permissions(descriptor)?;
                        Frame::open(descriptor, entry.name)
                    } else {
                        Err(error)
                    }
                });
                match child {
                    Ok(child) => {
                        stack.push(child);
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        } else {
            let frame = stack.pop().unwrap();
            let frame_parent = stack.last().map_or(&parent, |parent| &parent.descriptor);
            if let Err(error) = remove_at(frame_parent, &frame.name, AtFlags::REMOVEDIR) {
                if error.kind() != io::ErrorKind::PermissionDenied {
                    return Err(error);
                }
                if !stack.is_empty() {
                    reset_permissions(frame_parent)?;
                }
                reset_permissions(&frame.descriptor)?;
                remove_at(frame_parent, &frame.name, AtFlags::REMOVEDIR)?;
            }
        }
        if !stack.is_empty() {
            path.pop();
        }
    }
    Ok(())
}

/// # Safety
/// Root the optional exact directory string and required prefix/suffix strings.
/// Pending error state must be clear. Success transfers one absolute pathname;
/// the compiler retains a separate hidden owner until its finally cleanup runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_tempdir_new(
    directory: *const Str,
    prefix: *const Str,
    suffix: *const Str,
) -> *const Str {
    // SAFETY: the caller roots all present immutable strings through creation.
    let result = unsafe {
        create(
            directory.as_ref().map(|value| value.as_str()),
            &*prefix,
            &*suffix,
        )
    };
    finish(
        result.map(|name| StrOwner::into_raw(StrOwner::new(name))),
        std::ptr::null(),
    )
}

/// # Safety
/// Root the exact name returned by new with clear pending error state. This
/// removes filesystem state, never consumes the name owner, and reports cleanup
/// failures so finally can replace an earlier return or exception correctly.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_tempdir_cleanup(name: *const Str) {
    // SAFETY: the hidden immutable name remains live through descriptor cleanup.
    let result = cleanup(unsafe { &*name });
    finish(result, ());
}

/// A stored context roots its enter value at construction. Temporary workspaces
/// are removed on explicit exit or on the last owner's release before exit.
pub(super) struct DirectoryContext {
    value: super::words::OwnedWord,
    temporary: bool,
    armed: std::cell::Cell<bool>,
}

impl DirectoryContext {
    pub(super) fn needs_cleanup(&self) -> bool {
        self.armed.get()
    }

    fn remove(&self) -> Result<()> {
        if self.temporary {
            self.armed.set(false);
            // SAFETY: a temporary context always owns a tagged string (tag zero).
            cleanup(unsafe { &*(self.value.word as *const Str) })?;
        }
        Ok(())
    }
}

impl Drop for DirectoryContext {
    fn drop(&mut self) {
        if self.armed.get() {
            // Finalization cannot overwrite an in-flight return or exception.
            let _ = self.remove();
        }
    }
}

/// # Safety
/// The tagged word is a rooted str, Path, or absent value. Temporary creation
/// has already succeeded and provides the exact directory name as a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_directory_context_new(
    value: usize,
    temporary: u32,
) -> *const super::Object {
    // SAFETY: the compiler restricts the payload to nullable str | Path.
    let value = unsafe { super::words::WordKind::new(256 | 9).acquire(value) };
    let temporary = temporary != 0;
    let manager = DirectoryContext {
        value,
        temporary,
        armed: std::cell::Cell::new(temporary),
    };
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(super::Object(
        super::StorageCell::new(super::Storage::DirectoryContext(manager)),
    )))
}

/// # Safety
/// The manager remains rooted. Each enter returns a fresh owner for the same value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_directory_context_enter(manager: *const super::Object) -> usize {
    // SAFETY: both handles are rooted while the payload owner is acquired.
    unsafe {
        super::object(manager).0.with(|storage| {
            let super::Storage::DirectoryContext(manager) = storage else {
                unreachable!()
            };
            manager.value.kind.acquire(manager.value.word).into_word()
        })
    }
}

/// # Safety
/// The manager is rooted, with pending error state clear on entry. Explicit
/// cleanup can raise and participates in normal finally unwinding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_directory_context_exit(manager: *const super::Object) {
    // SAFETY: there are no callbacks during descriptor-based cleanup.
    let result = unsafe {
        super::object(manager).0.with(|storage| {
            let super::Storage::DirectoryContext(manager) = storage else {
                unreachable!()
            };
            manager.remove()
        })
    };
    finish(result, ());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::objects::exceptions::{imre_error_pending, imre_error_take, imre_exception_matches};
    use crate::objects::imre_object_release;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "imre-tempdir-tests-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = cleanup(self.0.to_str().unwrap());
        }
    }

    #[test]
    fn private_unique_creation_preserves_prefix_suffix_and_absolute_join_behavior() {
        let directory = Directory::new();
        let root = directory.0.to_str().unwrap();
        let first = create(Some(root), "prefix-", "-suffix").unwrap_or_else(|_| panic!());
        let second = create(Some(root), "prefix-", "-suffix").unwrap_or_else(|_| panic!());
        assert_ne!(first, second);
        for name in [&first, &second] {
            let path = Path::new(name.as_str());
            assert!(path.is_absolute());
            assert_eq!(path.parent().unwrap(), directory.0);
            let leaf = path.file_name().unwrap().to_str().unwrap();
            assert!(leaf.starts_with("prefix-") && leaf.ends_with("-suffix"));
            assert_eq!(leaf.len(), "prefix-".len() + 8 + "-suffix".len());
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        }
        let prefix = format!("{root}/absolute-");
        let absolute_prefix =
            create(Some("missing\0ignored"), &prefix, "").unwrap_or_else(|_| panic!());
        assert_eq!(
            Path::new(absolute_prefix.as_str()).parent().unwrap(),
            directory.0
        );
        assert_eq!(
            absolute(Path::new("//one/two/../three")).unwrap(),
            Path::new("//one/three")
        );
        fs::write(directory.0.join("file"), "").unwrap();
        assert!(!probe(&directory.0.join("file")).unwrap());
        assert!(!probe(&directory.0.join("missing")).unwrap());
        assert!(probe(&directory.0).unwrap());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 4);
    }

    #[test]
    fn cleanup_handles_locked_directories_native_names_and_external_symlinks() {
        let outside = Directory::new();
        fs::write(outside.0.join("keep"), "unchanged").unwrap();
        let directory = Directory::new();
        let nested = directory.0.join("locked");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("native-é"), "bytes").unwrap();
        #[cfg(target_os = "linux")]
        fs::write(
            nested.join(OsString::from_vec(b"native-\xff".to_vec())),
            "bytes",
        )
        .unwrap();
        symlink(&outside.0, directory.0.join("external")).unwrap();
        symlink("absent", nested.join("broken")).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o0)).unwrap();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o0)).unwrap();
        remove_tree(&mut directory.0.clone()).unwrap();
        remove_tree(&mut directory.0.clone()).unwrap();
        assert!(fs::symlink_metadata(&directory.0).is_err());
        assert_eq!(
            fs::read_to_string(outside.0.join("keep")).unwrap(),
            "unchanged"
        );
    }

    #[test]
    fn replacing_the_root_with_a_symlink_or_file_never_removes_its_target() {
        let outside = Directory::new();
        fs::write(outside.0.join("keep"), "unchanged").unwrap();
        let directory = Directory::new();
        fs::remove_dir(&directory.0).unwrap();
        symlink(&outside.0, &directory.0).unwrap();
        let error = remove_tree(&mut directory.0.clone()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(fs::symlink_metadata(&directory.0).unwrap().is_symlink());
        assert_eq!(
            fs::read_to_string(outside.0.join("keep")).unwrap(),
            "unchanged"
        );
        fs::remove_file(&directory.0).unwrap();
        fs::write(&directory.0, "replacement").unwrap();
        assert_eq!(
            remove_tree(&mut directory.0.clone()).unwrap_err().kind(),
            io::ErrorKind::NotADirectory
        );
        assert_eq!(fs::read_to_string(&directory.0).unwrap(), "replacement");
        fs::remove_file(&directory.0).unwrap();
    }

    #[test]
    fn cleanup_repairs_the_root_but_never_changes_its_external_parent() {
        // Root can remove entries regardless of DAC bits; the differential suite
        // still validates all ownership-independent cleanup behavior there.
        // SAFETY: geteuid has no arguments, allocation, or language callbacks.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let directory = Directory::new();
        let child = directory.0.join("workspace");
        fs::create_dir(&child).unwrap();
        fs::set_permissions(&child, fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o500)).unwrap();
        let result = remove_tree(&mut child.clone());
        let parent_mode = fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777;
        let child_mode = fs::metadata(&child).unwrap().permissions().mode() & 0o777;
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(parent_mode, 0o500);
        assert_eq!(child_mode, 0o700);
    }

    #[test]
    fn permission_recovery_discovers_a_previously_unknown_directory() {
        // SAFETY: geteuid reads the process identity without changing state.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let directory = Directory::new();
        fs::create_dir(directory.0.join("child")).unwrap();
        let held = fs::File::open(&directory.0).unwrap().into();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(unlink_entry(&held, c"child").unwrap());
        assert!(directory.0.join("child").is_dir());
        assert_eq!(
            fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn creation_errors_are_pending_and_never_leave_partial_workspaces() {
        let directory = Directory::new();
        let root = Str::from(directory.0.to_str().unwrap());
        for (prefix, expected) in [
            ("absent/prefix-", Kind::FileNotFoundError),
            ("bad\0prefix", Kind::ValueError),
        ] {
            // SAFETY: exact strings remain rooted, and the pending owner is taken
            // and released before the next potentially failing call.
            unsafe {
                assert!(imre_tempdir_new(&root, &Str::from(prefix), &Str::from("")).is_null());
                assert_eq!(imre_error_pending(), 1);
                let error = imre_error_take();
                assert_eq!(imre_exception_matches(error, expected as u32), 1);
                imre_object_release(error);
            }
        }
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }
}
