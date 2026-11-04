//! Month grids own four to six ordinary, independently mutable week lists.
use super::*;
use crate::Int;
use crate::objects::words::WordKind;
use std::sync::atomic::{AtomicU8, Ordering};

// The public module setter updates this state for every subsequent grid.
static FIRST_WEEKDAY: AtomicU8 = AtomicU8::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn imre_calendar_firstweekday() -> usize {
    Int::from(i64::from(FIRST_WEEKDAY.load(Ordering::Relaxed))).into_raw()
}

fn argument_error(kind: exceptions::Kind, message: String, value: &Int) -> Error {
    // SAFETY: the caller keeps the integer rooted while the exception acquires its argument.
    Error::object_argument(kind, message, unsafe { WordKind::new(2).acquire(value.0) })
}

/// # Safety
/// The caller keeps the exact integer argument live throughout the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_setfirstweekday(value: usize) -> u8 {
    // SAFETY: the checked caller supplies a rooted Python integer.
    let value = unsafe { Int::borrow(value) };
    if let Some(day @ 0..=6) = value.to_i64() {
        FIRST_WEEKDAY.store(day as u8, Ordering::Relaxed);
    } else {
        argument_error(
            exceptions::Kind::IllegalWeekdayError,
            format!(
                "bad weekday number {}; must be 0 (Monday) to 6 (Sunday)",
                *value
            ),
            &value,
        )
        .raise();
    }
    0
}

fn month_grid(year: &Int, month: &Int) -> exceptions::Result<*const Object> {
    let Some(month @ 1..=12) = month.to_i64() else {
        return Err(argument_error(
            exceptions::Kind::IllegalMonthError,
            format!("bad month number {month}; must be 1-12"),
            month,
        ));
    };
    // Each Gregorian cycle has 146097 days, a whole number of weeks. Reduce
    // even unbounded Python years before entering the ordinary date arithmetic.
    let cycle_year = year.to_i64().map_or_else(
        || {
            (year.big().as_ref() % 400_i32)
                .to_i32()
                .unwrap()
                .rem_euclid(400)
        },
        |year| year.rem_euclid(400) as i32,
    );
    let year = 2000 + cycle_year;
    let month = month as i32;
    let first = ((ordinal(year, month, 1) - 1) % 7) as i32;
    let offset = (first - i32::from(FIRST_WEEKDAY.load(Ordering::Relaxed))).rem_euclid(7);
    let days = month_days(year, month);
    let mut rows = Words::new(4);
    rows.values
        .try_reserve_exact(((offset + days + 6) / 7) as usize)
        .map_err(|_| Error::memory_error())?;
    for start in (1 - offset..=days).step_by(7) {
        let mut values = Vec::new();
        values
            .try_reserve_exact(7)
            .map_err(|_| Error::memory_error())?;
        values.extend(
            (start..start + 7)
                .map(|day| Int::from(i64::from(if (1..=days).contains(&day) { day } else { 0 }))),
        );
        let week = crate::ListOwner::try_new(crate::List::new(values))
            .map_err(|_| Error::memory_error())?;
        rows.values.push(crate::ListOwner::into_raw(week) as usize);
    }
    Ok(crate::ObjectOwner::into_raw(rows.into_object()))
}

/// # Safety
/// Both words are exact integers rooted by the checked caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_month_grid(year: usize, month: usize) -> *const Object {
    // SAFETY: both inputs remain owned by the caller until the result is returned.
    let (year, month) = unsafe { (Int::borrow(year), Int::borrow(month)) };
    finish(month_grid(&year, &month), std::ptr::null())
}
