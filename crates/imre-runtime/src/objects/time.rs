//! Local civil time uses the platform timezone database and mktime normalization.

use super::{
    Int,
    calendar::c_component,
    exceptions::{Error, Kind, finish},
    logging::timed,
    records::{RecordDescriptor, descriptor, imre_record_try_new},
    words::WordKind,
};
use std::ffi::CStr;

pub(crate) use super::stdlib::imre_wall_time as wall_time;

/// The nine sequence fields and two named timezone attributes of a civil time.
pub(crate) struct ClockParts {
    pub fields: [i64; 9],
    pub zone: Option<crate::StrOwner>,
    pub offset: i64,
}

impl ClockParts {
    pub(crate) fn from_float(stamp: f64, utc: bool) -> Result<Self, Error> {
        Self::civil(timed::float_seconds(stamp)?, utc)
    }

    pub(crate) fn from_int(stamp: &Int, utc: bool) -> Result<Self, Error> {
        Self::civil(stamp.to_i64().ok_or_else(timed::overflow)?, utc)
    }

    fn civil(seconds: i64, utc: bool) -> Result<Self, Error> {
        let tm = timed::civil(seconds, utc)?;
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
        let zone = (!tm.tm_zone.is_null()).then(|| {
            // SAFETY: libc returns a NUL-terminated zone name. Copy it before the next conversion.
            let name = unsafe { CStr::from_ptr(tm.tm_zone) }.to_string_lossy();
            crate::StrOwner::new(crate::Str::from(name.as_ref()))
        });
        Ok(Self {
            fields,
            zone,
            offset: tm.tm_gmtoff,
        })
    }
}

/// # Safety
/// The descriptor has the nine integer slots yielded by a Python struct_time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_clock_parts(
    stamp: f64,
    utc: bool,
    layout: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: the caller supplies the struct_time descriptor.
    unsafe { clock_parts(ClockParts::from_float(stamp, utc), layout) }
}

/// Convert an integer timestamp without rounding it to a floating-point second.
///
/// # Safety
/// The caller roots the tagged integer and supplies the struct_time descriptor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_clock_int_parts(
    stamp: usize,
    utc: bool,
    layout: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: the caller keeps the Python integer and tuple layout live.
    unsafe { clock_parts(ClockParts::from_int(&Int::borrow(stamp), utc), layout) }
}

/// Build a struct_time from checked seconds and the caller's nine-field descriptor.
unsafe fn clock_parts(
    parts: Result<ClockParts, Error>,
    layout: *const RecordDescriptor,
) -> *mut usize {
    finish(
        parts.map(|parts| {
            // SAFETY: the caller supplied the time tuple layout, with two zero-initialized hidden words.
            unsafe {
                let record = imre_record_try_new(layout);
                if record.is_null() {
                    return record;
                }
                for (index, value) in parts.fields.into_iter().enumerate() {
                    record
                        .add(index)
                        .write(Int::small(value).unwrap().into_raw());
                }
                if let Some(zone) = parts.zone {
                    record
                        .add(9)
                        .write(crate::StrOwner::into_raw(zone) as usize);
                }
                record.add(10).write(Int::from(parts.offset).into_raw());
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

/// Local civil time and the backing storage for its optional POSIX timezone name.
pub(crate) struct LocalTime {
    tm: libc::tm,
    zone: Vec<u8>,
}

impl LocalTime {
    pub(crate) fn new(parts: [libc::c_int; 9]) -> Result<Self, Error> {
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
        // SAFETY: zero initializes every numeric field and the timezone pointer.
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
        Ok(Self {
            tm,
            zone: Vec::new(),
        })
    }

    pub(crate) fn set_zone(&mut self, name: &str) {
        self.zone.clear();
        self.zone.extend_from_slice(name.as_bytes());
        self.zone.push(0);
    }

    pub(crate) fn set_offset(&mut self, offset: &Int) -> Result<(), Error> {
        self.tm.tm_gmtoff = offset
            .to_i64()
            .and_then(|n| libc::c_long::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    Kind::OverflowError,
                    "Python int too large to convert to C long",
                )
            })?;
        Ok(())
    }

    pub(crate) fn timestamp(mut self) -> Result<f64, Error> {
        if !self.zone.is_empty() {
            self.tm.tm_zone = self.zone.as_mut_ptr().cast();
        }
        self.tm.tm_wday = -1;
        // SAFETY: the zone buffer lives through mktime. Success sets the normalized
        // weekday even for the valid timestamp -1.
        let stamp = unsafe { libc::mktime(&mut self.tm) };
        if stamp == -1 && self.tm.tm_wday == -1 {
            Err(Error::new(
                Kind::OverflowError,
                "mktime argument out of range",
            ))
        } else {
            Ok(stamp as f64)
        }
    }
}

/// Validate the visible fields and timezone name before converting the offset.
unsafe fn mktime_fields(
    record: *const usize,
    original: *const usize,
    copy_zone: bool,
) -> Result<LocalTime, Error> {
    let mut parts = [0; 9];
    for (i, part) in parts.iter_mut().enumerate() {
        // SAFETY: all nine slots remain rooted while validating the integer arguments.
        *part = c_component(unsafe { *record.add(i) })?;
    }
    let mut local = LocalTime::new(parts)?;
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
                if copy_zone {
                    local.set_zone(text.as_str());
                }
            }
        }
    }
    Ok(local)
}

/// Read an offset for Python index conversion after the preceding fields have passed validation.
///
/// # Safety
/// Both tuples are rooted. The original is an extended struct_time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_mktime_offset(record: *const usize, original: *const usize) -> usize {
    // SAFETY: the tuple layouts and ownership are supplied by the caller.
    let result =
        unsafe { mktime_fields(record, original, false).map(|_| imre_time_extra(original, false)) };
    finish(result, 0)
}

/// Convert a rooted time tuple using an optional preconverted offset word.
unsafe fn mktime(record: *const usize, original: *const usize, indexed: usize) -> f64 {
    let result = (|| {
        // SAFETY: the caller roots both tuples throughout conversion.
        let mut local = unsafe { mktime_fields(record, original, true) }?;
        // SAFETY: the descriptor identifies the two additional words of a struct_time.
        unsafe {
            if descriptor(original).has_time_zone() {
                let (offset, kind) = if indexed != 0 {
                    (indexed, 2)
                } else {
                    (*original.add(10), extra_kind(original, 10).base())
                };
                if offset != 0 {
                    match kind {
                        1 => local.tm.tm_gmtoff = (offset - 1) as libc::c_long,
                        2 => local.set_offset(&Int::borrow(offset))?,
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
        local.timestamp()
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
