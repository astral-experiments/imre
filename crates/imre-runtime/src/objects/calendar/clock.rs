//! Time-of-day storage shared by typed logging and runtime-loaded datetime.
use super::*;

pub(in crate::objects) fn clock_parts(clock: &Object) -> [i64; 5] {
    with(clock, |v| match v {
        Value::Time { local, fold, .. } => time_fields(*local, *fold).map(|n| n as i64),
        _ => unreachable!(),
    })
}

pub(super) fn order(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    let normalized = |v: &Value| match v {
        Value::Time { local, zone, .. } => {
            // CPython compares whole offset seconds, then the clock's own microseconds.
            let off = zone
                .as_ref()
                .map(|z| offset(z).div_euclid(1_000_000) * 1_000_000);
            (local - off.unwrap_or(0), off.is_some())
        }
        _ => unreachable!(),
    };
    let (a, aware) = normalized(left);
    let (b, other_aware) = normalized(right);
    (aware == other_aware).then(|| a.cmp(&b))
}

/// # Safety
/// The typed caller roots five integer fields and supplies None for tzinfo.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_clock_new(
    h: usize,
    m: usize,
    s: usize,
    us: usize,
    _tzinfo: u8,
    fold: usize,
) -> *const Object {
    rc_result((|| {
        let [h, m, s, us, fold] = [h, m, s, us, fold].map(c_component);
        let parts = [h?, m?, s?, us?, fold?];
        try_owner(Value::Time {
            local: time_micros(parts)?,
            fold: parts[4],
            zone: None,
        })
    })())
}

/// # Safety
/// The caller roots the clock and supplies an immediate field index in 0..5.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_clock_part(clock: *const Object, index: usize) -> usize {
    // SAFETY: the typed caller guarantees both exact inputs and the index range.
    let (clock, index) = unsafe {
        (
            object(clock),
            Int::borrow(index).as_small().unwrap() as usize,
        )
    };
    Int::small(clock_parts(clock)[index]).unwrap().into_raw()
}

/// # Safety
/// The typed caller roots two exact naive times.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_clock_compare(a: *const Object, b: *const Object) -> i32 {
    // SAFETY: the typed comparison roots both clocks for these immutable reads.
    unsafe {
        with(object(a), |a| {
            with(object(b), |b| order(a, b).unwrap() as i32)
        })
    }
}
