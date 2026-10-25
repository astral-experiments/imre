//! Raw POSIX spellings. Only ASCII slash and dot are special, including in byte paths.

use std::collections::TryReserveError;
use std::ffi::OsString;
use std::ops::Range;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::rc::Rc;

use super::exceptions::{Error, Kind};
use super::{Bytes, Str};

enum Spelling<'a> {
    Text(&'a Str),
    Bytes(&'a Bytes),
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
        if range == (0..self.bytes().len()) {
            // SAFETY: borrow obtained both typed references from rooted Rc payloads.
            return unsafe {
                match self {
                    Self::Text(text) => {
                        Rc::into_raw(super::acquire_rc::<Str>(std::ptr::from_ref(*text) as usize))
                            .cast()
                    }
                    Self::Bytes(bytes) => Rc::into_raw(super::acquire_rc::<Bytes>(
                        std::ptr::from_ref(*bytes) as usize,
                    ))
                    .cast(),
                }
            };
        }
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

fn parts(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    path.split(|&b| b == b'/')
        .filter(|p| !p.is_empty() && *p != b".")
}

/// Shorten a normalized component prefix. Dot-dot is an ordinary component
/// in commonpath: callers choose whether to resolve it with normpath first.
fn intersect(prefix: &mut Vec<u8>, path: &[u8], absolute: bool) {
    let root = usize::from(absolute);
    let mut len = root;
    let mut right = parts(path);
    for left in parts(prefix) {
        if right.next() != Some(left) {
            break;
        }
        len += usize::from(len > root) + left.len();
    }
    prefix.truncate(len);
}

/// Compare exact OS spellings. All type mixing is detected before an
/// absolute/relative conflict, matching the order of CPython's split and check.
///
/// # Safety
/// Root the typed list (or null for None). Kind 0/1/2/3 selects str, bytes,
/// their tagged union, or native paths. The result transfers one owned word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_posix_commonpath(list: *const super::Object, kind: u32) -> usize {
    use super::exceptions::{Result, finish};
    if list.is_null() {
        return Error::new(Kind::TypeError, "'NoneType' object is not iterable").returning(0);
    }
    let compute = |storage: &super::Storage| -> Result<usize> {
        let values = &storage.sequence()?.values;
        let Some(&first) = values.first() else {
            return Err(Error::new(
                Kind::ValueError,
                "commonpath() arg is an empty sequence",
            ));
        };
        let bytes = kind == 1 || (kind == 2 && first & 7 == 1);
        if kind == 2 && values.iter().any(|&word| (word & 7 == 1) != bytes) {
            return Err(Error::new(
                Kind::TypeError,
                "Can't mix strings and bytes in path components",
            ));
        }
        let mut prefix = Vec::new();
        let mut absolute = false;
        for (i, &word) in values.iter().enumerate() {
            let mut step = |path: &[u8]| -> Result<()> {
                if i == 0 {
                    absolute = path.starts_with(b"/");
                    prefix
                        .try_reserve_exact(path.len())
                        .map_err(|_| Error::allocation())?;
                    if absolute {
                        prefix.push(b'/');
                    }
                    for part in parts(path) {
                        if prefix.len() > usize::from(absolute) {
                            prefix.push(b'/');
                        }
                        prefix.extend_from_slice(part);
                    }
                } else {
                    if path.starts_with(b"/") != absolute {
                        return Err(Error::new(
                            Kind::ValueError,
                            "Can't mix absolute and relative paths",
                        ));
                    }
                    intersect(&mut prefix, path, absolute);
                }
                Ok(())
            };
            if kind == 3 {
                // SAFETY: a homogeneous Path list roots the fixed path object during the borrow.
                unsafe {
                    super::object(word as *const super::Object)
                        .0
                        .with(|s| step(s.path().as_bytes()))
                }?;
            } else {
                let pointer = if kind == 2 { word & !7 } else { word };
                // SAFETY: the static element kind or verified uniform tag gives the exact representation.
                let spelling = unsafe { Spelling::borrow(pointer as *const (), u32::from(bytes)) };
                step(spelling.bytes())?;
            }
        }
        if bytes {
            let owner = Rc::into_raw(Rc::new(prefix)) as usize;
            Ok(owner | usize::from(kind == 2))
        } else {
            let value = std::str::from_utf8(&prefix).expect("joined UTF-8 path components");
            Ok(Rc::into_raw(Rc::new(
                Str::try_new(value).map_err(|_| Error::allocation())?,
            )) as usize)
        }
    };
    // SAFETY: the caller roots the list and each member; no callback can reenter the borrow.
    let result = unsafe { super::object(list).0.with(compute) };
    finish(result, 0)
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
