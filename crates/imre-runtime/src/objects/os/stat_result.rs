//! POSIX stat results share ordinary tuple ownership, comparison and representation.
//! Only their trailing numeric attributes are absent from iteration and equality.

use std::fs::Metadata;
#[cfg(target_os = "macos")]
use std::os::darwin::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;

use super::super::Object;
use super::super::records::{RecordDescriptor, imre_record_try_new};
use super::Int;
use num_traits::ToPrimitive;

const SEQUENCE_FIELDS: usize = 10;

/// Validate the path before a non-boolean follow_symlinks argument invokes user code.
///
/// # Safety
/// The caller roots an exact Path during the OS conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_check_stat(path: *const Object) {
    // SAFETY: tag 2 describes the caller's rooted native Path.
    let result = unsafe { super::syscall_path(path.cast(), 2, "stat", "path") };
    super::finish(result.map(|_| ()), ());
}

/// Look up a concrete path, retaining its original spelling on errors.
///
/// # Safety
/// The caller roots an exact Path and supplies the permanent stat tuple descriptor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_stat(
    path: *const Object,
    follow: u32,
    lstat: u32,
    layout: *const RecordDescriptor,
) -> *mut usize {
    let function = if lstat == 0 { "stat" } else { "lstat" };
    super::finish(
        (|| {
            // SAFETY: tag 2 describes the caller's rooted native Path.
            let (path, filename) =
                unsafe { super::syscall_path(path.cast(), 2, function, "path") }?;
            let path = std::ffi::OsStr::from_bytes(path.as_bytes());
            let meta = super::metadata(path, follow != 0)
                .map_err(|error| super::errno_with_filenames(error, Some(filename), None))?;
            // SAFETY: the descriptor has the ten owning fields and stat's numeric tail.
            Ok(unsafe { pack(meta, layout) })
        })(),
        std::ptr::null_mut(),
    )
}

#[repr(C)]
struct Extra {
    nanoseconds: [i64; 3],
    block_size: u64,
    blocks: u64,
    rdev: u64,
    #[cfg(target_os = "macos")]
    darwin: DarwinFields,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct DarwinFields {
    birthtime: f64,
    flags: u32,
    generation: u32,
}

pub(in crate::objects) const EXTRA_WORDS: usize = size_of::<Extra>() / size_of::<usize>();
const _: () = assert!(
    align_of::<Extra>() == align_of::<usize>()
        && size_of::<Extra>() == EXTRA_WORDS * size_of::<usize>()
);

fn signed(value: i64) -> Int {
    Int::small(value).unwrap_or_else(|| Int::new(value.into()))
}

/// Match the rounding of CPython's C stat expression on its baseline targets.
fn float_seconds(seconds: f64, nanoseconds: i64) -> f64 {
    let fraction = nanoseconds as f64;
    if cfg!(target_arch = "aarch64") {
        // AArch64 fuses the multiply and add, which matters just before the epoch.
        fraction.mul_add(1e-9, seconds)
    } else {
        seconds + 1e-9 * fraction
    }
}

/// # Safety
/// The compiler supplies a permanent ten-int tuple descriptor with the stat attribute flag.
unsafe fn pack(meta: Metadata, layout: *const RecordDescriptor) -> *mut usize {
    // SAFETY: the supplied descriptor describes every visible owning field and the raw tail.
    let result = unsafe { imre_record_try_new(layout) };
    if result.is_null() {
        return result;
    }
    let fields = [
        Int::from_usize(meta.mode() as usize),
        Int::from_usize(meta.ino() as usize),
        Int::from_usize(meta.dev() as usize),
        Int::from_usize(meta.nlink() as usize),
        Int::from_usize(meta.uid() as usize),
        Int::from_usize(meta.gid() as usize),
        signed(meta.size() as i64),
        signed(meta.atime()),
        signed(meta.mtime()),
        signed(meta.ctime()),
    ];
    // SAFETY: this fresh payload owns the first ten tagged integers; the aligned tail
    // contains only numerical values and the descriptor accounts for its allocation.
    unsafe {
        for (i, value) in fields.into_iter().enumerate() {
            result.add(i).write(value.into_raw());
        }
        result.add(SEQUENCE_FIELDS).cast::<Extra>().write(Extra {
            nanoseconds: [meta.atime_nsec(), meta.mtime_nsec(), meta.ctime_nsec()],
            block_size: meta.blksize(),
            blocks: meta.blocks(),
            rdev: meta.rdev(),
            #[cfg(target_os = "macos")]
            darwin: DarwinFields {
                // CPython's POSIX stat packs birthtime via an unsigned long.
                birthtime: float_seconds(
                    meta.st_birthtime() as u64 as f64,
                    meta.st_birthtime_nsec(),
                ),
                flags: meta.st_flags(),
                generation: meta.st_gen(),
            },
        });
    }
    result
}

/// # Safety
/// The caller roots an initialized stat tuple and selects one of its three times.
unsafe fn timestamp(result: *const usize, index: usize) -> (i64, i64) {
    // SAFETY: the stat producer writes three signed time_t values and their fractions.
    unsafe {
        let seconds = Int::borrow(*result.add(7 + index));
        let seconds = seconds
            .as_small()
            .unwrap_or_else(|| seconds.big().to_i64().unwrap());
        let extra = &*result.add(SEQUENCE_FIELDS).cast::<Extra>();
        (seconds, extra.nanoseconds[index])
    }
}

/// # Safety
/// The caller roots an initialized stat tuple and selects a named time attribute.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_stat_time(result: *const usize, index: u32) -> f64 {
    if index == 3 {
        #[cfg(target_os = "macos")]
        // SAFETY: the runtime produced this tuple with the Darwin numeric tail.
        return unsafe {
            (*result.add(SEQUENCE_FIELDS).cast::<Extra>())
                .darwin
                .birthtime
        };
        #[cfg(not(target_os = "macos"))]
        return absent("st_birthtime").returning(0.0);
    }
    // SAFETY: the index is fixed by the compiler's named attribute lookup.
    let (seconds, ns) = unsafe { timestamp(result, index as usize) };
    float_seconds(seconds as f64, ns)
}

fn absent(name: &str) -> super::Error {
    super::Error::new(
        super::Kind::AttributeError,
        format!("'os.stat_result' object has no attribute '{name}'"),
    )
}

/// # Safety
/// The caller roots an initialized stat tuple and selects a named integer attribute.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_stat_extra_int(result: *const usize, index: u32) -> usize {
    if index < 3 {
        // SAFETY: the index names one of the three timestamps.
        let (seconds, ns) = unsafe { timestamp(result, index as usize) };
        return Int::new((i128::from(seconds) * 1_000_000_000 + i128::from(ns)).into()).into_raw();
    }
    // SAFETY: the immutable stat tuple retains the initialized numeric tail.
    let extra = unsafe { &*result.add(SEQUENCE_FIELDS).cast::<Extra>() };
    let value = match index {
        3 => extra.block_size,
        4 => extra.blocks,
        5 => extra.rdev,
        #[cfg(target_os = "macos")]
        6 => u64::from(extra.darwin.flags),
        #[cfg(target_os = "macos")]
        7 => u64::from(extra.darwin.generation),
        #[cfg(not(target_os = "macos"))]
        6 => return absent("st_flags").returning(0),
        #[cfg(not(target_os = "macos"))]
        7 => return absent("st_gen").returning(0),
        8 => return absent("st_birthtime_ns").returning(0),
        9 => 19 + u64::from(cfg!(target_os = "macos")) * 3,
        10 => SEQUENCE_FIELDS as u64,
        11 => 3,
        _ => unreachable!("unknown stat field"),
    };
    Int::from_usize(value as usize).into_raw()
}
