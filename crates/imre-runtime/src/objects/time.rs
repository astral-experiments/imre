//! Local civil time uses the platform timezone database and mktime normalization.

use super::{
    Int,
    calendar::c_component,
    exceptions::{Error, Kind, finish},
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
                record
                    .add(10)
                    .write(Int::from(tm.tm_gmtoff as i64).into_raw());
                record
            }
        }),
        std::ptr::null_mut(),
    )
}

/// # Safety
/// A rooted struct_time has two additional zero-initialized words for its named attributes.
pub(super) unsafe fn release_extras(record: *const usize) {
    // SAFETY: the final record owner transfers the nullable string and integer.
    unsafe {
        WordKind::new(3).release(*record.add(9));
        WordKind::new(2).release(*record.add(10));
    }
}

/// # Safety
/// The compiler supplies a fresh time tuple and roots the optional attributes during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_time_extras(record: *mut usize, zone: usize, offset: usize) {
    // SAFETY: each acquired reference is transferred to one initially empty tail slot.
    unsafe {
        WordKind::new(3).retain(zone);
        WordKind::new(2).retain(offset);
        record.add(9).write(zone);
        record.add(10).write(offset);
    }
}

/// # Safety
/// The time tuple remains rooted while its selected optional attribute is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_time_extra(record: *const usize, zone: bool) -> usize {
    // SAFETY: the two hidden words use nullable string and integer representations.
    unsafe {
        let word = *record.add(if zone { 9 } else { 10 });
        WordKind::new(if zone { 3 } else { 2 }).retain(word);
        word
    }
}

/// Validate one __index__ result before invoking any later tuple field's protocol.
#[unsafe(no_mangle)]
pub extern "C" fn imre_time_component(value: usize) -> usize {
    finish(
        c_component(value).map(|n| Int::small(i64::from(n)).unwrap().into_raw()),
        0,
    )
}

/// # Safety
/// The argument is a rooted tuple with nine tagged integers in positional order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_mktime(record: *const usize) -> f64 {
    let result = (|| {
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
