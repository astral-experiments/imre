//! RFC 2822 date operations on the ordinary native datetime and fixed-zone objects.
use super::*;
use crate::objects::logging::timed;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn month(word: &str) -> Option<usize> {
    MONTHS
        .iter()
        .position(|m| m.eq_ignore_ascii_case(word) || m[..3].eq_ignore_ascii_case(word))
        .map(|i| i + 1)
}

// str.isdigit also includes Unicode 16 Digit characters which int() rejects.
fn year_starts_digit(c: char) -> bool {
    crate::integer_text::decimal_digit(c).is_some()
        || matches!(c as u32,
        0xb2..=0xb3 | 0xb9 | 0x1369..=0x1371 | 0x19da | 0x2070 | 0x2074..=0x2079 |
        0x2080..=0x2089 | 0x2460..=0x2468 | 0x2474..=0x247c | 0x2488..=0x2490 |
        0x24ea | 0x24f5..=0x24fd | 0x24ff | 0x2776..=0x277e | 0x2780..=0x2788 |
        0x278a..=0x2792 | 0x10a40..=0x10a43 | 0x10e60..=0x10e68 | 0x11052..=0x1105a |
        0x1f100..=0x1f10a)
}

fn integer(text: &str) -> Option<Int> {
    let digits = text
        .chars()
        .filter(|&c| crate::integer_text::decimal_digit(c).is_some())
        .count();
    crate::integer_text::decimal_digits_allowed(digits)
        .then(|| crate::parse_decimal_int(text))
        .flatten()
}

fn parsed_zone(word: &str) -> Result<Option<crate::ObjectOwner>, Error> {
    let hours = match word.to_uppercase().as_str() {
        "UT" | "UTC" | "GMT" | "Z" => 0,
        "AST" | "EDT" => -4,
        "ADT" => -3,
        "EST" | "CDT" => -5,
        "CST" | "MDT" => -6,
        "MST" | "PDT" => -7,
        "PST" => -8,
        _ => {
            let Some(hhmm) = integer(word) else {
                return Ok(None);
            };
            if hhmm.as_small() == Some(0) && word.starts_with('-') {
                return Ok(None);
            }
            let n = hhmm.big();
            let secs = Int::new((n.as_ref() / 100u8) * 3600u32 + (n.as_ref() % 100u8) * 60u8);
            let mut words = [Int::from(0).0; 7];
            words[1] = secs.0;
            return make_zone(make_delta(0, words)?, None).map(Some);
        }
    };
    make_zone(owner(Value::Delta(i128::from(hours) * 3_600_000_000)), None).map(Some)
}

// Mirror _parsedate_tz's obsolete RFC 850 and asctime forms, including its
// two-digit year pivot and the distinction between a known UTC and -0000.
fn parse(text: &str) -> Result<crate::ObjectOwner, Error> {
    let invalid = || {
        Error::new(
            Kind::ValueError,
            format!("Invalid date value or format \"{text}\""),
        )
    };
    let mut words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .filter(|s| !s.is_empty())
        // Five fields plus an optional weekday, even when trailing comments are long.
        .take(6)
        .collect();
    let first = words.first_mut().ok_or_else(invalid)?;
    if first.ends_with(',') || DAYS.iter().any(|d| first.eq_ignore_ascii_case(d)) {
        words.remove(0);
    } else if let Some((_, rest)) = first.rsplit_once(',') {
        *first = rest;
    }
    if words.len() == 3 {
        let v: Vec<_> = words[0].split('-').take(4).collect();
        if v.len() == 3 {
            words.splice(..1, v);
        }
    }
    if words.len() == 4 {
        let time = words[3];
        if let Some(i) = time.find('+').or_else(|| time.find('-')).filter(|&i| i > 0) {
            words[3] = &time[..i];
            words.push(&time[i..]);
        } else {
            words.push("");
        }
    }
    let &[mut dd, m, mut yy, mut tm, mut tz, ..] = words.as_slice() else {
        return Err(invalid());
    };
    if dd.is_empty() || m.is_empty() || yy.is_empty() {
        return Err(invalid());
    }
    let mon = month(m)
        .or_else(|| {
            let n = month(dd);
            dd = m;
            n
        })
        .ok_or_else(invalid)?;
    dd = dd.strip_suffix(',').unwrap_or(dd);
    if yy.find(':').is_some_and(|i| i > 0) {
        std::mem::swap(&mut yy, &mut tm);
    }
    yy = yy.strip_suffix(',').unwrap_or(yy);
    if !year_starts_digit(yy.chars().next().ok_or_else(invalid)?) {
        std::mem::swap(&mut yy, &mut tz);
    }
    tm = tm.strip_suffix(',').unwrap_or(tm);
    let delim = if !tm.contains(':') && tm.contains('.') {
        '.'
    } else {
        ':'
    };
    let mut clock = tm.split(delim);
    let hour = clock.next().and_then(integer).ok_or_else(invalid)?;
    let min = clock.next().and_then(integer).ok_or_else(invalid)?;
    let sec = integer(clock.next().unwrap_or("0")).ok_or_else(invalid)?;
    if clock.next().is_some() {
        return Err(invalid());
    }
    let mut year = integer(yy).ok_or_else(invalid)?;
    let day = integer(dd).ok_or_else(invalid)?;
    if year.cmp(&Int::from(100)).is_lt() {
        let century = if year.cmp(&Int::from(68)).is_gt() {
            1900
        } else {
            2000
        };
        year = Int::new(year.big().as_ref() + century);
    }
    let zone = parsed_zone(tz)?;
    let fields = [
        year,
        Int::from(mon as i64),
        day,
        hour,
        min,
        sec,
        Int::from(0),
        Int::from(0),
    ];
    new_date(std::array::from_fn(|i| fields[i].0), zone)
}

/// # Safety
/// The generated call roots an exact string for the duration of the parse.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_email_parsedate(input: *const Str) -> *const Object {
    // SAFETY: the exact string is immutable and its owner outlives this call.
    rc_result(parse(unsafe { &*input }))
}

fn formatted(fields: [i128; 8], weekday: usize, zone: &str) -> Result<*const Str, Error> {
    let [y, m, d, h, min, sec, ..] = fields;
    crate::checked_owned_str(compact_str::format_compact!(
        "{}, {d:02} {} {y:04} {h:02}:{min:02}:{sec:02} {zone}",
        DAYS[weekday],
        &MONTHS[m as usize - 1][..3]
    ))
}

/// # Safety
/// The caller roots an exact datetime and any fixed timezone it owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_email_format_datetime(dt: *const Object, usegmt: bool) -> *const Str {
    // SAFETY: only an exact immutable native datetime can reach this operation.
    let dt = unsafe { object(dt) };
    finish(
        with(dt, |value| {
            let Value::Date { local, zone, .. } = value else {
                unreachable!()
            };
            let offset = zone.as_ref().map(|z| offset(z));
            let zone = if usegmt {
                if offset != Some(0) {
                    return Err(Error::new(
                        Kind::ValueError,
                        "usegmt option requires a UTC datetime",
                    ));
                }
                "GMT".to_owned()
            } else {
                offset.map_or_else(|| "-0000".to_owned(), |us| offset_text(us).replace(':', ""))
            };
            formatted(
                date_fields(*local, 0),
                (local.div_euclid(DAY) % 7) as usize,
                &zone,
            )
        }),
        std::ptr::null(),
    )
}

// datetime.fromtimestamp rounds the fractional microsecond to nearest, ties even,
// before asking for the civil day. Include a possible carry across a whole second.
fn timestamp(value: f64) -> Result<i64, Error> {
    let seconds = timed::float_seconds(value)?;
    if ((value - value.floor()) * 1_000_000.0).round_ties_even() >= 1_000_000.0 {
        seconds.checked_add(1).ok_or_else(timed::overflow)
    } else {
        Ok(seconds)
    }
}

/// # Safety
/// The tag identifies either None, an owned integer word, or exact binary64 bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_email_formatdate(
    kind: u32,
    word: usize,
    local: bool,
    usegmt: bool,
) -> *const Str {
    finish(
        (|| {
            // None is the zero word in each optional calendar input. Raw binary64
            // positive zero also has zero bits, but denotes the Unix epoch.
            let present_kind = if matches!(kind, 3 | 4) && word == 0 {
                0
            } else {
                kind
            };
            let sec = match present_kind {
                0 => {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|_| timed::overflow())?;
                    timestamp(now.as_secs_f64())?
                }
                // SAFETY: the generated integer arm keeps its tagged word rooted.
                1 | 3 => unsafe { Int::borrow(word) }
                    .big()
                    .to_i64()
                    .ok_or_else(timed::overflow)?,
                2 => timestamp(f64::from_bits(word as u64))?,
                // SAFETY: the optional-float arm keeps a present payload rooted.
                4 => timestamp(unsafe { crate::floats::imre_float_unbox(word) })?,
                _ => unreachable!(),
            };
            let utc = timed::civil(sec, true)?;
            let year = i64::from(utc.tm_year) + 1900;
            if !(1..=9999).contains(&year) {
                return Err(Error::new(
                    Kind::ValueError,
                    format!("year must be in 1..9999, not {year}"),
                ));
            }
            let (civil, zone) = if local {
                let t = timed::civil(sec, false)?;
                let zone = offset_text(i128::from(t.tm_gmtoff) * 1_000_000).replace(':', "");
                (t, zone)
            } else {
                (utc, if usegmt { "GMT" } else { "-0000" }.to_owned())
            };
            let y = i128::from(civil.tm_year) + 1900;
            if !(1..=9999).contains(&y) {
                return Err(Error::new(Kind::OverflowError, "date value out of range"));
            }
            formatted(
                [
                    y,
                    i128::from(civil.tm_mon) + 1,
                    i128::from(civil.tm_mday),
                    i128::from(civil.tm_hour),
                    i128::from(civil.tm_min),
                    i128::from(civil.tm_sec),
                    0,
                    0,
                ],
                ((civil.tm_wday + 6) % 7) as usize,
                &zone,
            )
        })(),
        std::ptr::null(),
    )
}
