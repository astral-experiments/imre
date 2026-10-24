//! Raw POSIX spellings. Only ASCII slash and dot are special, including in byte paths.

use std::ops::Range;
use std::rc::Rc;

use super::exceptions::{Error, Kind, sequence_allocation_failed};
use super::{Bytes, Str};

enum Spelling<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
}

enum OwnedSpelling {
    Text(Str),
    Bytes(Bytes),
}

impl OwnedSpelling {
    fn into_raw(self) -> *const () {
        match self {
            Self::Text(text) => Rc::into_raw(Rc::new(text)).cast(),
            Self::Bytes(bytes) => Rc::into_raw(Rc::new(bytes)).cast(),
        }
    }
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
    fn slice(&self, range: Range<usize>) -> Option<OwnedSpelling> {
        match self {
            Self::Text(s) => Str::try_new(&s[range]).ok().map(OwnedSpelling::Text),
            Self::Bytes(b) => {
                let part = &b[range];
                let mut copy = Vec::new();
                copy.try_reserve_exact(part.len()).ok()?;
                copy.extend_from_slice(part);
                Some(OwnedSpelling::Bytes(copy))
            }
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
    // SAFETY: the compiler passes an initialized array with exactly count entries.
    let paths = unsafe { std::slice::from_raw_parts(paths, count) };
    if let Some(&last) = paths.last() {
        // SAFETY: every argument has already been evaluated and rooted.
        let final_part = unsafe { Spelling::borrow(last, kind) };
        if final_part.bytes().starts_with(b"/") {
            // An absolute final path replaces the whole prefix. Return one owner
            // of that argument without allocating a copy.
            match final_part {
                // SAFETY: the spelling tag proves the rooted Rc payload type.
                Spelling::Text(_) => unsafe { Rc::increment_strong_count(last.cast::<Str>()) },
                // SAFETY: the spelling tag proves the rooted Rc payload type.
                Spelling::Bytes(_) => unsafe { Rc::increment_strong_count(last.cast::<Bytes>()) },
            }
            return last;
        }
    }
    let mut output = Vec::new();
    for &path in paths {
        // SAFETY: the compiler rooted each argument and proved a homogeneous tag.
        let part = unsafe { Spelling::borrow(path, kind) };
        let bytes = part.bytes();
        if bytes.starts_with(b"/") {
            output.clear();
        }
        let separator = !output.is_empty() && output.last() != Some(&b'/');
        // A slice is bounded by isize::MAX, so one separator still fits usize.
        if output
            .try_reserve(bytes.len() + usize::from(separator))
            .is_err()
        {
            sequence_allocation_failed();
            return std::ptr::null();
        }
        if separator {
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
    let range = if head == 0 {
        cut..spelling.len()
    } else {
        // A directory made entirely of slashes retains them all.
        let end = spelling[..cut]
            .iter()
            .rposition(|&b| b != b'/')
            .map_or(cut, |i| i + 1);
        0..end
    };
    match path.slice(range) {
        Some(result) => result.into_raw(),
        None => {
            sequence_allocation_failed();
            std::ptr::null()
        }
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
    // Keep both slices owned here until the complete tuple can be published.
    let parts = path.slice(0..cut).and_then(|root| {
        let ext = path.slice(cut..spelling.len())?;
        Some((root, ext))
    });
    if let Some((root, ext)) = parts {
        // SAFETY: the fresh tuple has two pointer-width slots and takes both owners.
        unsafe {
            out.write(root.into_raw());
            out.add(1).write(ext.into_raw());
        }
    } else {
        sequence_allocation_failed();
    }
}
