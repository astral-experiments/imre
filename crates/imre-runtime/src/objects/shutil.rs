//! POSIX executable lookup shares the caller's credentials and current working directory.
use crate::StrOwner;
mod copy;
mod rmtree;

use std::borrow::Cow;
use std::collections::HashSet;
use std::ffi::OsString;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStringExt;

use num_traits::ToPrimitive;

use super::exceptions::{
    Error, Kind, finish, imre_exception_matches, replace_pending, sequence_allocation_failed,
    take_pending,
};
use super::{Bytes, Int, Str};

/// os.confstr("CS_PATH") returns None for an indeterminate value, or raises
/// OSError for a failed POSIX query. os.defpath is consulted only when the
/// Python attribute or configuration name is unavailable.
#[unsafe(no_mangle)]
pub extern "C" fn imre_shutil_confstr_cs_path() -> *const Bytes {
    let mut small = [0; 255];
    let mut large = Vec::new();
    let mut buffer: &mut [u8] = &mut small;
    loop {
        // SAFETY: this is the calling thread's errno; confstr borrows the
        // writable buffer only until return. Zero distinguishes None from error.
        let len = unsafe {
            #[cfg(target_os = "linux")]
            {
                *libc::__errno_location() = 0;
            }
            #[cfg(target_os = "macos")]
            {
                *libc::__error() = 0;
            }
            libc::confstr(libc::_CS_PATH, buffer.as_mut_ptr().cast(), buffer.len())
        };
        if len == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                super::files::errno_error(error).raise();
            }
            return std::ptr::null();
        }
        if len <= buffer.len() {
            return crate::BytesOwner::into_raw(crate::BytesOwner::new(buffer[..len - 1].to_vec()));
        }
        if large.try_reserve(len - large.len()).is_err() {
            sequence_allocation_failed();
            return std::ptr::null();
        }
        large.resize(len, 0);
        buffer = &mut large;
    }
}

/// The POSIX os.defpath attribute is independent of the confstr result.
#[unsafe(no_mangle)]
pub extern "C" fn imre_shutil_os_defpath() -> *const Bytes {
    crate::BytesOwner::into_raw(crate::BytesOwner::new(b"/bin:/usr/bin".to_vec()))
}

type PathProvider = extern "C" fn() -> *const Bytes;

fn default_path(
    confstr: PathProvider,
    defpath: PathProvider,
) -> Result<Option<Vec<u8>>, LookupError> {
    let mut value = confstr();
    if let Some(error) = take_pending() {
        // SAFETY: take_pending transfers ownership of the exception through both checks.
        let unavailable = unsafe {
            imre_exception_matches(
                crate::ObjectOwner::as_ptr(&error),
                Kind::AttributeError as u32,
            ) != 0
                || imre_exception_matches(
                    crate::ObjectOwner::as_ptr(&error),
                    Kind::ValueError as u32,
                ) != 0
        };
        if !unavailable {
            replace_pending(Some(error));
            return Err(LookupError::Pending);
        }
        value = defpath();
        if super::exceptions::imre_error_pending() != 0 {
            return Err(LookupError::Pending);
        }
    }
    if value.is_null() {
        Ok(None)
    } else {
        // SAFETY: a present provider result transfers an exact Bytes owner.
        Ok(Some(crate::BytesOwner::unwrap_or_clone(unsafe {
            crate::BytesOwner::from_raw(value)
        })))
    }
}

enum LookupError {
    Allocation,
    ModeOverflow,
    Pending,
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

fn which(
    cmd: &[u8],
    mode: &Int,
    path: Option<&[u8]>,
    confstr: PathProvider,
    defpath: PathProvider,
) -> LookupResult {
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
    let path = match path {
        Some(path) => Cow::Borrowed(path),
        None => match std::env::var_os("PATH") {
            Some(path) => Cow::Owned(path.into_vec()),
            None => match default_path(confstr, defpath)? {
                Some(path) => Cow::Owned(path),
                None => return Ok(None),
            },
        },
    };
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
/// Each OS provider transfers an owned Bytes handle or returns null for None or a
/// pending Python exception. Both are called only after the environment lookup misses.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_which(
    cmd: *const (),
    mode: usize,
    path: *const (),
    cmd_bytes: u32,
    path_bytes: u32,
    confstr: PathProvider,
    defpath: PathProvider,
) -> *const () {
    // SAFETY: the compiler supplies both tags and keeps every input rooted across the lookup.
    let result = unsafe {
        let path = (!path.is_null()).then(|| spelling(path, path_bytes));
        which(
            spelling(cmd, cmd_bytes),
            &Int::borrow(mode),
            path,
            confstr,
            defpath,
        )
    };
    let result = match result {
        Err(LookupError::Pending) => return std::ptr::null(),
        Err(LookupError::Allocation) => {
            sequence_allocation_failed();
            return std::ptr::null();
        }
        Err(LookupError::ModeOverflow) => Err(Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C int",
        )),
        Ok(None) => Ok(std::ptr::null()),
        Ok(Some(name)) if cmd_bytes != 0 => {
            Ok(crate::BytesOwner::into_raw(crate::BytesOwner::new(name)).cast())
        }
        Ok(Some(name)) => super::paths::path_utf8(OsString::from_vec(name))
            .map(|text| StrOwner::into_raw(StrOwner::new(text)).cast()),
    };
    finish(result, std::ptr::null())
}
