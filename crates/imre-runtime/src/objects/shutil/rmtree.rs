//! Walk held directory descriptors. Snapshot names, unlink leaves, then visit
//! subdirectories in reverse enumeration order and remove their parents last.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::super::exceptions::{Error, Filename, Kind, Result, finish};
use super::super::files::{close_file, errno_with_filenames};
use super::super::tempdirs::{Entry, scan_entries};
use super::Int;
use num_traits::ToPrimitive;
use rustix::fs::{AtFlags, CWD, Dir, Mode, OFlags, fstat, openat, statat, unlinkat};

struct Frame {
    fd: OwnedFd,
    path: PathBuf,
    name: CString,
    children: std::vec::IntoIter<Entry>,
}

fn path_error(path: &Path, error: io::Error) -> Error {
    match filename(path) {
        Ok(path) => errno_with_filenames(error, Some(path), None),
        Err(e) => e,
    }
}

fn filename(path: &Path) -> Result<Filename> {
    // CPython's fd-safe walker calls fsdecode even for a bytes input.
    super::super::paths::path_utf8(path.as_os_str().to_owned())
        .map(|s| Filename::Str(std::rc::Rc::new(s)))
}

struct Policy {
    ignore: bool,
}

impl Policy {
    fn error(&self, path: &Path, error: io::Error, missing_ok: bool) -> Result<()> {
        if self.ignore || missing_ok && error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(path_error(path, error))
        }
    }

    fn open(
        &self,
        parent: BorrowedFd<'_>,
        name: CString,
        path: PathBuf,
        root: bool,
    ) -> Result<Option<Frame>> {
        let original = match statat(parent, name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => st,
            Err(e) => {
                self.error(&path, e.into(), !root)?;
                return Ok(None);
            }
        };
        // NONBLOCK also lets a top-level FIFO produce ENOTDIR instead of hanging.
        let fd = match openat(
            parent,
            name.as_c_str(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(e) => {
                self.error(&path, e.into(), !root)?;
                return Ok(None);
            }
        };
        let current = match fstat(&fd) {
            Ok(st) => st,
            Err(e) => {
                self.error(&path, e.into(), !root)?;
                return Ok(None);
            }
        };
        if original.st_dev != current.st_dev || original.st_ino != current.st_ino {
            if self.ignore {
                return Ok(None);
            }
            // The fd-safe CPython walker sets filename on its one-argument OSError.
            // This retains a missing errno and strerror when rendering the exception.
            return Err(Error::os_message_with_filename(
                "Cannot call rmtree on a symbolic link",
                filename(&path)?,
            ));
        }
        let mut children = Vec::new();
        // scandir(fd) duplicates its already-open descriptor. Re-opening "."
        // here would require execute permission on a readable empty directory.
        let entries = fd
            .try_clone()
            .and_then(|scan| Ok(Dir::new(scan)?))
            .and_then(|scan| scan_entries(&fd, scan));
        match entries {
            Ok(list) => {
                for entry in list {
                    if entry.directory {
                        children.push(entry);
                    } else if let Err(e) = unlinkat(&fd, entry.name.as_c_str(), AtFlags::empty()) {
                        self.error(
                            &path.join(OsStr::from_bytes(entry.name.to_bytes())),
                            e.into(),
                            true,
                        )?;
                    }
                }
            }
            Err(e) => self.error(&path, e, !root)?,
        }
        children.reverse();
        Ok(Some(Frame {
            fd,
            name,
            path,
            children: children.into_iter(),
        }))
    }
}

pub(super) fn remove(path: &Path, dirfd: Option<i32>, ignore: bool) -> Result<()> {
    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::new(Kind::ValueError, "embedded null byte"))?;
    let policy = Policy { ignore };
    // Absolute paths ignore dir_fd even if it is closed. For a relative path,
    // duplicate the untrusted integer before constructing a Rust fd owner.
    let parent = match dirfd.filter(|_| !path.is_absolute()) {
        Some(raw) if raw != libc::AT_FDCWD => {
            // SAFETY: fcntl validates raw; on success it returns a new owned descriptor.
            let fd = unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 0) };
            if fd < 0 {
                return policy.error(path, io::Error::last_os_error(), false);
            }
            // SAFETY: this successful fcntl transferred one fresh descriptor.
            Some(unsafe { OwnedFd::from_raw_fd(fd) })
        }
        _ => None,
    };
    let rootfd = parent.as_ref().map_or(CWD, AsFd::as_fd);
    let Some(root) = policy.open(rootfd, name, path.to_owned(), true)? else {
        return Ok(());
    };
    let mut stack = vec![root];
    while let Some(frame) = stack.last_mut() {
        if let Some(child) = frame.children.next() {
            let path = frame.path.join(OsStr::from_bytes(child.name.to_bytes()));
            if let Some(child) = policy.open(frame.fd.as_fd(), child.name, path, false)? {
                stack.push(child);
            }
        } else {
            let frame = stack.pop().unwrap();
            if let Err(e) = close_file(File::from(frame.fd)) {
                policy.error(&frame.path, e, false)?;
            }
            let parent = stack.last().map_or(rootfd, |f| f.fd.as_fd());
            if let Err(e) = unlinkat(parent, frame.name.as_c_str(), AtFlags::REMOVEDIR) {
                policy.error(&frame.path, e.into(), !stack.is_empty())?;
            }
        }
    }
    Ok(())
}

/// # Safety
/// The path is a rooted str/bytes (tags 0/1), or null for None. Integer fd
/// words are rooted until return; zero denotes an absent optional integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_rmtree(
    path: *const (),
    tag: u32,
    ignore: u32,
    fd: usize,
    fd_kind: u32,
) {
    let result = (|| {
        if tag == 2 {
            return Err(Error::new(
                Kind::TypeError,
                "lstat: path should be string, bytes or os.PathLike, not NoneType",
            ));
        }
        if fd_kind == 2 {
            return Err(Error::new(
                Kind::TypeError,
                "argument should be integer or None, not str",
            ));
        }
        let dirfd = if fd != 0 {
            // SAFETY: nonzero denotes a live exact Python integer.
            let borrowed = unsafe { Int::borrow(fd) };
            let value = borrowed.big();
            Some(value.to_i32().ok_or_else(|| {
                Error::new(
                    Kind::OverflowError,
                    if value.sign() == num_bigint::Sign::Minus {
                        "fd is less than minimum"
                    } else {
                        "fd is greater than maximum"
                    },
                )
            })?)
        } else {
            None
        };
        // SAFETY: only the compiler's str/bytes tags reach spelling.
        let bytes = unsafe { super::spelling(path, tag) };
        remove(Path::new(OsStr::from_bytes(bytes)), dirfd, ignore != 0)
    })();
    finish(result, ());
}
