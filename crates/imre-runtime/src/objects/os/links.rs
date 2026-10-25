//! POSIX link calls use the same pathname converter and error fields as os.open and os.unlink.
use super::super::Object;
use super::super::exceptions::Result;
use super::super::files::errno_with_filenames;
use super::syscall_path;
use rustix::fs::{AtFlags, CWD, linkat, readlinkat, symlinkat};
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

/// Read the stored target without following the link.
///
/// # Safety
/// The caller roots an exact Path for the conversion and syscall.
pub(in crate::objects) unsafe fn readlink(path: *const Object) -> Result<OsString> {
    // SAFETY: tag 2 selects the caller's rooted Path and its original text.
    let (path, name) = unsafe { syscall_path(path.cast(), 2, "readlink", "path") }?;
    let target = readlinkat(CWD, path.as_c_str(), Vec::new())
        .map_err(|e| errno_with_filenames(e.into(), Some(name), None))?;
    Ok(OsString::from_vec(target.into_bytes()))
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
        0 => symlinkat(src.as_c_str(), CWD, dst.as_c_str()),
        1 => return Ok(()),
        2 => linkat(
            CWD,
            src.as_c_str(),
            CWD,
            dst.as_c_str(),
            AtFlags::SYMLINK_FOLLOW,
        ),
        _ => unreachable!("unknown pathlib link operation"),
    }
    .map_err(|e| errno_with_filenames(e.into(), Some(source), Some(destination)))
}
