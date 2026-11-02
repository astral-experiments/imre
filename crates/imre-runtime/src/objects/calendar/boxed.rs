//! The runtime import backend owns the same immutable calendar values as typed code.
use super::*;
use crate::objects::logging::timed;

#[derive(Clone)]
pub(crate) struct Calendar(crate::ObjectOwner);

pub(crate) enum CalendarNumber {
    Int(Int),
    Float(f64),
}

impl CalendarNumber {
    fn timeval(&self) -> Result<(i64, i32), Error> {
        match self {
            Self::Int(n) => Ok((n.to_i64().ok_or_else(timed::overflow)?, 0)),
            Self::Float(n) => {
                if n.is_nan() {
                    return Err(Error::new(
                        Kind::ValueError,
                        "Invalid value NaN (not a number)",
                    ));
                }
                if !(i64::MIN as f64..-(i64::MIN as f64)).contains(n) {
                    return Err(timed::overflow());
                }
                let micros = (n.fract() * 1_000_000.0).round_ties_even() as i64;
                let seconds = (*n as i64)
                    .checked_add(micros.div_euclid(1_000_000))
                    .ok_or_else(timed::overflow)?;
                Ok((seconds, micros.rem_euclid(1_000_000) as i32))
            }
        }
    }
}

impl Calendar {
    pub(crate) fn component(n: &Int) -> Result<i32, Error> {
        c_component(n.0)
    }

    pub(crate) fn class(&self) -> &'static str {
        with(&self.0, |value| match value {
            Value::DateOnly(_) => "date",
            Value::Time { .. } => "time",
            Value::Date { .. } => "datetime",
            Value::Delta(_) => "timedelta",
            Value::Zone { .. } => "timezone",
        })
    }

    /// Build each type's own min, max and resolution, in CPython's class-dictionary order.
    pub(crate) fn class_constants(class: &str) -> Option<[Self; 3]> {
        let (min, max, resolution) = match class {
            "datetime.date" => (Value::DateOnly(1), Value::DateOnly(MAX_ORD), DAY),
            "datetime.datetime" => {
                let datetime = |local| Value::Date {
                    local,
                    zone: None,
                    fold: 0,
                };
                (datetime(0), datetime(MAX_ORD * DAY - 1), 1)
            }
            "datetime.time" => {
                let time = |local| Value::Time {
                    local,
                    zone: None,
                    fold: 0,
                };
                (time(0), time(DAY - 1), 1)
            }
            "datetime.timedelta" => (
                Value::Delta(-999_999_999 * DAY),
                Value::Delta(1_000_000_000 * DAY - 1),
                1,
            ),
            _ => return None,
        };
        Some([min, max, Value::Delta(resolution)].map(|value| Self(owner(value))))
    }

    pub(crate) fn zone_limit(max: bool) -> Self {
        Self(zone_limit(max))
    }

    pub(crate) fn utc() -> Self {
        Self(utc())
    }

    /// Keep a parsed date's native timezone alive in the boxed datetime as well.
    pub(crate) fn parse_mail_date(text: &str) -> Result<(Self, Option<Self>), Error> {
        let day = maildates::parse(text)?;
        let tz = with(&day, |value| match value {
            Value::Date { zone, .. } => zone.clone().map(Self),
            _ => unreachable!(),
        });
        Ok((Self(day), tz))
    }

    pub(crate) fn format_mail_date(&self, usegmt: bool) -> Result<Str, Error> {
        maildates::format_datetime(&self.0, usegmt)
    }

    pub(crate) fn format_mail_timestamp(
        number: Option<&CalendarNumber>,
        local: bool,
        usegmt: bool,
    ) -> Result<Str, Error> {
        maildates::formatdate(number, local, usegmt)
    }

    pub(crate) fn duration(units: &[CalendarNumber; 7]) -> Result<Self, Error> {
        let mut mask = 0;
        let words = std::array::from_fn(|i| match &units[i] {
            CalendarNumber::Int(n) => n.0,
            CalendarNumber::Float(n) => {
                mask |= 1 << i;
                n.to_bits() as usize
            }
        });
        make_delta(mask, words).map(Self)
    }

    /// Shift the receiver by a duration using the typed calendar's range and fold rules.
    pub(crate) fn shift(&self, duration: &Self, subtract: bool) -> Result<Self, Error> {
        shift(&self.0, duration.delta_microseconds(), subtract).map(Self)
    }

    /// Subtract two datetimes backed by naive or fixed-offset wall times.
    pub(crate) fn datetime_difference(&self, other: &Self) -> Result<Self, Error> {
        let parts = |dt: &Self| {
            with(&dt.0, |v| match v {
                Value::Date { local, zone, .. } => (*local, zone.as_ref().map(|z| offset(z))),
                _ => unreachable!(),
            })
        };
        let (a, az) = parts(self);
        let (b, bz) = parts(other);
        let difference = match (az, bz) {
            (None, None) => a - b,
            (Some(x), Some(y)) => (a - x) - (b - y),
            _ => {
                return Err(Error::new(
                    Kind::TypeError,
                    "can't subtract offset-naive and offset-aware datetimes",
                ));
            }
        };
        try_owner(Value::Delta(difference)).map(Self)
    }

    pub(crate) fn negate_delta(&self) -> Result<Self, Error> {
        negate_delta(&self.0).map(Self)
    }

    pub(crate) fn seconds(&self) -> f64 {
        total_seconds(delta(&self.0))
    }

    pub(crate) fn zone(offset: &Self, name: Option<&str>) -> Result<Self, Error> {
        make_zone(offset.0.clone(), name.map(text::owned_string)).map(Self)
    }

    pub(crate) fn zone_offset(&self) -> Option<Self> {
        with(&self.0, |value| match value {
            Value::Zone { offset, .. } => Some(Self(offset.clone())),
            _ => None,
        })
    }

    /// Format the stored fixed UTC offset for %z or %:z.
    pub(crate) fn strftime_offset(&self, colon: bool) -> String {
        Self::format_utc_offset(offset(&self.0), colon)
    }

    pub(crate) fn format_utc_offset(micros: i128, colon: bool) -> String {
        let text = offset_text(micros);
        if colon { text } else { text.replace(':', "") }
    }

    pub(crate) fn is_utc(&self) -> bool {
        crate::ObjectOwner::ptr_eq(&self.0, &utc())
    }

    pub(crate) fn date(year: i32, month: i32, day: i32) -> Result<Self, Error> {
        try_owner(Value::DateOnly(checked_ordinal(year, month, day)?)).map(Self)
    }

    pub(crate) fn time_micros(fields: [i32; 5]) -> Result<i128, Error> {
        time_micros(fields)
    }

    pub(crate) fn time(local: i128, fold: i32, zone: Option<&Self>) -> Result<Self, Error> {
        try_owner(Value::Time {
            local,
            fold,
            zone: zone.map(|z| z.0.clone()),
        })
        .map(Self)
    }

    /// Return the datetime's local clock and fold without carrying over its timezone.
    pub(crate) fn time_of_day(&self) -> Result<Self, Error> {
        with(&self.0, |value| match value {
            Value::Date { local, fold, .. } => Self::time(local.rem_euclid(DAY), *fold, None),
            _ => unreachable!(),
        })
    }

    pub(crate) fn wall_microseconds(&self) -> i128 {
        with(&self.0, |value| match value {
            Value::Date { local, .. } | Value::Time { local, .. } => *local,
            _ => unreachable!(),
        })
    }

    pub(crate) fn clock_parts(&self) -> [i64; 5] {
        clock_parts(&self.0)
    }

    /// Compare two values of the same orderable calendar class.
    pub(crate) fn order(&self, other: &Self, operator: &str) -> Result<std::cmp::Ordering, Error> {
        compare_rooted(&self.0, &other.0, false, operator)
    }

    pub(crate) fn combine(&self, time: &Self, zone: Option<&Self>) -> Result<Self, Error> {
        combine(&self.0, &time.0, zone.map(|z| z.0.clone())).map(Self)
    }

    pub(crate) fn datetime(fields: [i32; 8], zone: Option<&Self>) -> Result<Self, Error> {
        datetime_from_fields(fields, zone.map(|z| z.0.clone())).map(Self)
    }

    /// Preserve the civil fields while replacing their native fixed timezone.
    pub(crate) fn with_datetime_zone(&self, zone: Option<&Self>) -> Result<Self, Error> {
        with(&self.0, |value| match value {
            Value::Date { local, fold, .. } => try_owner(Value::Date {
                local: *local,
                fold: *fold,
                zone: zone.map(|zone| zone.0.clone()),
            })
            .map(Self),
            _ => unreachable!(),
        })
    }

    /// Resolve a local clock to the system offset on its side of a fold or gap.
    pub(crate) fn system_offset(&self) -> Result<Self, Error> {
        with(&self.0, |value| {
            let Value::Date { local, fold, .. } = value else {
                unreachable!()
            };
            let micros = conversion::naive_offset(*local, *fold != 0)?;
            Ok(Self(owner(Value::Delta(micros))))
        })
    }

    /// Obtain the fixed offset and abbreviation in effect at these UTC fields.
    pub(crate) fn system_zone_at_utc(&self) -> Result<(Self, Option<String>), Error> {
        let micros = self.wall_microseconds() - (EPOCH_ORD - 1) * DAY;
        let seconds = (micros.div_euclid(1_000_000)) as i64;
        let parts = super::super::time::ClockParts::from_int(&Int::from(seconds), false)?;
        let offset = owner(Value::Delta(i128::from(parts.offset) * 1_000_000));
        let name = parts.zone.as_ref().map(|name| name.as_str().to_owned());
        Ok((Self(make_zone(offset, parts.zone)?), name))
    }

    pub(crate) fn timestamp_with_offset(&self, offset: i128) -> f64 {
        total_seconds(self.wall_microseconds() - (EPOCH_ORD - 1) * DAY - offset)
    }

    pub(crate) fn timestamp(&self) -> Result<f64, Error> {
        with(&self.0, |value| {
            let Value::Date { local, zone, fold } = value else {
                unreachable!()
            };
            let micros = *local - (EPOCH_ORD - 1) * DAY;
            if let Some(zone) = zone {
                return Ok(total_seconds(micros - offset(zone)));
            }
            let seconds =
                conversion::local_candidate(micros.div_euclid(1_000_000) as i64, *fold != 0)?;
            // CPython adds the fractional second after resolving the local wall time.
            Ok(seconds as f64 + micros.rem_euclid(1_000_000) as f64 / 1_000_000.0)
        })
    }

    pub(crate) fn from_timestamp(
        timestamp: &CalendarNumber,
        zone: Option<&Self>,
    ) -> Result<Self, Error> {
        let (seconds, micros) = timestamp.timeval()?;
        Self::from_timeval(seconds, micros, zone)
    }

    pub(crate) fn now(zone: Option<&Self>) -> Result<Self, Error> {
        let micros = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(delta) => delta.as_micros() as i128,
            Err(e) => (-(e.duration().as_nanos() as i128)).div_euclid(1000),
        };
        let seconds = i64::try_from(micros.div_euclid(1_000_000)).map_err(|_| timed::overflow())?;
        Self::from_timeval(seconds, micros.rem_euclid(1_000_000) as i32, zone)
    }

    fn from_timeval(seconds: i64, micros: i32, zone: Option<&Self>) -> Result<Self, Error> {
        let tm = timed::civil(seconds, zone.is_some())?;
        let year = tm.tm_year.checked_add(1900).ok_or_else(timed::overflow)?;
        let fields = [
            year,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec.min(59),
            micros,
            if zone.is_none() {
                conversion::local_fold(seconds)?
            } else {
                0
            },
        ];
        let value = Self::datetime(fields, zone)?;
        if let Some(zone) = zone {
            let adjustment = offset(&zone.0);
            if adjustment != 0 {
                return with(&value.0, |v| match v {
                    Value::Date { local, .. } => {
                        date(*local + adjustment, Some(zone.0.clone())).map(Self)
                    }
                    _ => unreachable!(),
                });
            }
        }
        Ok(value)
    }

    pub(crate) fn fields(&self) -> [i128; 8] {
        with(&self.0, |v| match v {
            Value::Date { local, fold, .. } => date_fields(*local, *fold),
            Value::DateOnly(ord) => date_fields((*ord - 1) * DAY, 0),
            _ => unreachable!(),
        })
    }

    pub(crate) fn timetuple(&self) -> [i64; 9] {
        let [y, m, d, h, minute, s, _, _] = self.fields().map(|n| n as i64);
        let ord = self.ordinal();
        [
            y,
            m,
            d,
            h,
            minute,
            s,
            (ord - 1) % 7,
            ord - i64::from(days_before_year(y as i32)),
            -1,
        ]
    }

    pub(crate) fn ordinal_fields(ordinal: i32) -> Result<[i32; 3], Error> {
        if ordinal < 1 {
            return Err(Error::new(Kind::ValueError, "ordinal must be >= 1"));
        }
        // A subtype constructor can accept a year past 9999. Decode within one
        // Gregorian cycle, then restore the full year before calling Python.
        let days = ordinal - 1;
        let cycles = days / 146_097;
        let (year, month, day) = ymd(i128::from(days % 146_097 + 1));
        Ok([year + 400 * cycles, month, day])
    }

    /// Convert an ISO week date, including a final week that extends into year 10000.
    pub(crate) fn iso_week_date(year: i32, week: i32, weekday: i32) -> Option<[i32; 3]> {
        if !(1..=9999).contains(&year) || !(1..=53).contains(&week) || !(1..=7).contains(&weekday)
        {
            return None;
        }
        let week_one = |y| {
            let jan_four = days_before_year(y) + 4;
            jan_four - (jan_four - 1) % 7
        };
        let monday = week_one(year) + (week - 1) * 7;
        if monday >= week_one(year + 1) {
            return None;
        }
        Self::ordinal_fields(monday + weekday - 1).ok()
    }

    /// Normalize ISO 24:00 before the receiving class validates the civil fields.
    pub(crate) fn iso_midnight(date: &mut [i32; 3], clock: &mut [i32; 4]) -> Result<(), Error> {
        let [year, month, day] = date;
        if clock[0] == 24 && *month <= 12 {
            // CPython's days-in-month table has a zero sentinel for month zero.
            let last = if *month == 0 {
                0
            } else {
                month_days(*year, *month)
            };
            if *day <= last {
                if clock[1..] != [0, 0, 0] {
                    return Err(Error::new(
                        Kind::ValueError,
                        "minute, second, and microsecond must be 0 when hour is 24",
                    ));
                }
                clock[0] = 0;
                *day += 1;
                if *day > last {
                    *day = 1;
                    *month += 1;
                    if *month > 12 {
                        *month = 1;
                        *year += 1;
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn ordinal(&self) -> i64 {
        with(&self.0, |v| match v {
            Value::DateOnly(ord) => *ord as i64,
            Value::Date { local, .. } => (local.div_euclid(DAY) + 1) as i64,
            _ => unreachable!(),
        })
    }

    pub(crate) fn delta_microseconds(&self) -> i128 {
        delta(&self.0)
    }

    pub(crate) fn from_delta_microseconds(us: &Int) -> Result<Self, Error> {
        try_owner(Value::Delta(validate_delta(&us.big())?)).map(Self)
    }

    pub(crate) fn delta_parts(&self) -> (i128, i128, i128) {
        parts(delta(&self.0))
    }

    /// CPython hashes an aware clock as a duration, counting January 1 of year 1 as day one.
    pub(crate) fn clock_hash_parts(&self, offset: i128) -> (i128, i128, i128) {
        let origin = if self.class() == "datetime" { DAY } else { 0 };
        parts(self.wall_microseconds() + origin - offset)
    }

    /// Timedeltas, zones and aware datetimes hash as their normalized duration.
    pub(crate) fn hash_delta_parts(&self) -> Option<(i128, i128, i128)> {
        with(&self.0, |v| match v {
            Value::Delta(n) => Some(parts(*n)),
            Value::Zone { offset, .. } => Some(parts(delta(offset))),
            Value::Time {
                local,
                zone: Some(zone),
                ..
            } => Some(parts(local - offset(zone))),
            Value::Date {
                local,
                zone: Some(zone),
                ..
            } => Some(parts(local + DAY - offset(zone))),
            _ => None,
        })
    }

    pub(crate) fn equal(&self, other: &Self) -> bool {
        with(&self.0, |a| with(&other.0, |b| equal(a, b)))
    }

    pub(crate) fn text(&self) -> String {
        with(&self.0, |v| calendar_text(v, false))
    }

    /// date.__str__ calls isoformat, which uses a T separator for a datetime receiver.
    pub(crate) fn iso_text(&self) -> String {
        with(&self.0, |v| calendar_text(v, true))
    }

    /// Render civil fields with the receiver's class name. The caller formats its Python timezone.
    pub(crate) fn subtype_repr(&self, class: &str, as_date: bool) -> String {
        let mut text = String::new();
        let mut out = super::super::values::ReprWriter(&mut text);
        if as_date {
            write_named_repr(
                &Value::DateOnly(i128::from(self.ordinal())),
                Some(class),
                &mut out,
            )
        } else {
            with(&self.0, |v| write_named_repr(v, Some(class), &mut out))
        }
        .expect("writing to a string succeeds");
        text
    }

    pub(crate) fn repr(&self) -> String {
        let mut s = String::new();
        with(&self.0, |v| {
            write_repr(v, &mut super::super::values::ReprWriter(&mut s))
        })
        .expect("writing to a string succeeds");
        s
    }
}

/// # Safety
/// A nonzero word holds a rooted fixed timezone for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_calendar_now(z: usize) -> *const Object {
    // SAFETY: a present zone stays rooted until the calendar owns it.
    let zone = (z != 0).then(|| Calendar(unsafe { retain(z as *const Object) }));
    rc_result(Calendar::now(zone.as_ref()).map(|datetime| datetime.0))
}
