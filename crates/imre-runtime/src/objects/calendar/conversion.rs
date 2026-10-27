//! Civil field replacement and conversion to an explicit fixed-offset timezone.
use super::*;
use crate::objects::logging::timed;

/// # Safety
/// The receiver and supplied zone are rooted. A zero numeric word denotes an
/// omitted field and every present numeric word is a rooted Python integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_replace(
    dt: *const Object,
    y: usize,
    m: usize,
    d: usize,
    h: usize,
    mi: usize,
    s: usize,
    u: usize,
    zone: usize,
    fold: usize,
    timezone_supplied: bool,
) -> *const Object {
    // SAFETY: the compiler roots the original datetime throughout field replacement.
    let dt = unsafe { object(dt) };
    rc_result(with(dt, |value| {
        let Value::Date {
            local,
            zone: previous,
            fold: old_fold,
        } = value
        else {
            unreachable!()
        };
        let mut fields = date_fields(*local, *old_fold).map(|field| field as i32);
        for (field, word) in fields.iter_mut().zip([y, m, d, h, mi, s, u, fold]) {
            if word != 0 {
                // replace uses CPython's C-int converter, whose overflow message
                // differs from the datetime constructor's argument parser.
                // SAFETY: nonzero component words are rooted Python integers.
                *field = unsafe { Int::borrow(word) }
                    .as_small()
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| {
                        Error::new(
                            Kind::OverflowError,
                            "Python int too large to convert to C int",
                        )
                    })?;
            }
        }
        let zone = if timezone_supplied {
            // SAFETY: the generated call keeps an explicitly supplied fixed zone rooted.
            (zone != 0).then(|| unsafe { retain(zone as *const Object) })
        } else {
            previous.clone()
        };
        datetime_from_fields(fields, zone)
    }))
}

/// Local wall-clock seconds at an epoch timestamp. CPython checks the calendar
/// year even for the temporary probes used to resolve a gap or repeated hour.
fn local_seconds(utc: i64) -> Result<i64, Error> {
    let t = timed::civil(utc, false)?;
    let year = t.tm_year.checked_add(1900).ok_or_else(timed::overflow)?;
    check(year, "year", 1, 9999)?;
    let days = (ordinal(year, t.tm_mon + 1, t.tm_mday) - EPOCH_ORD) as i64;
    Ok(days * 86_400 + i64::from((t.tm_hour * 60 + t.tm_min) * 60 + t.tm_sec))
}

/// Detect the second occurrence of a repeated local time, as in datetime.fromtimestamp.
pub(super) fn local_fold(seconds: i64) -> Result<i32, Error> {
    const DAY_SECONDS: i64 = 86_400;
    if cfg!(windows) && seconds <= DAY_SECONDS {
        return Ok(0);
    }
    let wall = local_seconds(seconds)?;
    let previous = seconds
        .checked_sub(DAY_SECONDS)
        .ok_or_else(timed::overflow)?;
    let transition = wall - local_seconds(previous)? - DAY_SECONDS;
    Ok(i32::from(
        transition < 0 && local_seconds(seconds + transition)? == wall,
    ))
}

/// Solve local_seconds(utc) == wall using the two offsets bracketing a clock
/// transition, as in CPython's local_to_seconds.
fn local_candidate(wall: i64, fold: bool) -> Result<i64, Error> {
    let a = local_seconds(wall)? - wall;
    let first = wall - a;
    let first_local = local_seconds(first)?;
    let b = if first_local == wall {
        let probe = if fold { first + 86_400 } else { first - 86_400 };
        let alternate_offset = local_seconds(probe)? - probe;
        if alternate_offset == a {
            return Ok(first);
        }
        alternate_offset
    } else {
        first_local - first
    };
    let second = wall - b;
    if local_seconds(second)? == wall {
        return Ok(second);
    }
    if first_local == wall {
        return Ok(first);
    }
    Ok(if fold {
        first.min(second)
    } else {
        first.max(second)
    })
}

fn naive_offset(local: i128, fold: bool) -> Result<i128, Error> {
    let wall = ((local - (EPOCH_ORD - 1) * DAY).div_euclid(1_000_000)) as i64;
    let first = local_candidate(wall, fold)?;
    let other = local_candidate(wall, !fold)?;
    // For a gap, astimezone selects the offset on the opposite side from timestamp().
    let utc = if first != other && (other > first) == fold {
        other
    } else {
        first
    };
    Ok(i128::from(timed::civil(utc, false)?.tm_gmtoff) * 1_000_000)
}

/// # Safety
/// The receiver is a rooted datetime and the target a rooted fixed timezone.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_astimezone(
    dt: *const Object,
    target: *const Object,
) -> *const Object {
    // SAFETY: both handles remain rooted throughout conversion and any returned alias is retained.
    let (source, target_ref) = unsafe { (object(dt), object(target)) };
    rc_result(with(source, |value| {
        let Value::Date { local, zone, fold } = value else {
            unreachable!()
        };
        if zone
            .as_ref()
            .is_some_and(|z| crate::ObjectOwner::as_ptr(z) == target)
        {
            // SAFETY: a same-zone conversion returns a new owner of the original datetime.
            return Ok(unsafe { retain(dt) });
        }
        let source_offset = match zone {
            Some(zone) => offset(zone),
            None => naive_offset(*local, *fold != 0)?,
        };
        let utc = local - source_offset;
        check_datetime_range(utc)?;
        // SAFETY: the target stays alive until the new datetime acquires its reference.
        date(utc + offset(target_ref), Some(unsafe { retain(target) }))
    }))
}
