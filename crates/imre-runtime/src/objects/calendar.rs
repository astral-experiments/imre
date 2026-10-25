//! Immutable Python calendar values. Civil dates use an ordinal and microseconds of day;
//! fixed zones keep their original offset and owner, including the canonical UTC singleton.
use super::*;
/// # Safety
/// The rooted receiver is an exact timedelta.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_seconds(d: *const Object) -> f64 {
    // SAFETY: the caller roots the exact timedelta until the conversion returns.
    let n = delta(unsafe { object(d) });
    if n == 0 {
        return 0.0;
    }
    // Round the exact integer / 1,000,000 ratio only once, even for long durations.
    let v = n.unsigned_abs();
    let den = 1_000_000u128;
    let mut e = v.ilog2() as i32 - den.ilog2() as i32;
    if if e >= 0 {
        v < den << e
    } else {
        (v << (-e)) < den
    } {
        e -= 1;
    }
    let shift = 52 - e;
    let scaled = v << shift;
    let q = scaled / den;
    let rem = scaled % den;
    let rounded = q + u128::from(2 * rem > den || (2 * rem == den && q % 2 == 1));
    (if n < 0 { -1.0 } else { 1.0 }) * (rounded as f64) * 2.0f64.powi(-shift)
}
/// # Safety
/// The fixed timezone owns the returned offset object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_zoneoffset(z: *const Object) -> *const Object {
    // SAFETY: the caller roots the zone that owns the returned offset.
    with(unsafe { object(z) }, |v| match v {
        Value::Zone { offset, .. } => crate::ObjectOwner::into_raw(offset.clone()),
        _ => unreachable!(),
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn imre_calendar_zone_limit(max: bool) -> *const Object {
    thread_local! {
        static MIN:crate::ObjectOwner=owner(Value::Zone{offset:owner(Value::Delta(-86_340_000_000)),name:None});
        static MAX:crate::ObjectOwner=owner(Value::Zone{offset:owner(Value::Delta(86_340_000_000)),name:None});
    }
    crate::ObjectOwner::into_raw(if max {
        MAX.with(crate::ObjectOwner::clone)
    } else {
        MIN.with(crate::ObjectOwner::clone)
    })
}
/// # Safety
/// The rooted objects have the same exact calendar class. Naive/aware ordering raises.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_compare(
    a: *const Object,
    b: *const Object,
    equality: bool,
) -> i32 {
    // SAFETY: both calendar operands remain rooted throughout the comparison.
    let (a, b) = unsafe { (object(a), object(b)) };
    let res = with(a, |a| {
        with(b, |b| {
            let ord = match (a, b) {
                (Value::Delta(a), Value::Delta(b)) => a.cmp(b),
                (Value::Zone { offset: a, .. }, Value::Zone { offset: b, .. }) => {
                    delta(a).cmp(&delta(b))
                }
                (
                    Value::Date {
                        local: a, zone: az, ..
                    },
                    Value::Date {
                        local: b, zone: bz, ..
                    },
                ) => match (az, bz) {
                    (None, None) => a.cmp(b),
                    (Some(az), Some(bz)) => (a - offset(az)).cmp(&(b - offset(bz))),
                    _ if equality => return Ok(1),
                    _ => {
                        return Err(Error::new(
                            Kind::TypeError,
                            "can't compare offset-naive and offset-aware datetimes",
                        ));
                    }
                },
                _ => unreachable!(),
            };
            Ok(ord as i32)
        })
    });
    finish(res, 0)
}

use num_bigint::BigInt;
use num_traits::{FromPrimitive, ToPrimitive};
use std::time::{SystemTime, UNIX_EPOCH};

const DAY: i128 = 86_400_000_000;
const MAX_ORD: i128 = 3_652_059;
const EPOCH_ORD: i128 = 719_163;
const FACTORS: [i64; 7] = [
    86_400_000_000,
    1_000_000,
    1,
    1_000,
    60_000_000,
    3_600_000_000,
    604_800_000_000,
];

pub(super) enum Value {
    Delta(i128),
    // The value is local civil microseconds since ordinal 1. Both naive and aware datetimes
    // use the same calendar; the fixed offset matters for instant comparison, not day addition.
    Date {
        local: i128,
        zone: Option<crate::ObjectOwner>,
        fold: i32,
    },
    Zone {
        offset: crate::ObjectOwner,
        name: Option<crate::StrOwner>,
    },
}

fn owner(value: Value) -> crate::ObjectOwner {
    crate::ObjectOwner::new(Object(StorageCell::new(Storage::Calendar(Box::new(value)))))
}

fn with<T>(obj: &Object, f: impl FnOnce(&Value) -> T) -> T {
    // SAFETY: calendar storage is immutable after creation, including during nested reads.
    unsafe {
        obj.0.with(|storage| match storage {
            Storage::Calendar(value) => f(value),
            _ => unreachable!("compiler admitted an exact calendar value"),
        })
    }
}

/// # Safety
/// The caller roots an immutable timedelta.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_truth(o: *const Object) -> bool {
    // SAFETY: the caller roots the exact timedelta.
    delta(unsafe { object(o) }) != 0
}
fn delta(obj: &Object) -> i128 {
    with(obj, |v| match v {
        Value::Delta(us) => *us,
        _ => unreachable!(),
    })
}

fn offset(zone: &Object) -> i128 {
    with(zone, |v| match v {
        Value::Zone { offset, .. } => delta(offset),
        _ => unreachable!(),
    })
}

thread_local! {
    static UTC: crate::ObjectOwner = owner(Value::Zone { offset:owner(Value::Delta(0)), name:None });
}
fn utc() -> crate::ObjectOwner {
    UTC.with(crate::ObjectOwner::clone)
}

fn validate_delta(value: &BigInt) -> Result<i128, Error> {
    let unit = BigInt::from(DAY);
    let rem = ((value % &unit) + &unit) % &unit;
    let days = (value - rem) / &unit;
    let count = days.to_i32().ok_or_else(|| {
        Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C int",
        )
    })?;
    if !(-999_999_999..=999_999_999).contains(&count) {
        return Err(Error::new(
            Kind::OverflowError,
            format!("days={count}; must have magnitude <= 999999999"),
        ));
    }
    Ok(value.to_i128().expect("validated duration magnitude"))
}

// The C constructor accumulates integer units exactly, carries the integral portion of
// each floating unit and rounds their combined fractional microseconds once, ties to even.
fn make_delta(mask: u32, words: [usize; 7]) -> Result<crate::ObjectOwner, Error> {
    let mut total = BigInt::from(0);
    let mut fraction = 0.0;
    for i in [2, 3, 1, 4, 5, 0, 6] {
        let word = words[i];
        let factor = FACTORS[i];
        if mask & (1 << i) == 0 {
            // SAFETY: the caller roots each tagged integer over this operation.
            total += unsafe { Int::borrow(word) }.big().as_ref() * factor;
        } else {
            let n = f64::from_bits(word as u64);
            if !n.is_finite() {
                return Err(if n.is_nan() {
                    Error::new(Kind::ValueError, "cannot convert float NaN to integer")
                } else {
                    Error::new(
                        Kind::OverflowError,
                        "cannot convert float infinity to integer",
                    )
                });
            }
            total += BigInt::from_f64(n.trunc()).unwrap() * factor;
            let frac = n.fract() * (factor as f64);
            total += BigInt::from_f64(frac.trunc()).unwrap();
            fraction += frac.fract();
        }
    }
    // Rounding an exact half depends on the parity of the accumulated whole part.
    let near = fraction.round_ties_even() as i64;
    let correction =
        if (fraction - near as f64).abs() == 0.5 && (&total & BigInt::from(1)) != BigInt::from(0) {
            if fraction > near as f64 { 1 } else { -1 }
        } else {
            0
        };
    total += near + correction;
    Ok(owner(Value::Delta(validate_delta(&total)?)))
}

fn parts(value: i128) -> (i128, i128, i128) {
    (
        value.div_euclid(DAY),
        value.rem_euclid(DAY) / 1_000_000,
        value.rem_euclid(1_000_000),
    )
}

pub(super) fn write_repr(v: &Value, out: &mut String) {
    match v {
        Value::Delta(n) => out.push_str(&delta_repr(*n)),
        Value::Zone { offset, name } => {
            if delta(offset) == 0 && name.is_none() {
                out.push_str("datetime.timezone.utc");
                return;
            }
            out.push_str("datetime.timezone(");
            out.push_str(&delta_repr(delta(offset)));
            if let Some(s) = name {
                out.push_str(", ");
                exceptions::push_string_repr(s.as_str(), out);
            }
            out.push(')');
        }
        Value::Date { local, zone, fold } => {
            let [y, m, d, h, mi, s, u, _] = date_fields(*local, *fold);
            out.push_str(&format!("datetime.datetime({y}, {m}, {d}, {h}, {mi}"));
            if s != 0 || u != 0 {
                out.push_str(&format!(", {s}"));
            }
            if u != 0 {
                out.push_str(&format!(", {u}"));
            }
            if *fold != 0 {
                out.push_str(", fold=1");
            }
            if let Some(z) = zone {
                out.push_str(", tzinfo=");
                with(z, |v| write_repr(v, out));
            }
            out.push(')');
        }
    }
}
/// # Safety
/// Each nonzero word denotes a rooted calendar object; zero denotes Python None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_equal_optional(a: usize, b: usize) -> bool {
    if a == b {
        return true;
    }
    if a == 0 || b == 0 {
        return false;
    }
    // SAFETY: zero was excluded; both calendar values remain rooted for comparison.
    let (a, b) = unsafe { (object(a as *const Object), object(b as *const Object)) };
    with(a, |l| with(b, |r| equal(l, r)))
}
pub(super) fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Delta(a), Value::Delta(b)) => a == b,
        (Value::Zone { offset: a, .. }, Value::Zone { offset: b, .. }) => delta(a) == delta(b),
        (
            Value::Date {
                local: a, zone: az, ..
            },
            Value::Date {
                local: b, zone: bz, ..
            },
        ) => match (az, bz) {
            (None, None) => a == b,
            (Some(x), Some(y)) => a - offset(x) == b - offset(y),
            _ => false,
        },
        _ => false,
    }
}

fn delta_repr(value: i128) -> String {
    let (d, s, u) = parts(value);
    let args = [("days", d), ("seconds", s), ("microseconds", u)]
        .into_iter()
        .filter(|(_, n)| *n != 0)
        .map(|(s, n)| format!("{s}={n}"))
        .collect::<Vec<_>>();
    format!(
        "datetime.timedelta({})",
        if args.is_empty() {
            "0".to_owned()
        } else {
            args.join(", ")
        }
    )
}

fn zone_name(offset: i128) -> String {
    if offset == 0 {
        return "UTC".into();
    }
    format!("UTC{}", offset_text(offset))
}
fn offset_text(offset: i128) -> String {
    let t = offset.abs();
    let mut s = format!(
        "{}{:02}:{:02}",
        if offset < 0 { '-' } else { '+' },
        t / 3_600_000_000,
        t / 60_000_000 % 60
    );
    if t % 60_000_000 != 0 {
        s.push_str(&format!(":{:02}", t / 1_000_000 % 60));
    }
    if t % 1_000_000 != 0 {
        s.push_str(&format!(".{:06}", t % 1_000_000));
    }
    s
}
fn make_zone(
    delta_obj: crate::ObjectOwner,
    name: Option<crate::StrOwner>,
) -> Result<crate::ObjectOwner, Error> {
    let us = delta(&delta_obj);
    if us.abs() >= DAY {
        return Err(Error::new(
            Kind::ValueError,
            format!(
                "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), not {}",
                delta_repr(us)
            ),
        ));
    }
    if us == 0 && name.is_none() {
        return Ok(utc());
    }
    Ok(owner(Value::Zone {
        offset: delta_obj,
        name,
    }))
}

fn leap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}
fn days_before_year(y: i32) -> i32 {
    let n = y - 1;
    365 * n + n / 4 - n / 100 + n / 400
}
fn month_days(y: i32, m: i32) -> i32 {
    match m {
        2 => {
            if leap(y) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
fn ordinal(y: i32, m: i32, d: i32) -> i128 {
    i128::from(days_before_year(y) + (1..m).map(|q| month_days(y, q)).sum::<i32>() + d)
}
fn ymd(ord: i128) -> (i32, i32, i32) {
    let (mut low, mut high) = (1, 10000);
    while low + 1 < high {
        let mid = (low + high) / 2;
        if i128::from(days_before_year(mid)) < ord {
            low = mid;
        } else {
            high = mid;
        }
    }
    let mut day = ord as i32 - days_before_year(low);
    let mut month = 1;
    while day > month_days(low, month) {
        day -= month_days(low, month);
        month += 1;
    }
    (low, month, day)
}
fn c_component(word: usize) -> Result<i32, Error> {
    // SAFETY: the frontend accepts/roots only exact tagged integers and bools converted to int.
    let value = unsafe { Int::borrow(word) };
    let v = value
        .big()
        .to_i64()
        .filter(|n| std::mem::size_of::<libc::c_long>() == 8 || i32::try_from(*n).is_ok())
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C long",
            )
        })?;
    i32::try_from(v).map_err(|_| {
        Error::new(
            Kind::OverflowError,
            if v < 0 {
                "signed integer is less than minimum"
            } else {
                "signed integer is greater than maximum"
            },
        )
    })
}
fn check(value: i32, name: &str, low: i32, high: i32) -> Result<(), Error> {
    if (low..=high).contains(&value) {
        Ok(())
    } else {
        Err(Error::new(
            Kind::ValueError,
            format!("{name} must be in {low}..{high}, not {value}"),
        ))
    }
}
fn new_date(
    words: [usize; 8],
    zone: Option<crate::ObjectOwner>,
) -> Result<crate::ObjectOwner, Error> {
    let [y, m, d, h, mn, s, u, fold] = words.map(c_component);
    let (y, m, d, h, mn, s, u, fold) = (y?, m?, d?, h?, mn?, s?, u?, fold?);
    check(y, "year", 1, 9999)?;
    check(m, "month", 1, 12)?;
    let max = month_days(y, m);
    if !(1..=max).contains(&d) {
        return Err(Error::new(
            Kind::ValueError,
            format!("day {d} must be in range 1..{max} for month {m} in year {y}"),
        ));
    }
    check(h, "hour", 0, 23)?;
    check(mn, "minute", 0, 59)?;
    check(s, "second", 0, 59)?;
    check(u, "microsecond", 0, 999_999)?;
    if !(0..=1).contains(&fold) {
        return Err(Error::new(
            Kind::ValueError,
            format!("fold must be either 0 or 1, not {fold}"),
        ));
    }
    let local = (ordinal(y, m, d) - 1) * DAY
        + i128::from(h * 3600 + mn * 60 + s) * 1_000_000
        + i128::from(u);
    Ok(owner(Value::Date { local, zone, fold }))
}
fn date(local: i128, zone: Option<crate::ObjectOwner>) -> Result<crate::ObjectOwner, Error> {
    if !(0..MAX_ORD * DAY).contains(&local) {
        return Err(Error::new(Kind::OverflowError, "date value out of range"));
    }
    Ok(owner(Value::Date {
        local,
        zone,
        fold: 0,
    }))
}
fn date_fields(local: i128, fold: i32) -> [i128; 8] {
    let ord = local.div_euclid(DAY) + 1;
    let (y, m, d) = ymd(ord);
    let tod = local.rem_euclid(DAY);
    [
        y.into(),
        m.into(),
        d.into(),
        tod / 3_600_000_000,
        tod / 60_000_000 % 60,
        tod / 1_000_000 % 60,
        tod % 1_000_000,
        fold.into(),
    ]
}
fn date_text(local: i128, zone: &Option<crate::ObjectOwner>, sep: char) -> String {
    let [y, m, d, h, mi, s, u, _] = date_fields(local, 0);
    let mut text = format!("{y:04}-{m:02}-{d:02}{sep}{h:02}:{mi:02}:{s:02}");
    if u != 0 {
        text.push_str(&format!(".{u:06}"));
    }
    if let Some(z) = zone {
        text.push_str(&offset_text(offset(z)));
    }
    text
}

/// Makes a separately owned reference from a Python value retained by its caller.
///
/// # Safety
/// `p` must point to a live native object.
unsafe fn retain(p: *const Object) -> crate::ObjectOwner {
    // SAFETY: preserve the caller's strong reference when materializing the new owner.
    unsafe { crate::ObjectOwner::acquire_word(p as usize) }
}

fn rc_result(result: Result<crate::ObjectOwner, Error>) -> *const Object {
    finish(result.map(crate::ObjectOwner::into_raw), std::ptr::null())
}

/// # Safety
/// Non-float words are rooted Python integers; float words preserve the original binary64 bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_delta(
    mask: u32,
    d: usize,
    s: usize,
    u: usize,
    ms: usize,
    m: usize,
    h: usize,
    w: usize,
) -> *const Object {
    rc_result(make_delta(mask, [d, s, u, ms, m, h, w]))
}
/// # Safety
/// The incoming delta and optional string are rooted for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_zone(d: *const Object, name: usize) -> *const Object {
    // SAFETY: the caller roots the delta and any present name until the new zone owns both.
    let (d, name) = unsafe {
        (
            retain(d),
            (name != 0).then(|| crate::StrOwner::acquire_word(name)),
        )
    };
    rc_result(make_zone(d, name))
}
#[unsafe(no_mangle)]
pub extern "C" fn imre_calendar_utc() -> *const Object {
    crate::ObjectOwner::into_raw(utc())
}
/// # Safety
/// Integer arguments and any nonzero optional zone remain rooted until the constructor returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_datetime(
    y: usize,
    m: usize,
    d: usize,
    h: usize,
    mi: usize,
    s: usize,
    u: usize,
    z: usize,
    f: usize,
) -> *const Object {
    // SAFETY: a present zone remains rooted until the new datetime owns it.
    let zone = (z != 0).then(|| unsafe { retain(z as *const Object) });
    rc_result(new_date([y, m, d, h, mi, s, u, f], zone))
}
/// # Safety
/// The argument is a rooted exact fixed timezone, retained by the resulting datetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_now(z: *const Object) -> *const Object {
    // SAFETY: the caller roots this nonnull fixed zone until the new datetime owns it.
    let zone = unsafe { retain(z) };
    let epoch_us = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_micros() as i128,
        Err(e) => (-(e.duration().as_nanos() as i128)).div_euclid(1000),
    };
    rc_result(date(
        (EPOCH_ORD - 1) * DAY + epoch_us + offset(&zone),
        Some(zone),
    ))
}
/// # Safety
/// The first argument is a rooted datetime and the second a rooted timedelta.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_shift(
    dt: *const Object,
    d: *const Object,
    sub: bool,
) -> *const Object {
    // SAFETY: both typed operands remain rooted until the new datetime is built.
    let (dt, d) = unsafe { (object(dt), object(d)) };
    let n = delta(d);
    let n = if sub { -n } else { n };
    rc_result(with(dt, |dt| match dt {
        Value::Date { local, zone, .. } => date(*local + n, zone.clone()),
        _ => unreachable!(),
    }))
}
/// # Safety
/// The argument is a rooted exact timedelta; the result is separately owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_negate(d: *const Object) -> *const Object {
    // SAFETY: the caller roots the exact timedelta until its microseconds have been read.
    let n = delta(unsafe { object(d) });
    rc_result(validate_delta(&BigInt::from(-n)).map(|n| owner(Value::Delta(n))))
}
/// # Safety
/// The receiver is a rooted datetime or timedelta; the field index is statically validated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_field(o: *const Object, index: usize) -> usize {
    // SAFETY: the receiver is rooted and the compiler supplies a tagged integer field index.
    let (o, index) = unsafe { (object(o), Int::borrow(index)) };
    let i = index.as_small().unwrap() as usize;
    let n = with(o, |v| match v {
        Value::Date { local, fold, .. } => date_fields(*local, *fold)[i],
        Value::Delta(n) => {
            let (d, s, u) = parts(*n);
            [d, s, u][i]
        }
        _ => unreachable!(),
    });
    Int::new(BigInt::from(n)).into_raw()
}
/// # Safety
/// The receiver is a rooted datetime; the optional owned zone is zero only for naive values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_tzinfo(dt: *const Object) -> usize {
    // SAFETY: the caller roots the datetime while its optional zone is cloned.
    with(unsafe { object(dt) }, |v| match v {
        Value::Date { zone, .. } => zone
            .as_ref()
            .map_or(0, |z| crate::ObjectOwner::into_raw(z.clone()) as usize),
        _ => unreachable!(),
    })
}
/// # Safety
/// The rooted receiver is a timezone or datetime. A naive datetime has no UTC offset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_offset(o: *const Object) -> usize {
    fn ret(z: &Object) -> usize {
        with(z, |v| match v {
            Value::Zone { offset, .. } => crate::ObjectOwner::into_raw(offset.clone()) as usize,
            _ => unreachable!(),
        })
    }
    // SAFETY: the caller roots the receiver, which owns its fixed offset when present.
    with(unsafe { object(o) }, |v| match v {
        Value::Date { zone, .. } => zone.as_ref().map_or(0, |z| ret(z)),
        Value::Zone { offset, .. } => crate::ObjectOwner::into_raw(offset.clone()) as usize,
        _ => unreachable!(),
    })
}
/// # Safety
/// The receiver is an exact timezone; the result has its own string owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_zonename(z: *const Object) -> *const Str {
    // SAFETY: the caller roots the exact fixed zone during the string lookup.
    with(unsafe { object(z) }, |v| match v {
        Value::Zone { offset, name } => crate::StrOwner::into_raw(
            name.clone()
                .unwrap_or_else(|| text::owned_string(&zone_name(delta(offset)))),
        ),
        _ => unreachable!(),
    })
}
/// # Safety
/// The receiver is a rooted datetime or timedelta; static calls decide whether to use the ISO separator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_text(o: *const Object, iso: bool) -> *const Str {
    // SAFETY: the compiler roots this exact calendar value while its fields are formatted.
    let s = with(unsafe { object(o) }, |v| match v {
        Value::Date { local, zone, .. } => date_text(*local, zone, if iso { 'T' } else { ' ' }),
        Value::Delta(n) => {
            let (d, s, u) = parts(*n);
            let prefix = if d == 0 {
                String::new()
            } else {
                format!("{d} day{}, ", if d.abs() == 1 { "" } else { "s" })
            };
            let frac = if u == 0 {
                String::new()
            } else {
                format!(".{u:06}")
            };
            format!(
                "{prefix}{}:{:02}:{:02}{frac}",
                s / 3600,
                s / 60 % 60,
                s % 60
            )
        }
        Value::Zone { offset, name } => name
            .as_ref()
            .map_or_else(|| zone_name(delta(offset)), |s| s.as_str().to_owned()),
    });
    crate::StrOwner::into_raw(text::owned_string(&s))
}
