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

    pub(crate) fn seconds(&self) -> f64 {
        total_seconds(delta(&self.0))
    }

    pub(crate) fn zone(offset: &Self, name: Option<&str>) -> Result<Self, Error> {
        make_zone(offset.0.clone(), name.map(text::owned_string)).map(Self)
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

    pub(crate) fn clock_parts(&self) -> [i64; 5] {
        clock_parts(&self.0)
    }

    pub(crate) fn clock_order(&self, other: &Self) -> Result<std::cmp::Ordering, Error> {
        with(&self.0, |a| with(&other.0, |b| clock::order(a, b))).ok_or_else(|| {
            Error::new(
                Kind::TypeError,
                "can't compare offset-naive and offset-aware times",
            )
        })
    }

    pub(crate) fn combine(&self, time: &Self, zone: Option<&Self>) -> Result<Self, Error> {
        combine(&self.0, &time.0, zone.map(|z| z.0.clone())).map(Self)
    }

    pub(crate) fn datetime(fields: [i32; 8], zone: Option<&Self>) -> Result<Self, Error> {
        datetime_from_fields(fields, zone.map(|z| z.0.clone())).map(Self)
    }

    pub(crate) fn from_timestamp(
        timestamp: &CalendarNumber,
        zone: Option<&Self>,
    ) -> Result<Self, Error> {
        let (seconds, micros) = timestamp.timeval()?;
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

    pub(crate) fn ordinal(&self) -> i64 {
        with(&self.0, |v| match v {
            Value::DateOnly(ord) => *ord as i64,
            Value::Date { local, .. } => (local.div_euclid(DAY) + 1) as i64,
            _ => unreachable!(),
        })
    }

    pub(crate) fn delta_parts(&self) -> (i128, i128, i128) {
        parts(delta(&self.0))
    }

    /// Timedeltas, zones and aware datetimes hash as their normalized duration.
    pub(crate) fn hash_delta_parts(&self) -> Option<(i128, i128, i128)> {
        with(&self.0, |v| match v {
            Value::Delta(n) => Some(parts(*n)),
            Value::Zone { offset, .. } => Some(parts(delta(offset))),
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

    pub(crate) fn repr(&self) -> String {
        let mut s = String::new();
        with(&self.0, |v| {
            write_repr(v, &mut super::super::values::ReprWriter(&mut s))
        })
        .expect("writing to a string succeeds");
        s
    }
}
