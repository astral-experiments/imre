//! Copy metadata in CPython order: timestamps, extended attributes, mode, then flags.

#[cfg(target_os = "macos")]
use super::{Error, Kind};
use super::{Name, Result};
use rustix::fs::{AtFlags, CWD, Mode, Timespec, Timestamps, chmodat, utimensat};
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;

fn follow_links(src: &Name<'_>, dst: &Name<'_>, follow: bool) -> bool {
    follow || !(src.path().is_symlink() && dst.path().is_symlink())
}

fn stat(src: &Name<'_>, follow: bool) -> Result<Metadata> {
    let path = src.checked_path()?;
    if follow {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    }
    .map_err(|e| src.error(e))
}

fn chmod(dst: &Name<'_>, st: &Metadata, follow: bool) -> Result<()> {
    // CPython has no lchmod on Linux, and never changes the symlink target's mode here.
    if !follow && cfg!(target_os = "linux") {
        return Ok(());
    }
    let flags = if follow {
        AtFlags::empty()
    } else {
        AtFlags::SYMLINK_NOFOLLOW
    };
    chmodat(
        CWD,
        dst.checked_path()?,
        Mode::from_raw_mode((st.mode() & 0o7777) as _),
        flags,
    )
    .map_err(|e| dst.error(e.into()))
}

pub(super) fn copymode(src: &Name<'_>, dst: &Name<'_>, follow: bool) -> Result<()> {
    let follow = follow_links(src, dst, follow);
    chmod(dst, &stat(src, follow)?, follow)
}

pub(super) fn copystat(src: &Name<'_>, dst: &Name<'_>, follow: bool) -> Result<()> {
    let follow = follow_links(src, dst, follow);
    let st = stat(src, follow)?;
    let flags = if follow {
        AtFlags::empty()
    } else {
        AtFlags::SYMLINK_NOFOLLOW
    };
    let times = Timestamps {
        last_access: Timespec {
            tv_sec: st.atime(),
            tv_nsec: st.atime_nsec(),
        },
        last_modification: Timespec {
            tv_sec: st.mtime(),
            tv_nsec: st.mtime_nsec(),
        },
    };
    utimensat(CWD, dst.checked_path()?, &times, flags).map_err(|e| dst.error(e.into()))?;
    copyxattr(src, dst, follow)?;
    chmod(dst, &st, follow)?;
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt as DarwinMetadata;
        use std::os::unix::ffi::OsStrExt;
        let dst_c = std::ffi::CString::new(dst.path().as_os_str().as_bytes())
            .map_err(|_| Error::new(Kind::ValueError, "embedded null byte"))?;
        // SAFETY: both calls borrow the terminated path and retain no pointers.
        let status = unsafe {
            if follow {
                libc::chflags(dst_c.as_ptr(), st.st_flags())
            } else {
                libc::lchflags(dst_c.as_ptr(), st.st_flags())
            }
        };
        if status < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ENOTSUP) {
                return Err(dst.error(e));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn copyxattr(src: &Name<'_>, dst: &Name<'_>, follow: bool) -> Result<()> {
    use rustix::fs::{XattrFlags, getxattr, lgetxattr, listxattr, llistxattr, lsetxattr, setxattr};
    use rustix::io::Errno;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    // Linux bounds each name list and each attribute value at 64 KiB. Retry a
    // short buffer: the names and values may change between successive calls.
    fn read(
        mut fetch: impl FnMut(&mut [u8]) -> rustix::io::Result<usize>,
    ) -> rustix::io::Result<Vec<u8>> {
        let mut buf = vec![0; 256];
        loop {
            match fetch(&mut buf) {
                Ok(n) => {
                    buf.truncate(n);
                    return Ok(buf);
                }
                Err(Errno::RANGE) if buf.len() < 65536 => buf.resize(65536, 0),
                Err(e) => return Err(e),
            }
        }
    }
    let src_path = src.checked_path()?;
    let names = match read(|buf| {
        if follow {
            listxattr(src_path, buf)
        } else {
            llistxattr(src_path, buf)
        }
    }) {
        Ok(names) => names,
        Err(Errno::NOTSUP | Errno::NODATA | Errno::INVAL) => return Ok(()),
        Err(e) => return Err(src.error(e.into())),
    };
    for name in names.split(|&b| b == 0).filter(|s| !s.is_empty()) {
        let name = OsStr::from_bytes(name);
        let value = match read(|buf| {
            if follow {
                getxattr(src_path, name, buf)
            } else {
                lgetxattr(src_path, name, buf)
            }
        }) {
            Ok(value) => value,
            Err(Errno::PERM | Errno::NOTSUP | Errno::NODATA | Errno::INVAL | Errno::ACCESS) => {
                continue;
            }
            Err(e) => return Err(src.error(e.into())),
        };
        let flags = XattrFlags::empty();
        let dst_path = dst.checked_path()?;
        match if follow {
            setxattr(dst_path, name, &value, flags)
        } else {
            lsetxattr(dst_path, name, &value, flags)
        } {
            Ok(())
            | Err(Errno::PERM | Errno::NOTSUP | Errno::NODATA | Errno::INVAL | Errno::ACCESS) => {}
            Err(e) => return Err(dst.error(e.into())),
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(super) fn copyxattr(_src: &Name<'_>, _dst: &Name<'_>, _follow: bool) -> Result<()> {
    // CPython only exposes the os.*xattr functions on Linux.
    Ok(())
}
