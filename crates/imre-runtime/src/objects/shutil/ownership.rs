//! Resolve ownership names before asking the operating system to change the file.
use std::ffi::CString;
use std::io;
use std::rc::Rc;

use num_bigint::Sign;
use num_traits::ToPrimitive;

use super::super::exceptions::{Error, Filename, Kind, finish, push_string_repr};
use super::super::{
    Bytes, Int, Str, acquire_rc, files::errno_with_filenames, unions, words::NULLABLE,
};
use super::accounts::account;

type Result<T> = std::result::Result<T, Error>;

pub(super) enum Arg {
    None,
    Bool(bool),
    Integer(Int),
    Text(Rc<Str>),
    Path(Rc<Str>),
    Bytes(Rc<Bytes>),
    Other(&'static str),
}

impl Arg {
    /// The caller keeps the concrete value, including any optional or union member, rooted.
    pub(super) unsafe fn read(word: usize, kind: u32) -> Self {
        if kind & NULLABLE != 0 && word == 0 {
            return Self::None;
        }
        match kind & !NULLABLE {
            0 => Self::None,
            1 => Self::Bool(if kind & NULLABLE != 0 {
                word == 2
            } else {
                word != 0
            }),
            // SAFETY: the caller roots the exact integer while we acquire a reference.
            2 => Self::Integer((*unsafe { Int::borrow(word) }).clone()),
            // SAFETY: kind 3 identifies the caller's rooted string.
            3 => Self::Text(unsafe { acquire_rc(word) }),
            7 => Self::Other("float"),
            // SAFETY: kind 8 identifies the caller's rooted byte vector.
            8 => Self::Bytes(unsafe { acquire_rc(word) }),
            9 if word & 7 == 6 => {
                // SAFETY: this union member roots a pathlib.Path; path_text acquires its immutable spelling.
                Self::Path(unsafe {
                    Rc::from_raw(super::super::paths::imre_path_text((word & !7) as *const _))
                })
            }
            9 => {
                let (value, member) = unions::member(word);
                // SAFETY: removing the union tag recovers exactly the identified rooted member.
                unsafe { Self::read(value, member.raw()) }
            }
            _ => Self::Other("object"),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::None => "NoneType",
            Self::Bool(_) => "bool",
            Self::Integer(_) => "int",
            Self::Text(_) => "str",
            Self::Path(_) => "PosixPath",
            Self::Bytes(_) => "bytes",
            Self::Other(name) => name,
        }
    }

    fn resolve(self, user: bool) -> Result<Self> {
        match self {
            Self::Text(name) => {
                if name.as_bytes().contains(&0) {
                    return Err(Error::new(Kind::ValueError, "embedded null byte"));
                }
                account(user, 0, Some(&name))
                    .map(|(id, _)| Self::Integer(Int::from_usize(id as usize)))
                    .ok_or_else(|| {
                        let mut message =
                            format!("no such {}: ", if user { "user" } else { "group" });
                        push_string_repr(&name, &mut message);
                        Error::new(Kind::LookupError, message)
                    })
            }
            Self::None => Ok(Self::Integer(Int::small(-1).unwrap())),
            Self::Bool(value) => Ok(Self::Integer(Int::from_usize(usize::from(value)))),
            Self::Integer(_) => Ok(self),
            _ if user => Ok(self),
            _ => Err(Error::new(
                Kind::TypeError,
                format!(
                    "getgrnam() argument 'name' must be str, not {}",
                    self.label()
                ),
            )),
        }
    }

    fn id(&self, name: &str) -> Result<u32> {
        let Self::Integer(n) = self else {
            return Err(Error::new(
                Kind::TypeError,
                format!("{name} should be integer, not {}", self.label()),
            ));
        };
        let n = n.big();
        if n.to_i64() == Some(-1) {
            return Ok(u32::MAX);
        }
        n.to_u32().ok_or_else(|| {
            // CPython first tests for truncation from a signed long; that case reports underflow.
            let below = n.sign() == Sign::Minus || n.to_i64().is_some();
            overflow(name, below)
        })
    }

    pub(super) fn path_or_fd(&self, operation: &str) -> Result<(i32, Option<CString>)> {
        let (fd, name) = match self {
            Arg::Text(name) | Arg::Path(name) => (-1, Some(name.as_bytes())),
            Arg::Bytes(name) => (-1, Some(name.as_slice())),
            Arg::Bool(fd) => (i32::from(*fd), None),
            Arg::Integer(fd) => (Arg::fd(fd)?, None),
            other => {
                return Err(Error::new(
                    Kind::TypeError,
                    format!(
                        "{operation}: path should be string, bytes, os.PathLike or integer, not {}",
                        other.label()
                    ),
                ));
            }
        };
        let name = name.map(CString::new).transpose().map_err(|_| {
            Error::new(
                Kind::ValueError,
                format!("{operation}: embedded null character in path"),
            )
        })?;
        Ok((fd, name))
    }

    pub(super) fn os_error(self, error: io::Error) -> Error {
        let name = match self {
            Self::Text(s) | Self::Path(s) => Filename::Str(s),
            Self::Bytes(b) => Filename::Bytes(b),
            Self::Integer(n) => Filename::Descriptor(n),
            Self::Bool(b) => Filename::Bool(b),
            _ => unreachable!(),
        };
        errno_with_filenames(error, Some(name), None)
    }

    fn fd(n: &Int) -> Result<i32> {
        let n = n.big();
        n.to_i32()
            .ok_or_else(|| overflow("fd", n.sign() == Sign::Minus))
    }
}

fn overflow(name: &str, below: bool) -> Error {
    Error::new(
        Kind::OverflowError,
        format!(
            "{name} is {}",
            if below {
                "less than minimum"
            } else {
                "greater than maximum"
            }
        ),
    )
}

fn chown(path: Arg, user: Arg, group: Arg, dir_fd: Arg, follow: bool) -> Result<()> {
    if matches!(user, Arg::None) && matches!(group, Arg::None) {
        return Err(Error::new(
            Kind::ValueError,
            "user and/or group must be set",
        ));
    }
    let user = user.resolve(true)?;
    let group = group.resolve(false)?;
    let (path_fd, name) = path.path_or_fd("chown")?;
    let uid = user.id("uid")?;
    let gid = group.id("gid")?;
    let dir_fd = match dir_fd {
        Arg::None => libc::AT_FDCWD,
        Arg::Bool(fd) => i32::from(fd),
        Arg::Integer(n) => Arg::fd(&n)?,
        other => {
            return Err(Error::new(
                Kind::TypeError,
                format!("argument should be integer or None, not {}", other.label()),
            ));
        }
    };
    if path_fd != -1 && dir_fd != libc::AT_FDCWD {
        return Err(Error::new(
            Kind::ValueError,
            "chown: can't specify both dir_fd and fd",
        ));
    }
    if path_fd >= 0 && !follow {
        return Err(Error::new(
            Kind::ValueError,
            "chown: cannot use fd and follow_symlinks together",
        ));
    }
    // SAFETY: the file descriptors remain caller-owned, and a present pathname is
    // terminated for the complete syscall. CPython's -1 fd sentinel reaches chown
    // with a null path: report the syscall's EFAULT without dereferencing null.
    let status = unsafe {
        if path_fd != -1 {
            libc::fchown(path_fd, uid, gid)
        } else {
            libc::fchownat(
                dir_fd,
                name.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                uid,
                gid,
                if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW },
            )
        }
    };
    if status == 0 {
        return Ok(());
    }
    Err(path.os_error(io::Error::last_os_error()))
}

/// # Safety
/// The four compiler-supplied descriptors describe rooted machine words for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_chown(
    path: usize,
    pk: u32,
    user: usize,
    uk: u32,
    group: usize,
    gk: u32,
    dir: usize,
    dk: u32,
    follow: u32,
) {
    // SAFETY: each word has the declared representation until the call returns.
    let result = unsafe {
        chown(
            Arg::read(path, pk),
            Arg::read(user, uk),
            Arg::read(group, gk),
            Arg::read(dir, dk),
            follow != 0,
        )
    };
    finish(result, ());
}
