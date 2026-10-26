//! Walk held directory descriptors. Snapshot names, unlink leaves, then visit
//! subdirectories in reverse enumeration order and remove their parents last.

use crate::StrOwner;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::super::exceptions::{Error, Filename, Kind, replace_pending, swap_active, take_pending};
use super::super::files::{close_file, errno_with_filenames};
use super::super::tempdirs::scan_names;
use super::super::{Object, Str, paths};
use super::Int;
use num_traits::ToPrimitive;
use rustix::fs::{
    AtFlags, CWD, Dir, FileType, Mode, OFlags, Stat, fstat, openat, statat, unlinkat,
};

fn path_error(filename: Filename, error: io::Error) -> Error {
    errno_with_filenames(error, Some(filename), None)
}

fn filename(path: &Path) -> Filename {
    // CPython's fd-safe walker calls fsdecode even for a bytes input.
    Filename::from_fs_text(path.as_os_str().to_owned())
}

/// The failing Python operation; the fd-safe walk reports islink for a failed
/// fstat/identity check, and scandir for opening or reading a scan stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(super) enum Operation {
    Lstat,
    Open,
    IsLink,
    ScanDir,
    Unlink,
    Close,
    Rmdir,
}

type Parent = Option<Rc<OwnedFd>>;

struct Entry {
    parent: Parent,
    name: CString,
    path: PathBuf,
    root: bool,
}

/// The two stat fields used by os.path.samestat on the held descriptor.
fn identity(stat: &Stat) -> (rustix::fs::Dev, u64) {
    (stat.st_dev, stat.st_ino)
}

enum Step {
    Visit(Entry, Option<(rustix::fs::Dev, u64)>),
    Close(Rc<OwnedFd>, PathBuf),
    Rmdir(Entry),
}

struct Walk<'a, F> {
    stack: Vec<Step>,
    onexc: F,
    root: &'a Path,
    root_object: Option<crate::ObjectOwner>,
    default_handler: bool,
}

fn parent_fd(parent: &Parent) -> BorrowedFd<'_> {
    parent.as_deref().map_or(CWD, AsFd::as_fd)
}

impl<F, E> Walk<'_, F>
where
    F: FnMut(Operation, &Path, Error) -> std::result::Result<(), E>,
    E: From<Error>,
{
    /// CPython assigns the original root object to errors from its root step.
    /// Child paths are strings produced by joining the directory and entry names.
    fn error_filename(&self, path: &Path) -> Filename {
        if path.as_os_str() == self.root.as_os_str()
            && let Some(original) = &self.root_object
        {
            return Filename::Path(crate::ObjectOwner::clone(original));
        }
        filename(path)
    }

    fn io_error(
        &mut self,
        op: Operation,
        path: &Path,
        error: io::Error,
        missing_ok: bool,
    ) -> std::result::Result<(), E> {
        if missing_ok && error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            let error = path_error(self.error_filename(path), error);
            (self.onexc)(op, path, error)
        }
    }

    fn close(&mut self, fd: Rc<OwnedFd>, path: &Path) -> std::result::Result<(), E> {
        // Visits which use this descriptor precede its Close on the stack.
        let fd = Rc::try_unwrap(fd).expect("all child references have been released");
        match close_file(File::from(fd)) {
            Ok(()) => Ok(()),
            Err(e) => self.io_error(Operation::Close, path, e, false),
        }
    }

    fn step(&mut self, step: Step) -> std::result::Result<(), E> {
        let (entry, cached_identity) = match step {
            Step::Visit(entry, cached_identity) => (entry, cached_identity),
            Step::Close(fd, path) => return self.close(fd, &path),
            Step::Rmdir(entry) => {
                return match unlinkat(
                    parent_fd(&entry.parent),
                    entry.name.as_c_str(),
                    AtFlags::REMOVEDIR,
                ) {
                    Ok(()) => Ok(()),
                    Err(e) => self.io_error(Operation::Rmdir, &entry.path, e.into(), !entry.root),
                };
            }
        };
        let parent = parent_fd(&entry.parent);
        let original = match cached_identity.map_or_else(
            || {
                statat(parent, entry.name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW)
                    .map(|st| identity(&st))
            },
            Ok,
        ) {
            Ok(st) => st,
            Err(e) => return self.io_error(Operation::Lstat, &entry.path, e.into(), !entry.root),
        };
        // NONBLOCK also lets a top-level FIFO produce ENOTDIR instead of hanging.
        let fd = match openat(
            parent,
            entry.name.as_c_str(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => Rc::new(fd),
            Err(e) => return self.io_error(Operation::Open, &entry.path, e.into(), !entry.root),
        };
        let path = entry.path.clone();
        let root = entry.root;
        let checked = fstat(&*fd);
        let same = checked.as_ref().is_ok_and(|st| identity(st) == original);
        if same {
            self.stack.push(Step::Rmdir(entry));
        }
        self.stack.push(Step::Close(Rc::clone(&fd), path.clone()));
        if let Err(e) = checked {
            return self.io_error(Operation::IsLink, &path, e.into(), !root);
        }
        if !same {
            let error = Error::os_message_with_filename(
                "Cannot call rmtree on a symbolic link",
                self.error_filename(&path),
            );
            return (self.onexc)(Operation::IsLink, &path, error);
        }
        // Duplicate the held descriptor; reopening "." needs search permission
        // even on a readable empty directory that this process can remove.
        let entries = fd
            .try_clone()
            .and_then(|scan| Ok(Dir::new(scan)?))
            .and_then(|scan| scan_names(scan).collect::<io::Result<Vec<_>>>());
        let entries = match entries {
            Ok(entries) => entries,
            Err(e) => return self.io_error(Operation::ScanDir, &path, e, !root),
        };
        for (name, kind) in entries {
            let basename = Path::new(OsStr::from_bytes(name.to_bytes()));
            let full = path.join(basename);
            // DirEntry.is_dir() caches its stat when the directory omits d_type.
            let cached_identity = if kind == FileType::Unknown {
                statat(&*fd, name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW)
                    .ok()
                    .filter(|st| FileType::from_raw_mode(st.st_mode) == FileType::Directory)
                    .map(|st| identity(&st))
            } else {
                None
            };
            if kind == FileType::Directory || cached_identity.is_some() {
                self.stack.push(Step::Visit(
                    Entry {
                        parent: Some(Rc::clone(&fd)),
                        name,
                        path: full,
                        root: false,
                    },
                    cached_identity,
                ));
            } else if let Err(e) = unlinkat(&*fd, name.as_c_str(), AtFlags::empty()) {
                if e == rustix::io::Errno::NOENT {
                    continue;
                }
                // The default handler reraises into the directory step, which
                // assigns its own path. A custom handler sees the unlink basename.
                if self.default_handler {
                    return self.io_error(Operation::Unlink, &path, e.into(), false);
                }
                (self.onexc)(
                    Operation::Unlink,
                    &full,
                    path_error(filename(basename), e.into()),
                )?;
            }
        }
        Ok(())
    }

    fn run(&mut self) -> std::result::Result<(), E> {
        while let Some(step) = self.stack.pop() {
            if let Err(error) = self.step(step) {
                // A handler may raise. Do not remove any more entries, but
                // close every held directory, reporting close failures in order.
                while let Some(step) = self.stack.pop() {
                    if let Step::Close(fd, path) = step {
                        self.close(fd, &path)?;
                    }
                }
                return Err(error);
            }
        }
        Ok(())
    }
}

fn checked_name(bytes: &[u8]) -> Result<CString, Error> {
    CString::new(bytes).map_err(|_| Error::new(Kind::ValueError, "embedded null byte"))
}

/// Remove the copied source of a move using rmtree's default error handler.
pub(super) fn remove_root(path: &Path, original: Option<crate::ObjectOwner>) -> Result<(), Error> {
    let name = checked_name(path.as_os_str().as_bytes())?;
    remove_with(path, original, name, None, true, |_, _, error| Err(error))
}

fn remove_with<E: From<Error>>(
    path: &Path,
    root_object: Option<crate::ObjectOwner>,
    name: CString,
    dirfd: Option<i32>,
    default_handler: bool,
    onexc: impl FnMut(Operation, &Path, Error) -> std::result::Result<(), E>,
) -> std::result::Result<(), E> {
    let mut walk = Walk {
        stack: Vec::new(),
        onexc,
        root: path,
        root_object,
        default_handler,
    };
    // Absolute paths ignore dir_fd even if closed; a relative path needs a
    // reference we own before borrowing the caller's potentially invalid fd.
    let parent = match dirfd.filter(|_| !path.is_absolute()) {
        Some(raw) if raw != libc::AT_FDCWD => {
            // SAFETY: fcntl validates raw and transfers a fresh descriptor.
            let fd = unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 0) };
            if fd < 0 {
                return walk.io_error(Operation::Lstat, path, io::Error::last_os_error(), false);
            }
            // SAFETY: the successful fcntl transferred this descriptor to us.
            Some(Rc::new(unsafe { OwnedFd::from_raw_fd(fd) }))
        }
        _ => None,
    };
    let root = Entry {
        parent,
        name,
        path: path.to_owned(),
        root: true,
    };
    walk.stack.push(Step::Visit(root, None));
    walk.run()
}

enum WalkError {
    Os(Error),
    Raised(crate::ObjectOwner),
}

impl From<Error> for WalkError {
    fn from(error: Error) -> Self {
        Self::Os(error)
    }
}

type Handler = unsafe extern "C" fn(*const (), u32, *const Str, *const Object);

/// # Safety
/// The optional handler accepts a borrowed context, operation, path and exception;
/// it returns with pending state set if it raises. All operands are live through return.
/// The path is a rooted str/bytes (tags 0/1), null for None (2), or a path
/// object (3). Integer fd words are rooted until return; zero denotes absence.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_rmtree(
    path: *const (),
    tag: u32,
    ignore: u32,
    fd: usize,
    fd_kind: u32,
    onerror: Option<Handler>,
    onerror_ctx: *const (),
    onexc: Option<Handler>,
    onexc_ctx: *const (),
) {
    let result: std::result::Result<(), WalkError> = (|| {
        if tag == 2 {
            return Err(Error::new(
                Kind::TypeError,
                "lstat: path should be string, bytes or os.PathLike, not NoneType",
            )
            .into());
        }
        // SAFETY: tag 3 identifies a live path object that an exception can retain.
        let original =
            (tag == 3).then(|| unsafe { crate::ObjectOwner::acquire_word(path as usize) });
        let spelling;
        let bytes = if let Some(original) = &original {
            // SAFETY: original roots the path, and imre_path_text transfers one owner.
            spelling = unsafe {
                StrOwner::from_raw(paths::imre_path_text(crate::ObjectOwner::as_ptr(original)))
            };
            spelling.as_bytes()
        } else {
            // SAFETY: the remaining tags name a rooted str or bytes.
            unsafe { super::spelling(path, tag) }
        };
        let name = checked_name(bytes)?;
        let path = Path::new(OsStr::from_bytes(bytes));
        if fd_kind == 2 {
            return Err(Error::new(
                Kind::TypeError,
                "argument should be integer or None, not str",
            )
            .into());
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
        let handler = onexc
            .map(|f| (f, onexc_ctx))
            .or_else(|| onerror.map(|f| (f, onerror_ctx)))
            .filter(|_| ignore == 0);
        let dispatch = |op: Operation, path: &Path, error: Error| {
            let Some((handler, context)) = handler else {
                return if ignore == 0 {
                    Err(error.into())
                } else {
                    Ok(())
                };
            };
            let path = StrOwner::new(paths::path_utf8(path.as_os_str().to_owned())?);
            error.raise();
            let exc = take_pending().expect("the walk just raised this error");
            let mut previous = Some(crate::ObjectOwner::clone(&exc));
            swap_active(&mut previous);
            // SAFETY: this synchronous call borrows its three owners. The active
            // exception lets a native handler inspect it or use a bare raise.
            unsafe {
                handler(
                    context,
                    op as u32,
                    StrOwner::as_ptr(&path),
                    crate::ObjectOwner::as_ptr(&exc),
                )
            };
            swap_active(&mut previous);
            match take_pending() {
                Some(raised) => Err(WalkError::Raised(raised)),
                None => Ok(()),
            }
        };
        remove_with(
            path,
            original,
            name,
            dirfd,
            handler.is_none() && ignore == 0,
            dispatch,
        )
    })();
    match result {
        Ok(()) => {}
        Err(WalkError::Os(error)) => error.raise(),
        Err(WalkError::Raised(raised)) => replace_pending(Some(raised)),
    }
}
