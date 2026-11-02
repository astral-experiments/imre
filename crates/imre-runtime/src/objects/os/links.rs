//! POSIX link calls use the same pathname converter and error fields as os.open and os.unlink.
use super::super::Object;
use super::super::exceptions::Result;
use super::super::files::errno_with_filenames;
use super::syscall_path;
use rustix::fs::{CWD, readlinkat_raw, symlinkat};
use std::ffi::{CStr, OsString};
use std::io;
use std::os::unix::ffi::OsStringExt;

/// Read the stored target without following the link.
///
/// # Safety
/// The caller roots an exact Path for the conversion and syscall.
pub(in crate::objects) unsafe fn readlink(path: *const Object) -> Result<OsString> {
    // SAFETY: tag 2 selects the caller's rooted Path and its original text.
    let (path, name) = unsafe { syscall_path(path.cast(), 2, "readlink", "path") }?;
    readlink_at(&path, libc::AT_FDCWD)
        .map(OsString::from_vec)
        .map_err(|e| errno_with_filenames(e, Some(name), None))
}

/// Read a link using the same MAXPATHLEN limit as CPython's POSIX os.readlink.
pub(crate) fn readlink_at(path: &CStr, dir_fd: i32) -> io::Result<Vec<u8>> {
    let mut target = [0u8; libc::PATH_MAX as usize];
    let count = if dir_fd == libc::AT_FDCWD {
        readlinkat_raw(CWD, path, &mut target[..])?
    } else {
        // Python accepts arbitrary integers, including closed descriptors. These cannot
        // be represented by BorrowedFd; let the kernel validate the descriptor.
        // SAFETY: path is NUL-terminated, and target is writable up to its passed length.
        let count = unsafe {
            libc::readlinkat(
                dir_fd,
                path.as_ptr(),
                target.as_mut_ptr().cast(),
                target.len(),
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        count as usize
    };
    Ok(target[..count].to_vec())
}

/// Link two directory-relative paths, optionally dereferencing the final source symlink.
pub(crate) fn link_at(
    src_fd: i32,
    source: &CStr,
    dst_fd: i32,
    destination: &CStr,
    follow: bool,
) -> io::Result<()> {
    let flags = if follow { libc::AT_SYMLINK_FOLLOW } else { 0 };
    // SAFETY: the C strings live through the call. The kernel validates both raw descriptors.
    if unsafe { libc::linkat(src_fd, source.as_ptr(), dst_fd, destination.as_ptr(), flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Create the link without following the destination. The source is stored verbatim.
pub(crate) fn symlink_at(source: &CStr, dir_fd: i32, destination: &CStr) -> io::Result<()> {
    if dir_fd == libc::AT_FDCWD {
        return Ok(symlinkat(source, CWD, destination)?);
    }
    // SAFETY: both C strings remain valid throughout the call; the kernel checks the raw fd.
    if unsafe { libc::symlinkat(source.as_ptr(), dir_fd, destination.as_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Convert both original filenames in order. CheckSymlink runs before a user flag's truth method.
///
/// # Safety
/// The caller roots the destination Path and the source str, bytes, or tagged str | bytes.
pub(in crate::objects) unsafe fn link(
    path: *const Object,
    word: usize,
    tag: u32,
    kind: u32,
) -> Result<()> {
    let function = if kind == 2 { "link" } else { "symlink" };
    let (word, tag) = if tag == 2 {
        (word & !7, (word & 7) as u32)
    } else {
        (word, tag)
    };
    // SAFETY: the compiler emits str (0) or bytes (1); the destination is always a Path (2).
    let (src, source) = unsafe { syscall_path(word as *const (), tag, function, "src") }?;
    // SAFETY: the destination remains rooted and tag 2 selects an exact Path.
    let (dst, destination) = unsafe { syscall_path(path.cast(), 2, function, "dst") }?;
    match kind {
        0 => symlink_at(&src, libc::AT_FDCWD, &dst),
        1 => return Ok(()),
        2 => link_at(libc::AT_FDCWD, &src, libc::AT_FDCWD, &dst, true),
        _ => unreachable!("unknown pathlib link operation"),
    }
    .map_err(|e| errno_with_filenames(e, Some(source), Some(destination)))
}
