//! Walk held directory descriptors. Snapshot names, unlink leaves, then visit
//! subdirectories in reverse enumeration order and remove their parents last.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::super::exceptions::{
    Error, Filename, Kind, Result, replace_pending, swap_active, take_pending,
};
use super::super::files::{close_file, errno_with_filenames};
use super::super::tempdirs::scan_entries;
use super::super::{Object, Str};
use super::Int;
use num_traits::ToPrimitive;
use rustix::fs::{AtFlags, CWD, Dir, Mode, OFlags, fstat, openat, statat, unlinkat};

fn path_error(path: &Path, error: io::Error) -> Error {
    errno_with_filenames(error, Some(filename(path)), None)
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

enum Step {
    Visit(Entry),
    Close(Rc<OwnedFd>, PathBuf),
    Rmdir(Entry),
}

struct Walk<F> {
    stack: Vec<Step>,
    onexc: F,
}

fn parent_fd(parent: &Parent) -> BorrowedFd<'_> {
    parent.as_deref().map_or(CWD, AsFd::as_fd)
}

impl<F, E> Walk<F>
where
    F: FnMut(Operation, &Path, Error) -> std::result::Result<(), E>,
    E: From<Error>,
{
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
            (self.onexc)(op, path, path_error(path, error))
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
        let entry = match step {
            Step::Visit(entry) => entry,
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
        let original = match statat(parent, entry.name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW) {
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
        let same = checked
            .as_ref()
            .is_ok_and(|st| st.st_dev == original.st_dev && st.st_ino == original.st_ino);
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
                filename(&path),
            );
            return (self.onexc)(Operation::IsLink, &path, error);
        }
        // Duplicate the held descriptor; reopening "." needs search permission
        // even on a readable empty directory that this process can remove.
        let entries = fd
            .try_clone()
            .and_then(|scan| Ok(Dir::new(scan)?))
            .and_then(|scan| scan_entries(&fd, scan));
        let entries = match entries {
            Ok(entries) => entries,
            Err(e) => return self.io_error(Operation::ScanDir, &path, e, !root),
        };
        for entry in entries {
            let basename = Path::new(OsStr::from_bytes(entry.name.to_bytes()));
            let full = path.join(basename);
            if entry.directory {
                self.stack.push(Step::Visit(Entry {
                    parent: Some(Rc::clone(&fd)),
                    name: entry.name,
                    path: full,
                    root: false,
                }));
            } else if let Err(e) = unlinkat(&*fd, entry.name.as_c_str(), AtFlags::empty()) {
                if e == rustix::io::Errno::NOENT {
                    continue;
                }
                // os.unlink receives the name relative to fd; onexc receives the full path.
                (self.onexc)(Operation::Unlink, &full, path_error(basename, e.into()))?;
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

fn remove_with<E: From<Error>>(
    path: &Path,
    dirfd: Option<i32>,
    mut onexc: impl FnMut(Operation, &Path, Error) -> std::result::Result<(), E>,
) -> std::result::Result<(), E> {
    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::new(Kind::ValueError, "embedded null byte"))?;
    // Absolute paths ignore dir_fd even if closed; a relative path needs a
    // reference we own before borrowing the caller's potentially invalid fd.
    let parent = match dirfd.filter(|_| !path.is_absolute()) {
        Some(raw) if raw != libc::AT_FDCWD => {
            // SAFETY: fcntl validates raw and transfers a fresh descriptor.
            let fd = unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 0) };
            if fd < 0 {
                return onexc(
                    Operation::Lstat,
                    path,
                    path_error(path, io::Error::last_os_error()),
                );
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
    Walk {
        stack: vec![Step::Visit(root)],
        onexc,
    }
    .run()
}

pub(super) fn remove(path: &Path, dirfd: Option<i32>, ignore: bool) -> Result<()> {
    remove_with(
        path,
        dirfd,
        |_, _, error| if ignore { Ok(()) } else { Err(error) },
    )
}

enum WalkError {
    Os(Error),
    Raised(Rc<Object>),
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
/// The path is a rooted str/bytes (tags 0/1), or null for None. Integer fd
/// words are rooted until return; zero denotes an absent optional integer.
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
        // SAFETY: only the compiler's str/bytes tags reach spelling.
        let bytes = unsafe { super::spelling(path, tag) };
        let path = Path::new(OsStr::from_bytes(bytes));
        let handler = onexc
            .map(|f| (f, onexc_ctx))
            .or_else(|| onerror.map(|f| (f, onerror_ctx)));
        let Some((handler, context)) = handler.filter(|_| ignore == 0) else {
            return Ok(remove(path, dirfd, ignore != 0)?);
        };
        remove_with(path, dirfd, |op, path, error| {
            let path = Rc::new(super::super::paths::path_utf8(path.as_os_str().to_owned())?);
            error.raise();
            let exc = take_pending().expect("the walk just raised this error");
            let mut previous = Some(Rc::clone(&exc));
            swap_active(&mut previous);
            // SAFETY: this synchronous call borrows its three owners. The active
            // exception lets a native handler inspect it or use a bare raise.
            unsafe { handler(context, op as u32, Rc::as_ptr(&path), Rc::as_ptr(&exc)) };
            swap_active(&mut previous);
            match take_pending() {
                Some(raised) => Err(WalkError::Raised(raised)),
                None => Ok(()),
            }
        })
    })();
    match result {
        Ok(()) => {}
        Err(WalkError::Os(error)) => error.raise(),
        Err(WalkError::Raised(raised)) => replace_pending(Some(raised)),
    }
}
