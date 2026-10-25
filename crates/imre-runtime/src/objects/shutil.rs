//! POSIX executable lookup shares the caller's credentials and current working directory.
use crate::StrOwner;
use std::borrow::Cow;
use std::collections::HashSet;
use std::ffi::OsString;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStringExt;
use std::rc::Rc;

use num_traits::ToPrimitive;

use super::exceptions::{Error, Kind, finish, sequence_allocation_failed};
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

enum LookupError {
    Allocation,
    ModeOverflow,
}

type LookupResult = Result<Option<Vec<u8>>, LookupError>;

/// Access uses the real UID/GID. Convert mode after stat() and before the directory check,
/// so a missing path can bypass an overflowing mode, just as CPython does.
fn candidate(dir: &[u8], cmd: &[u8], mode: &Int) -> LookupResult {
    if dir.contains(&0) || cmd.contains(&0) {
        return Ok(None);
    }
    let separator = !dir.is_empty() && !dir.ends_with(b"/");
    let capacity = dir
        .len()
        .saturating_add(cmd.len())
        .saturating_add(usize::from(separator) + 1);
    let mut name = Vec::new();
    name.try_reserve_exact(capacity)
        .map_err(|_| LookupError::Allocation)?;
    name.extend_from_slice(dir);
    if separator {
        name.push(b'/');
    }
    name.extend_from_slice(cmd);
    name.push(0);
    let path = name.as_ptr().cast();
    let mut metadata = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: name is NUL-terminated and stable throughout each call. stat writes
    // one struct on success and access retains neither the path nor any output.
    if unsafe { libc::stat(path, metadata.as_mut_ptr()) } != 0 {
        return Ok(None);
    }
    let mode = mode.big().to_i32().ok_or(LookupError::ModeOverflow)?;
    // SAFETY: the terminated name still owns path and access retains no pointer.
    if unsafe { libc::access(path, mode) } != 0 {
        return Ok(None);
    }
    // Match isdir() after access, including a failed second stat returning false.
    // SAFETY: only a successful stat initializes the metadata used in this test.
    if unsafe {
        libc::stat(path, metadata.as_mut_ptr()) == 0
            && metadata.assume_init().st_mode & libc::S_IFMT == libc::S_IFDIR
    } {
        return Ok(None);
    }
    name.pop();
    Ok(Some(name))
}

fn which(cmd: &[u8], mode: &Int, path: Option<&[u8]>) -> LookupResult {
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
        if seen.len() == seen.capacity() {
            if seen.contains(dir) {
                continue;
            }
            seen.try_reserve(1).map_err(|_| LookupError::Allocation)?;
        }
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
    };
    let result = match result {
        Err(LookupError::Allocation) => {
            sequence_allocation_failed();
            return std::ptr::null();
        }
        Err(LookupError::ModeOverflow) => Err(Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C int",
        )),
        Ok(None) => Ok(std::ptr::null()),
        Ok(Some(name)) if cmd_bytes != 0 => Ok(Rc::into_raw(Rc::new(name)).cast()),
        Ok(Some(name)) => super::paths::path_utf8(OsString::from_vec(name))
            .map(|text| StrOwner::into_raw(StrOwner::new(text)).cast()),
    };
    finish(result, std::ptr::null())
}
