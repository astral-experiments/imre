//! POSIX executable lookup shares the caller's credentials and current working directory.
use std::borrow::Cow;
use std::collections::HashSet;
use std::ffi::{CString, OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::rc::Rc;

use num_traits::ToPrimitive;

use super::exceptions::{Error, Kind, Result, finish};
use super::{Bytes, Int, Str};

/// POSIX provides the default executable search path even when PATH is unset.
fn default_path() -> Vec<u8> {
    // SAFETY: a null output with length zero queries the size including the terminator.
    let size = unsafe { libc::confstr(libc::_CS_PATH, std::ptr::null_mut(), 0) };
    if size > 0 {
        let mut path = vec![0u8; size];
        // SAFETY: the vector has size writable bytes, and confstr bounds its output.
        let written = unsafe { libc::confstr(libc::_CS_PATH, path.as_mut_ptr().cast(), size) };
        if written > 0 && written <= size {
            path.truncate(written - 1);
            return path;
        }
    }
    b"/bin:/usr/bin".to_vec()
}

/// Access uses the real UID/GID. Convert mode after exists() and before isdir(),
/// so a missing path can bypass an overflowing mode, just as CPython does.
fn candidate(dir: &[u8], cmd: &[u8], mode: &Int) -> Result<Option<Vec<u8>>> {
    let mut name = dir.to_vec();
    if !dir.is_empty() && !dir.ends_with(b"/") {
        name.push(b'/');
    }
    name.extend_from_slice(cmd);
    let path = std::path::Path::new(OsStr::from_bytes(&name));
    if !path.exists() {
        return Ok(None);
    }
    let mode = mode.big().to_i32().ok_or_else(|| {
        Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C int",
        )
    })?;
    let Ok(c_name) = CString::new(name.as_slice()) else {
        return Ok(None);
    };
    // SAFETY: c_name owns its terminated path throughout access(), which retains nothing.
    let allowed = unsafe { libc::access(c_name.as_ptr(), mode) } == 0;
    Ok((allowed && !path.is_dir()).then_some(name))
}

fn which(cmd: &[u8], mode: &Int, path: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
    if let Some(last) = cmd.iter().rposition(|&b| b == b'/') {
        // posixpath.split removes trailing separators from the head except when
        // every character is a slash. Joining it again preserves that root.
        let head = &cmd[..=last];
        let len = head
            .iter()
            .rposition(|&b| b != b'/')
            .map_or(head.len(), |i| i + 1);
        return candidate(&head[..len], &cmd[last + 1..], mode);
    }
    let path = path.map_or_else(
        || Cow::Owned(std::env::var_os("PATH").map_or_else(default_path, OsStringExt::into_vec)),
        Cow::Borrowed,
    );
    if path.is_empty() {
        return Ok(None);
    }
    let mut seen = HashSet::new();
    for dir in path.split(|&b| b == b':') {
        if seen.insert(dir)
            && let Some(found) = candidate(dir, cmd, mode)?
        {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// Borrow exact str and bytes handles without decoding byte-valued filesystem paths.
unsafe fn spelling<'a>(handle: *const (), bytes: u32) -> &'a [u8] {
    if bytes == 0 {
        // SAFETY: tag zero selects the compiler-rooted Str.
        unsafe { (&*handle.cast::<Str>()).as_bytes() }
    } else {
        // SAFETY: tag one selects the compiler-rooted Bytes.
        unsafe { &*handle.cast::<Bytes>() }
    }
}

/// # Safety
/// The tagged cmd and non-null search path are rooted Str/Bytes handles and mode is a
/// rooted Python integer. A null search path means PATH. A null result can mean no match.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_which(
    cmd: *const (),
    mode: usize,
    path: *const (),
    cmd_bytes: u32,
    path_bytes: u32,
) -> *const () {
    // SAFETY: the compiler supplies both tags and keeps every input rooted across the lookup.
    let result = unsafe {
        let path = (!path.is_null()).then(|| spelling(path, path_bytes));
        which(spelling(cmd, cmd_bytes), &Int::borrow(mode), path)
    }
    .and_then(|name| match name {
        None => Ok(std::ptr::null()),
        Some(name) if cmd_bytes != 0 => Ok(Rc::into_raw(Rc::new(name)).cast()),
        Some(name) => super::paths::path_utf8(OsString::from_vec(name))
            .map(|text| Rc::into_raw(Rc::new(text)).cast()),
    });
    finish(result, std::ptr::null())
}
