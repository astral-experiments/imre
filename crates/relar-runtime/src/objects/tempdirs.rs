//! Direct-with temporary workspaces. Creation is exclusive and private; cleanup
//! walks held directory descriptors and never follows a descendant symlink.

use std::cell::RefCell;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::Str;
use super::exceptions::{Error, Kind, Result, finish};
use super::files::os_error;

const ATTEMPTS: usize = 20;
const DIRECTORY_FLAGS: i32 =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;

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
        // SAFETY: getrandom writes at most the capacity of this live byte array.
        let count = unsafe { libc::getrandom(entropy.as_mut_ptr().cast(), entropy.len(), 0) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "empty random source",
            ));
        }
        for &byte in &entropy[..count as usize] {
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
            Err(error) => return Err(os_error(error)),
        }
    }
    Err(Error::new(
        Kind::FileExistsError,
        "No usable temporary directory name found",
    ))
}

fn open_at(parent: RawFd, name: &CStr, flags: i32) -> io::Result<OwnedFd> {
    // SAFETY: the name is terminated and the borrowed parent descriptor stays
    // live; these flags never create an entry or require a mode argument.
    let descriptor = unsafe { libc::openat(parent, name.as_ptr(), flags) };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: successful openat transfers a new uniquely owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
    }
}

/// Repair the inode held by the descriptor, never a path that can be exchanged
/// for a symlink. Older kernels use procfs's reference to the same held inode.
fn reset_permissions(descriptor: RawFd) -> io::Result<()> {
    // SAFETY: the caller holds this descriptor throughout the synchronous call.
    if unsafe { libc::fchmod(descriptor, 0o700) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(libc::EBADF) {
        return Err(error);
    }
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

fn open_directory(parent: RawFd, name: &CStr) -> io::Result<OwnedFd> {
    match open_at(parent, name, DIRECTORY_FLAGS) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            let held = open_at(
                parent,
                name,
                libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )?;
            reset_permissions(held.as_raw_fd())?;
            open_at(held.as_raw_fd(), c".", DIRECTORY_FLAGS)
        }
        result => result,
    }
}

struct Entry {
    name: CString,
    directory: bool,
}

struct DirectoryStream(*mut libc::DIR);

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the live DIR and its descriptor.
        unsafe { libc::closedir(self.0) };
    }
}

fn mode_at(parent: RawFd, name: &CStr) -> io::Result<libc::mode_t> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: metadata is writable, the name is terminated, and the borrowed
    // parent remains live. NOFOLLOW reports the entry itself, including symlinks.
    if unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstatat initialized the complete stat structure.
    Ok(unsafe { metadata.assume_init() }.st_mode)
}

/// Permission recovery also repairs regular entries, as tempfile does. Inspect
/// the held inode before chmod so a concurrently substituted symlink is skipped.
fn reset_entry_permissions(parent: RawFd, name: &CStr) -> io::Result<()> {
    let held = match open_at(
        parent,
        name,
        libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    ) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    };
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: held roots the inode and metadata is writable for a complete stat.
    if unsafe { libc::fstat(held.as_raw_fd(), metadata.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstat initialized the complete structure.
    if unsafe { metadata.assume_init() }.st_mode & libc::S_IFMT != libc::S_IFLNK {
        reset_permissions(held.as_raw_fd())?;
    }
    Ok(())
}

/// Snapshot names as native bytes; cleanup must remove undecodable filenames too.
/// Close the scan descriptor before descending, while retaining the directory fd.
fn entries(descriptor: RawFd) -> io::Result<std::vec::IntoIter<Entry>> {
    let scan = open_at(descriptor, c".", DIRECTORY_FLAGS)?;
    // SAFETY: fdopendir borrows scan on failure and takes ownership on success.
    let stream = unsafe { libc::fdopendir(scan.as_raw_fd()) };
    if stream.is_null() {
        return Err(io::Error::last_os_error());
    }
    let _ = scan.into_raw_fd();
    let stream = DirectoryStream(stream);
    let mut result = Vec::new();
    loop {
        // SAFETY: this thread owns both errno and the live directory stream.
        let entry = unsafe {
            *libc::__errno_location() = 0;
            libc::readdir(stream.0)
        };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(0) {
                Ok(result.into_iter())
            } else {
                Err(error)
            };
        }
        // SAFETY: readdir returned a live dirent until the next call; copy its
        // terminated name and kind before advancing the stream.
        let (name, kind) = unsafe {
            (
                CStr::from_ptr((*entry).d_name.as_ptr()).to_owned(),
                (*entry).d_type,
            )
        };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let directory = kind == libc::DT_DIR
            || kind == libc::DT_UNKNOWN
                && mode_at(descriptor, &name)
                    .is_ok_and(|mode| mode & libc::S_IFMT == libc::S_IFDIR);
        result.push(Entry { name, directory });
    }
}

fn remove_at(parent: RawFd, name: &CStr, directory: bool) -> io::Result<()> {
    // SAFETY: the parent stays rooted, the entry name is terminated, and unlinkat
    // never follows the final symlink. AT_REMOVEDIR removes only an empty directory.
    if unsafe {
        libc::unlinkat(
            parent,
            name.as_ptr(),
            if directory { libc::AT_REMOVEDIR } else { 0 },
        )
    } == 0
    {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(error)
    }
}

/// Permission recovery may discover a directory whose initial DT_UNKNOWN stat
/// failed. Return true for that retry case so the caller descends, as tempfile's
/// onexc handler does after its repaired unlink raises IsADirectoryError.
fn unlink_entry(parent: RawFd, name: &CStr) -> io::Result<bool> {
    match remove_at(parent, name, false) {
        Ok(()) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            reset_permissions(parent)?;
            reset_entry_permissions(parent, name)?;
            match remove_at(parent, name, false) {
                Err(error) if error.kind() == io::ErrorKind::IsADirectory => Ok(true),
                result => result.map(|()| false),
            }
        }
        Err(error) => Err(error),
    }
}

struct Frame {
    descriptor: OwnedFd,
    parent: RawFd,
    name: CString,
    entries: std::vec::IntoIter<Entry>,
}

impl Frame {
    fn open(parent: RawFd, name: CString) -> io::Result<Self> {
        let descriptor = open_directory(parent, &name)?;
        let entries = match entries(descriptor.as_raw_fd()) {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                reset_permissions(descriptor.as_raw_fd())?;
                entries(descriptor.as_raw_fd())?
            }
            result => result?,
        };
        Ok(Self {
            descriptor,
            parent,
            name,
            entries,
        })
    }
}

/// Descriptor-relative traversal remains safe if children are exchanged for
/// symlinks. The explicit stack avoids Rust stack growth for deeply nested trees.
fn cleanup(path: &str) -> io::Result<()> {
    let path = Path::new(path);
    let parent = CString::new(
        path.parent()
            .unwrap_or(Path::new("."))
            .as_os_str()
            .as_bytes(),
    )?;
    let name = CString::new(path.file_name().unwrap_or(OsStr::new(".")).as_bytes())?;
    let parent = match open_at(
        libc::AT_FDCWD,
        &parent,
        libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
    ) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    };
    match mode_at(parent.as_raw_fd(), &name) {
        Ok(mode) if mode & libc::S_IFMT == libc::S_IFLNK => {
            return Err(io::Error::other("Cannot call rmtree on a symbolic link"));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => {
            result?;
        }
    }
    let root = match Frame::open(parent.as_raw_fd(), name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    };
    let mut stack = vec![root];
    while let Some(frame) = stack.last_mut() {
        let descriptor = frame.descriptor.as_raw_fd();
        if let Some(entry) = frame.entries.next() {
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
                    Ok(child) => stack.push(child),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        } else {
            let frame = stack.pop().unwrap();
            if let Err(error) = remove_at(frame.parent, &frame.name, true) {
                if error.kind() != io::ErrorKind::PermissionDenied {
                    return Err(error);
                }
                if !stack.is_empty() {
                    reset_permissions(frame.parent)?;
                }
                reset_permissions(frame.descriptor.as_raw_fd())?;
                remove_at(frame.parent, &frame.name, true)?;
            }
        }
    }
    Ok(())
}

/// # Safety
/// Root the optional exact directory string and required prefix/suffix strings.
/// Pending error state must be clear. Success transfers one absolute pathname;
/// the compiler retains a separate hidden owner until its finally cleanup runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_tempdir_new(
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
        result.map(|name| Rc::into_raw(Rc::new(name))),
        std::ptr::null(),
    )
}

/// # Safety
/// Root the exact name returned by new with clear pending error state. This
/// removes filesystem state, never consumes the name owner, and reports cleanup
/// failures so finally can replace an earlier return or exception correctly.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_tempdir_cleanup(name: *const Str) {
    // SAFETY: the hidden immutable name remains live through descriptor cleanup.
    let result = cleanup(unsafe { &*name }).map_err(os_error);
    finish(result, ());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::objects::exceptions::{
        relar_error_pending, relar_error_take, relar_exception_matches,
    };
    use crate::objects::relar_object_release;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "relar-tempdir-tests-{}-{}",
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
        fs::write(
            nested.join(OsString::from_vec(b"native-\xff".to_vec())),
            "bytes",
        )
        .unwrap();
        symlink(&outside.0, directory.0.join("external")).unwrap();
        symlink("absent", nested.join("broken")).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o0)).unwrap();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o0)).unwrap();
        cleanup(directory.0.to_str().unwrap()).unwrap();
        cleanup(directory.0.to_str().unwrap()).unwrap();
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
        let error = cleanup(directory.0.to_str().unwrap()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(fs::symlink_metadata(&directory.0).unwrap().is_symlink());
        assert_eq!(
            fs::read_to_string(outside.0.join("keep")).unwrap(),
            "unchanged"
        );
        fs::remove_file(&directory.0).unwrap();
        fs::write(&directory.0, "replacement").unwrap();
        assert_eq!(
            cleanup(directory.0.to_str().unwrap()).unwrap_err().kind(),
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
        let result = cleanup(child.to_str().unwrap());
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
        let name = CString::new(directory.0.as_os_str().as_bytes()).unwrap();
        let held = open_directory(libc::AT_FDCWD, &name).unwrap();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(unlink_entry(held.as_raw_fd(), c"child").unwrap());
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
                assert!(relar_tempdir_new(&root, &Str::from(prefix), &Str::from("")).is_null());
                assert_eq!(relar_error_pending(), 1);
                let error = relar_error_take();
                assert_eq!(relar_exception_matches(error, expected as u32), 1);
                relar_object_release(error);
            }
        }
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }
}
