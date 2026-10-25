//! Report filesystem accounting and real terminal dimensions in the compiled tuple layout.
use std::io;
use std::mem::MaybeUninit;

use num_bigint::{BigInt, Sign};

use super::super::exceptions::{Error, Kind, Result, finish};
use super::super::records::{RecordDescriptor, imre_record_try_new};
use super::ownership::Arg;
use crate::{Int, List};

/// Move each owned integer into the exact compiler-provided result record.
unsafe fn record<const N: usize>(layout: *const RecordDescriptor, values: [Int; N]) -> *mut usize {
    // SAFETY: the caller provides a live static record descriptor with N integer slots.
    let result = unsafe { imre_record_try_new(layout) };
    if !result.is_null() {
        for (i, value) in values.into_iter().enumerate() {
            // SAFETY: allocation reserves every one of the N initially zero slots.
            unsafe { result.add(i).write(value.into_raw()) };
        }
    }
    result
}

fn usage(path: Arg) -> Result<[Int; 3]> {
    let (fd, name) = path.path_or_fd("statvfs")?;
    let mut st = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the OS initializes st on success. A path is terminated; the -1
    // descriptor sentinel supplies a null pathname to the OS as in os.statvfs.
    let status = unsafe {
        if fd != -1 {
            libc::fstatvfs(fd, st.as_mut_ptr())
        } else {
            libc::statvfs(
                name.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                st.as_mut_ptr(),
            )
        }
    };
    if status != 0 {
        return Err(path.os_error(io::Error::last_os_error()));
    }
    // SAFETY: statvfs or fstatvfs succeeded and initialized this record.
    let st = unsafe { st.assume_init() };
    let block = BigInt::from(st.f_frsize);
    let total = BigInt::from(st.f_blocks);
    let used = &total - st.f_bfree;
    Ok([
        Int::new(&total * &block),
        Int::new(used * &block),
        Int::new(BigInt::from(st.f_bavail) * &block),
    ])
}

/// # Safety
/// The path uses its compiler-supplied descriptor; layout has three integer fields.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_disk_usage(
    path: usize,
    kind: u32,
    layout: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: the compiler roots the path and the three-field layout is static.
    let result = unsafe { usage(Arg::read(path, kind)).map(|values| record(layout, values)) };
    finish(result, std::ptr::null_mut())
}

/// Python int() accepts signed and underscored Unicode decimal strings. Its conversion
/// limit counts digits (including leading zeroes), independently of the integer value.
fn positive_variable(key: &str) -> Option<Int> {
    let value = std::env::var(key).ok()?;
    if value.len() > 640 {
        let count = value
            .chars()
            .filter(|&c| crate::integer_text::decimal_digit(c).is_some())
            .count();
        let limit = std::env::var("PYTHONINTMAXSTRDIGITS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n == 0 || n >= 640)
            .unwrap_or(4300);
        if limit != 0 && count > limit {
            return None;
        }
    }
    crate::parse_decimal_int(&value).filter(|n| n.big().sign() == Sign::Plus)
}

enum Fallback<'a> {
    Tuple(&'a [usize]),
    List(&'a List),
    Invalid,
}
impl Fallback<'_> {
    fn len(&self) -> Option<usize> {
        match self {
            Self::Tuple(s) => Some(s.len()),
            // SAFETY: no mutation or callback occurs during this length read.
            Self::List(l) => Some(unsafe { l.0.with(Vec::len) }),
            Self::Invalid => None,
        }
    }
    fn get(&self, index: usize) -> Result<Int> {
        match self {
            Self::Tuple(s) => {
                let word = s
                    .get(index)
                    .ok_or_else(|| Error::new(Kind::IndexError, "tuple index out of range"))?;
                // SAFETY: each slot contains a rooted Python integer until the call returns.
                Ok((*unsafe { Int::borrow(*word) }).clone())
            }
            // SAFETY: cloning the owned integer cannot reenter or mutate the caller's list.
            Self::List(l) => unsafe { l.0.with(|values| values.get(index).cloned()) }
                .ok_or_else(|| Error::new(Kind::IndexError, "list index out of range")),
            Self::Invalid => Err(Error::new(
                Kind::TypeError,
                "constructor requires a sequence",
            )),
        }
    }
}

fn dimensions(fallback: Fallback<'_>) -> Result<[Int; 2]> {
    let [columns, lines] = ["COLUMNS", "LINES"].map(positive_variable);
    let (columns, lines) = match (columns, lines) {
        (Some(c), Some(l)) => return Ok([c, l]),
        rest => rest,
    };
    let mut size = MaybeUninit::<libc::winsize>::uninit();
    // SAFETY: on success TIOCGWINSZ initializes one winsize in the supplied buffer.
    let status = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, size.as_mut_ptr()) };
    let actual = if status == 0 {
        // SAFETY: the successful ioctl initialized the buffer.
        let size = unsafe { size.assume_init() };
        [size.ws_col, size.ws_row]
    } else {
        match fallback.len() {
            Some(2) => {}
            Some(n) => {
                return Err(Error::new(
                    Kind::TypeError,
                    format!("os.terminal_size() takes a 2-sequence ({n}-sequence given)"),
                ));
            }
            None => {
                return Err(Error::new(
                    Kind::TypeError,
                    "constructor requires a sequence",
                ));
            }
        }
        [0, 0]
    };
    let choose = |i: usize, env: Option<Int>| match env {
        Some(n) => Ok(n),
        None if actual[i] > 0 => Ok(Int::from_usize(usize::from(actual[i]))),
        None => fallback.get(i),
    };
    Ok([choose(0, columns)?, choose(1, lines)?])
}

/// # Safety
/// A tuple fallback has len rooted integer slots; a list is a rooted List. The
/// static result descriptor has exactly the columns and lines integer slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_terminal_size(
    word: usize,
    kind: u32,
    len: usize,
    layout: *const RecordDescriptor,
) -> *mut usize {
    let fallback = match kind {
        // SAFETY: the compiler supplies the exact tuple payload and field count.
        0 => Fallback::Tuple(unsafe { std::slice::from_raw_parts(word as *const usize, len) }),
        // SAFETY: list[int] uses the owning List handle, rooted for this call.
        1 => Fallback::List(unsafe { &*(word as *const List) }),
        _ => Fallback::Invalid,
    };
    // SAFETY: the static return layout owns two integer fields.
    let result = dimensions(fallback).map(|values| unsafe { record(layout, values) });
    finish(result, std::ptr::null_mut())
}
