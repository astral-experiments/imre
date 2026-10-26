//! Local civil time uses the platform timezone database and mktime normalization.

use super::{
    Int,
    calendar::c_component,
    exceptions::{Error, Kind, finish},
    records::descriptor,
    words::WordKind,
};
use std::ffi::CStr;

/// # Safety
/// The descriptor has the nine integer slots yielded by a Python struct_time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_clock_parts(
    stamp: f64,
    utc: bool,
    layout: *const super::records::RecordDescriptor,
) -> *mut usize {
    use super::{logging::timed, records::imre_record_try_new};
    let parts = timed::float_seconds(stamp).and_then(|seconds| timed::civil(seconds, utc));
    finish(
        parts.map(|tm| {
            let fields = [
                i64::from(tm.tm_year) + 1900,
                i64::from(tm.tm_mon + 1),
                i64::from(tm.tm_mday),
                i64::from(tm.tm_hour),
                i64::from(tm.tm_min),
                i64::from(tm.tm_sec),
                i64::from((tm.tm_wday + 6) % 7),
                i64::from(tm.tm_yday + 1),
                i64::from(tm.tm_isdst),
            ];
            // SAFETY: the caller supplied the time tuple layout, with two zero-initialized hidden words.
            unsafe {
                let record = imre_record_try_new(layout);
                if record.is_null() {
                    return record;
                }
                for (index, value) in fields.into_iter().enumerate() {
                    record
                        .add(index)
                        .write(Int::small(value).unwrap().into_raw());
                }
                if !tm.tm_zone.is_null() {
                    let zone = CStr::from_ptr(tm.tm_zone).to_string_lossy();
                    let owner = crate::StrOwner::new(crate::Str::from(zone.as_ref()));
                    record
                        .add(9)
                        .write(crate::StrOwner::into_raw(owner) as usize);
                }
                record.add(10).write(Int::from(tm.tm_gmtoff).into_raw());
                record
            }
        }),
        std::ptr::null_mut(),
    )
}

/// # Safety
/// The time descriptor has two attribute kinds after the visible tuple kinds.
unsafe fn extra_kind(record: *const usize, index: usize) -> WordKind {
    // SAFETY: callers pass one of the two indices of a rooted time tuple.
    unsafe { WordKind::new(*descriptor(record).kinds.add(index)) }
}

/// # Safety
/// A rooted struct_time has two additional zero-initialized words for its named attributes.
pub(super) unsafe fn release_extras(record: *const usize) {
    // SAFETY: the final record owner transfers both attributes using their declared kinds.
    unsafe {
        extra_kind(record, 9).release(*record.add(9));
        extra_kind(record, 10).release(*record.add(10));
    }
}

/// # Safety
/// The compiler supplies a fresh time tuple and roots the optional attributes during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_time_extras(record: *mut usize, zone: usize, offset: usize) {
    // SAFETY: each acquired reference is transferred to one initially empty tail slot.
    unsafe {
        extra_kind(record, 9).retain(zone);
        extra_kind(record, 10).retain(offset);
        record.add(9).write(zone);
        record.add(10).write(offset);
    }
}

/// # Safety
/// The time tuple remains rooted while its selected optional attribute is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_time_extra(record: *const usize, zone: bool) -> usize {
    // SAFETY: the descriptor specifies the concrete nullable representation of each hidden field.
    unsafe {
        let word = *record.add(if zone { 9 } else { 10 });
        extra_kind(record, if zone { 9 } else { 10 }).retain(word);
        word
    }
}

/// Validate the list length before loading any of its tuple fields.
#[unsafe(no_mangle)]
pub extern "C" fn imre_time_sequence_length(word: usize) -> usize {
    // Every native list length is a nonnegative tagged immediate.
    let n = word >> 1;
    if (9..=11).contains(&n) {
        return word;
    }
    let bound = if n < 9 { "least 9" } else { "most 11" };
    Error::new(
        Kind::TypeError,
        format!("time.struct_time() takes an at {bound}-sequence ({n}-sequence given)"),
    )
    .raise();
    0
}

/// Validate one __index__ result before invoking any later tuple field's protocol.
#[unsafe(no_mangle)]
pub extern "C" fn imre_time_component(value: usize) -> usize {
    finish(
        c_component(value).map(|n| Int::small(i64::from(n)).unwrap().into_raw()),
        0,
    )
}

/// Validate the visible fields and timezone name before converting the offset.
unsafe fn mktime_fields(
    record: *const usize,
    original: *const usize,
    zone_bytes: Option<&mut Vec<u8>>,
) -> Result<libc::tm, Error> {
    let mut parts = [0; 9];
    for (i, part) in parts.iter_mut().enumerate() {
        // SAFETY: all nine slots remain rooted while validating the integer arguments.
        *part = c_component(unsafe { *record.add(i) })?;
    }
    let [
        year,
        month,
        day,
        hour,
        minute,
        second,
        _weekday,
        yearday,
        isdst,
    ] = parts;
    // SAFETY: zero initializes every numeric field and any timezone pointer.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year
        .checked_sub(1900)
        .ok_or_else(|| Error::new(Kind::OverflowError, "year out of range"))?;
    tm.tm_mon = month.wrapping_sub(1);
    tm.tm_mday = day;
    tm.tm_hour = hour;
    tm.tm_min = minute;
    tm.tm_sec = second;
    tm.tm_yday = yearday.wrapping_sub(1);
    tm.tm_isdst = isdst;
    // The extended tuple may contain caller-supplied attributes. CPython
    // validates them even though POSIX mktime determines the timezone itself.
    // SAFETY: original remains rooted with its own physical descriptor.
    if unsafe { descriptor(original).has_time_zone() } {
        // SAFETY: the descriptor marks the two hidden attributes.
        unsafe {
            let zone = *original.add(9);
            if zone != 0 {
                let text = match extra_kind(original, 9).base() {
                    3 => Some(crate::StrOwner::acquire_word(zone)),
                    10 => super::string_enums::string_value(zone),
                    _ => None,
                }
                .ok_or_else(|| {
                    Error::new(Kind::TypeError, "bad argument type for built-in operation")
                })?;
                if let Some(bytes) = zone_bytes {
                    bytes.extend_from_slice(text.as_bytes());
                    bytes.push(0);
                    tm.tm_zone = bytes.as_mut_ptr().cast();
                }
            }
        }
    }
    Ok(tm)
}

/// Read an offset for Python index conversion after the preceding fields have passed validation.
///
/// # Safety
/// Both tuples are rooted. The original is an extended struct_time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_mktime_offset(record: *const usize, original: *const usize) -> usize {
    // SAFETY: the tuple layouts and ownership are supplied by the caller.
    let result =
        unsafe { mktime_fields(record, original, None).map(|_| imre_time_extra(original, false)) };
    finish(result, 0)
}

/// Convert a rooted time tuple using an optional preconverted offset word.
unsafe fn mktime(record: *const usize, original: *const usize, indexed: usize) -> f64 {
    let result = (|| {
        let mut zone_bytes = Vec::new();
        // SAFETY: the caller roots both tuples and the zone buffer outlives the system call.
        let mut tm = unsafe { mktime_fields(record, original, Some(&mut zone_bytes)) }?;
        // SAFETY: the descriptor identifies the two additional words of a struct_time.
        unsafe {
            if descriptor(original).has_time_zone() {
                let (offset, kind) = if indexed != 0 {
                    (indexed, 2)
                } else {
                    (*original.add(10), extra_kind(original, 10).base())
                };
                if offset != 0 {
                    tm.tm_gmtoff = match kind {
                        1 => (offset - 1) as libc::c_long,
                        2 => num_traits::ToPrimitive::to_i64(&*Int::borrow(offset).big())
                            .and_then(|n| libc::c_long::try_from(n).ok())
                            .ok_or_else(|| {
                                Error::new(
                                    Kind::OverflowError,
                                    "Python int too large to convert to C long",
                                )
                            })?,
                        _ => {
                            let name = match kind {
                                3 => "str",
                                7 => "float",
                                8 => "bytes",
                                6 => "tuple",
                                4 => "list",
                                _ => "object",
                            };
                            return Err(Error::new(
                                Kind::TypeError,
                                format!("'{name}' object cannot be interpreted as an integer"),
                            ));
                        }
                    };
                }
            }
        }
        tm.tm_wday = -1;
        // SAFETY: mktime owns no pointers from this initialized tm. On success it
        // writes the normalized weekday, even when the timestamp is exactly -1.
        let stamp = unsafe { libc::mktime(&mut tm) };
        if stamp == -1 && tm.tm_wday == -1 {
            Err(Error::new(
                Kind::OverflowError,
                "mktime argument out of range",
            ))
        } else {
            Ok(stamp as f64)
        }
    })();
    finish(result, -1.0)
}

/// # Safety
/// The rooted civil tuple has nine integers. The original tuple supplies its timezone attributes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_mktime(record: *const usize, original: *const usize) -> f64 {
    // SAFETY: both arguments satisfy the shared conversion's tuple contract.
    unsafe { mktime(record, original, 0) }
}

/// # Safety
/// Both tuples and the tagged integer produced by the Python index protocol are rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_mktime_indexed(
    record: *const usize,
    original: *const usize,
    offset: usize,
) -> f64 {
    // SAFETY: all three arguments are rooted throughout conversion.
    unsafe { mktime(record, original, offset) }
}
