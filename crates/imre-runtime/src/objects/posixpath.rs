//! Raw POSIX spellings. Only ASCII slash and dot are special, including in byte paths.

use std::collections::TryReserveError;
use std::ffi::OsString;
use std::ops::Range;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::rc::Rc;

use super::exceptions::{Error, Kind};
use super::{Bytes, Str};

enum Spelling<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
}

impl<'a> Spelling<'a> {
    /// The compiler tags every rooted handle with its exact static representation.
    unsafe fn borrow(value: *const (), bytes: u32) -> Self {
        if bytes == 0 {
            // SAFETY: tag 0 identifies a rooted Str for this borrow's lifetime.
            Self::Text(unsafe { &*value.cast::<Str>() })
        } else {
            // SAFETY: the caller supplies tag 1 only for a rooted Bytes handle.
            Self::Bytes(unsafe { &*value.cast::<Bytes>() })
        }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            Self::Text(s) => s.as_bytes(),
            Self::Bytes(b) => b,
        }
    }

    /// All split offsets are adjacent to ASCII delimiters or at an endpoint.
    fn slice(&self, range: Range<usize>) -> *const () {
        match self {
            Self::Text(s) => Rc::into_raw(Rc::new(Str::from(&s[range]))).cast(),
            Self::Bytes(b) => Rc::into_raw(Rc::new(b[range].to_vec())).cast(),
        }
    }
}

/// Mixed filesystem spellings fail after all arguments have been evaluated.
#[unsafe(no_mangle)]
pub extern "C" fn imre_posix_join_error(path_in_tail: u32) -> *const () {
    let message = if path_in_tail == 0 {
        "Can't mix strings and bytes in path components"
    } else {
        "join() argument must be str, bytes, or os.PathLike object, not 'PosixPath'"
    };
    Error::new(Kind::TypeError, message).raise();
    std::ptr::null()
}

/// # Safety
/// The array contains `count` rooted handles of tag 0 (str) or 1 (bytes).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_posix_join(
    paths: *const *const (),
    count: usize,
    kind: u32,
) -> *const () {
    let mut output = Vec::new();
    // SAFETY: the compiler passes an initialized array with exactly count entries.
    for &path in unsafe { std::slice::from_raw_parts(paths, count) } {
        // SAFETY: the compiler rooted each argument and proved a homogeneous tag.
        let part = unsafe { Spelling::borrow(path, kind) };
        let bytes = part.bytes();
        if bytes.starts_with(b"/") {
            output.clear();
        } else if !output.is_empty() && output.last() != Some(&b'/') {
            output.push(b'/');
        }
        output.extend_from_slice(bytes);
    }
    if kind == 0 {
        let text = String::from_utf8(output).expect("ASCII separators preserve UTF-8");
        Rc::into_raw(Rc::new(Str::from(text))).cast()
    } else {
        Rc::into_raw(Rc::new(output)).cast()
    }
}

/// # Safety
/// Root an exact str or bytes handle of the indicated tag. The result owns its spelling.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_posix_component(path: *const (), bytes: u32, head: u32) -> *const () {
    // SAFETY: the caller roots the argument with its exact string or bytes tag.
    let path = unsafe { Spelling::borrow(path, bytes) };
    let spelling = path.bytes();
    let cut = spelling
        .iter()
        .rposition(|&b| b == b'/')
        .map_or(0, |i| i + 1);
    if head == 0 {
        path.slice(cut..spelling.len())
    } else {
        // A directory made entirely of slashes retains them all.
        let end = spelling[..cut]
            .iter()
            .rposition(|&b| b != b'/')
            .map_or(cut, |i| i + 1);
        path.slice(0..end)
    }
}

/// # Safety
/// Root a str or bytes handle and a fresh zeroed two-field tuple of the corresponding type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_posix_splitext(path: *const (), bytes: u32, out: *mut *const ()) {
    // SAFETY: the caller roots the argument with its exact string or bytes tag.
    let path = unsafe { Spelling::borrow(path, bytes) };
    let spelling = path.bytes();
    let last = spelling.rsplit(|&b| b == b'/').next().unwrap();
    let cut = last
        .iter()
        .rposition(|&b| b == b'.')
        .filter(|&dot| last[..dot].iter().any(|&b| b != b'.'))
        .map_or(spelling.len(), |dot| spelling.len() - last.len() + dot);
    // SAFETY: the fresh tuple has two pointer-width slots and takes both owners.
    unsafe {
        out.write(path.slice(0..cut));
        out.add(1).write(path.slice(cut..spelling.len()));
    }
}

/// Normalize components lexically. For relative abspath, supply the absolute cwd;
/// for an absolute path or for normpath, pass none.
/// The output buffer doubles as the component stack, so arbitrary bytes need no decoding.
pub(crate) fn normalize(path: &[u8], cwd: Option<&[u8]>) -> Result<Vec<u8>, TryReserveError> {
    let first = cwd.unwrap_or(path);
    let root = if first.starts_with(b"//") && !first.starts_with(b"///") {
        2
    } else {
        usize::from(first.starts_with(b"/"))
    };
    let mut output = Vec::new();
    output.try_reserve(root.max(1))?;
    output.resize(root, b'/');
    for part in cwd
        .into_iter()
        .chain([path])
        .flat_map(|p| p.split(|&b| b == b'/'))
    {
        match part {
            b"" | b"." => continue,
            b".." => {
                let tail = &output[root..];
                let last = tail.rsplit(|&b| b == b'/').next().unwrap_or_default();
                if !last.is_empty() && last != b".." {
                    let end = root + (tail.len() - last.len()).saturating_sub(1);
                    output.truncate(end);
                    continue;
                }
                if root != 0 {
                    continue;
                }
            }
            _ => {}
        }
        let separator = output.len() > root;
        output.try_reserve(part.len() + usize::from(separator))?;
        if separator {
            output.push(b'/');
        }
        output.extend_from_slice(part);
    }
    if output.is_empty() {
        output.push(b'.');
    }
    Ok(output)
}

/// # Safety
/// The caller roots an exact str (tag 0) or bytes (tag 1). Both operations return
/// an owned handle of the input's type, or null with a pending exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_posix_normalize(
    path: *const (),
    bytes: u32,
    absolute: u32,
) -> *const () {
    // SAFETY: the caller keeps the spelling of the indicated type alive.
    let spelling = unsafe { Spelling::borrow(path, bytes) };
    let path = spelling.bytes();
    let cwd = if absolute != 0 && !path.starts_with(b"/") {
        match std::env::current_dir() {
            Ok(cwd) => Some(cwd),
            Err(error) => return super::files::errno_error(error).returning(std::ptr::null()),
        }
    } else {
        None
    };
    let Ok(output) = normalize(path, cwd.as_ref().map(|p| p.as_os_str().as_bytes())) else {
        super::exceptions::sequence_allocation_failed();
        return std::ptr::null();
    };
    if bytes != 0 {
        Rc::into_raw(Rc::new(output)).cast()
    } else {
        // An undecodable cwd component canceled by `..` never reaches the text result.
        match super::paths::path_utf8(OsString::from_vec(output)) {
            Ok(text) => Rc::into_raw(Rc::new(text)).cast(),
            Err(error) => error.returning(std::ptr::null()),
        }
    }
}
