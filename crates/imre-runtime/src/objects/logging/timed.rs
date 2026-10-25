//! Calendar scheduling and retained file streams for TimedRotatingFileHandler.
use crate::StrOwner;
use std::fs;
use std::mem::MaybeUninit;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use num_bigint::BigInt;
use num_traits::ToPrimitive;

use super::super::exceptions::{Error, Kind, Result, finish};
use super::super::{Int, Object, Storage, StorageCell, Str, Words, fail, files, object, posixpath};

const DAY: i64 = 86_400;

pub(in crate::objects) struct Clock([i64; 5]);

#[derive(Clone, Copy)]
enum Period {
    Second,
    Minute,
    Hour,
    Day,
    Midnight,
    Week(i32),
}

impl Period {
    fn parse(when: &str) -> Result<Self> {
        Ok(match when {
            "S" => Self::Second,
            "M" => Self::Minute,
            "H" => Self::Hour,
            "D" => Self::Day,
            "MIDNIGHT" => Self::Midnight,
            _ if when.starts_with('W') => {
                if when.chars().count() != 2 {
                    return Err(Error::new(
                        Kind::ValueError,
                        format!(
                            "You must specify a day for weekly rollover from 0 to 6 (0 is Monday): {when}"
                        ),
                    ));
                }
                match when.as_bytes() {
                    [b'W', day @ b'0'..=b'6'] => Self::Week(i32::from(*day - b'0')),
                    _ => {
                        return Err(Error::new(
                            Kind::ValueError,
                            format!("Invalid day specified for weekly rollover: {when}"),
                        ));
                    }
                }
            }
            _ => {
                return Err(Error::new(
                    Kind::ValueError,
                    format!("Invalid rollover interval specified: {when}"),
                ));
            }
        })
    }

    fn seconds(self) -> i64 {
        match self {
            Self::Second => 1,
            Self::Minute => 60,
            Self::Hour => 3600,
            Self::Day | Self::Midnight => DAY,
            Self::Week(_) => 7 * DAY,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::Second => "%Y-%m-%d_%H-%M-%S",
            Self::Minute => "%Y-%m-%d_%H-%M",
            Self::Hour => "%Y-%m-%d_%H",
            _ => "%Y-%m-%d",
        }
    }

    fn matches(self, suffix: &str) -> bool {
        let mask = match self {
            Self::Second => b"dddd-dd-dd_dd-dd-dd".as_slice(),
            Self::Minute => b"dddd-dd-dd_dd-dd",
            Self::Hour => b"dddd-dd-dd_dd",
            _ => b"dddd-dd-dd",
        };
        mask.len() == suffix.len()
            && mask.iter().zip(suffix.bytes()).all(|(&m, b)| {
                if m == b'd' {
                    b.is_ascii_digit()
                } else {
                    m == b
                }
            })
    }
}

pub(super) fn overflow() -> Error {
    Error::new(
        Kind::OverflowError,
        "timestamp out of range for platform time_t",
    )
}

pub(super) fn civil(seconds: i64, utc: bool) -> Result<libc::tm> {
    let mut result = MaybeUninit::uninit();
    // SAFETY: the timestamp and properly sized result live through the reentrant libc call.
    let ptr = unsafe {
        if utc {
            libc::gmtime_r(&seconds, result.as_mut_ptr())
        } else {
            libc::localtime_r(&seconds, result.as_mut_ptr())
        }
    };
    if ptr.is_null() {
        return Err(files::errno_error(std::io::Error::last_os_error()));
    }
    // SAFETY: a non-null return means libc initialized the result.
    Ok(unsafe { result.assume_init() })
}

pub(super) fn float_seconds(value: f64) -> Result<i64> {
    if value.is_nan() {
        return Err(Error::new(
            Kind::ValueError,
            "Invalid value NaN (not a number)",
        ));
    }
    value.floor().to_i64().ok_or_else(overflow)
}

pub(in crate::objects) struct Handler {
    filename: StrOwner,
    stream: Option<crate::ObjectOwner>,
    when: StrOwner,
    period: Period,
    interval: Int,
    backups: Int,
    rollover_at: Int,
    utc: bool,
    delay: bool,
    clock: Option<crate::ObjectOwner>,
}

impl Handler {
    /// Keep the daily and weekday delays separate so each float addition rounds as in Python.
    /// An omitted time means the *end* of the chosen day. Explicit 00:00 means
    /// its start, which can select a different week.
    fn calendar_delays(&self, now: i64) -> Result<(i64, i64, bool)> {
        let t = civil(now, self.utc)?;
        let at = self.clock.as_ref().map_or(DAY, |clock| {
            // SAFETY: the handler retains this exact immutable time.
            unsafe {
                clock.0.with(|s| match s {
                    Storage::ClockTime(Clock([h, m, s, _, _])) => (h * 60 + m) * 60 + s,
                    _ => unreachable!(),
                })
            }
        });
        let mut until = at - i64::from((t.tm_hour * 60 + t.tm_min) * 60 + t.tm_sec);
        let mut day = (t.tm_wday + 6) % 7;
        if until <= 0 {
            until += DAY;
            day = (day + 1) % 7;
        }
        let weekday_delay = match self.period {
            Period::Week(target) => DAY * i64::from((target - day).rem_euclid(7)),
            _ => 0,
        };
        Ok((until, weekday_delay, t.tm_isdst > 0))
    }

    fn dst_adjust(&self, dst_now: bool, target: i64) -> Result<i64> {
        let dst_target = civil(target, false)?.tm_isdst > 0;
        Ok(if dst_now == dst_target {
            0
        } else if dst_now {
            3600
        } else if civil(target.checked_sub(3600).ok_or_else(overflow)?, false)?.tm_isdst > 0 {
            -3600
        } else {
            0
        })
    }

    fn compute_int(&self, now: &Int) -> Result<Int> {
        let mut result = now.big().as_ref() + self.interval.big().as_ref();
        if matches!(self.period, Period::Midnight | Period::Week(_)) {
            let (until, weekday_delay, dst) =
                self.calendar_delays(now.big().to_i64().ok_or_else(overflow)?)?;
            result += until + weekday_delay - self.period.seconds();
            if !self.utc {
                result += self.dst_adjust(dst, result.to_i64().ok_or_else(overflow)?)?;
            }
        }
        Ok(Int::new(result))
    }

    fn compute_float(&self, now: f64) -> Result<f64> {
        let interval = self
            .interval
            .big()
            .to_f64()
            .filter(|n| n.is_finite())
            .ok_or_else(|| Error::new(Kind::OverflowError, "int too large to convert to float"))?;
        let mut result = now + interval;
        if matches!(self.period, Period::Midnight | Period::Week(_)) {
            let (until, weekday_delay, dst) = self.calendar_delays(float_seconds(now)?)?;
            let extra = Int::new(self.interval.big().as_ref() - self.period.seconds());
            result =
                (now + until as f64) + weekday_delay as f64 + crate::floats::int_to_float(&extra)?;
            if !self.utc {
                result += self.dst_adjust(dst, float_seconds(result)?)? as f64;
            }
        }
        Ok(result)
    }

    fn wall_seconds() -> Int {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => Int::new(BigInt::from(d.as_secs())),
            Err(error) => Int::new(-BigInt::from(error.duration().as_secs())),
        }
    }

    fn should_roll(&mut self) -> Result<bool> {
        let now = Self::wall_seconds();
        if now.big() < self.rollover_at.big() {
            return Ok(false);
        }
        if fs::metadata(self.filename.as_str()).is_ok_and(|meta| !meta.is_file()) {
            self.rollover_at = self.compute_int(&now)?;
            return Ok(false);
        }
        Ok(true)
    }

    fn roll(&mut self) -> Result<()> {
        let now = Self::wall_seconds();
        let start = (self.rollover_at.big().as_ref() - self.interval.big().as_ref())
            .to_i64()
            .ok_or_else(overflow)?;
        let mut clock = civil(start, self.utc)?;
        if !self.utc {
            let dst = civil(now.big().to_i64().ok_or_else(overflow)?, false)?.tm_isdst > 0;
            if dst != (clock.tm_isdst > 0) {
                clock = civil(
                    start
                        .checked_add(if dst { 3600 } else { -3600 })
                        .ok_or_else(overflow)?,
                    false,
                )?;
            }
        }
        let format = std::ffi::CString::new(self.period.suffix()).unwrap();
        let mut buf = [0u8; 128];
        // SAFETY: civil initialized the tm; every possible date suffix fits in the output.
        let n =
            unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), format.as_ptr(), &clock) };
        let suffix = std::str::from_utf8(&buf[..n]).expect("the rotation suffix is ASCII");
        let destination = format!("{}.{suffix}", self.filename);
        if Path::new(&destination).exists() {
            return Ok(());
        }
        if let Some(stream) = self.stream.take() {
            files::close_stream(&stream)?;
        }
        if Path::new(self.filename.as_str()).exists() {
            fs::rename(self.filename.as_str(), &destination).map_err(files::errno_error)?;
        }
        if self.backups.big().sign() == num_bigint::Sign::Plus {
            for path in self.expired_paths()? {
                fs::remove_file(path).map_err(files::errno_error)?;
            }
        }
        if !self.delay {
            // SAFETY: open transfers a fresh text stream owner on success.
            self.stream = Some(unsafe {
                crate::ObjectOwner::from_raw(files::open(&self.filename, 2, false, false)?)
            });
        }
        self.rollover_at = self.compute_int(&now)?;
        Ok(())
    }

    fn expired_paths(&self) -> Result<Vec<std::path::PathBuf>> {
        let base = Path::new(self.filename.as_str());
        let dir = base.parent().unwrap_or(base);
        let prefix = format!(
            "{}.",
            base.file_name().and_then(|n| n.to_str()).unwrap_or("")
        );
        let mut files = Vec::new();
        for entry in fs::read_dir(dir).map_err(files::errno_error)? {
            let entry = entry.map_err(files::errno_error)?;
            if entry
                .file_name()
                .to_str()
                .and_then(|s| s.strip_prefix(&prefix))
                .is_some_and(|s| self.period.matches(s))
            {
                files.push(entry.path());
            }
        }
        files.sort_unstable();
        let keep = self.backups.big();
        let keep = if keep.sign() == num_bigint::Sign::Minus {
            0
        } else {
            keep.to_usize().unwrap_or(usize::MAX)
        };
        files.truncate(files.len().saturating_sub(keep));
        Ok(files)
    }

    fn files_to_delete(&self) -> Result<crate::ObjectOwner> {
        let files = self.expired_paths()?;
        let mut names = Words::new(3);
        names
            .values
            .try_reserve_exact(files.len())
            .map_err(|_| Error::allocation())?;
        for path in files {
            let name = super::super::paths::path_utf8(path.into_os_string())?;
            names
                .values
                .push(StrOwner::into_raw(StrOwner::new(name)) as usize);
        }
        Ok(names.into_object())
    }
}

impl Storage {
    fn timed(&self) -> &Handler {
        match self {
            Self::TimedFileHandler(h) => h,
            _ => fail("SystemError", "expected timed file handler"),
        }
    }
    fn timed_mut(&mut self) -> &mut Handler {
        match self {
            Self::TimedFileHandler(h) => h,
            _ => fail("SystemError", "expected timed file handler"),
        }
    }
}

/// # Safety
/// All five integer fields are borrowed exact tagged words; tzinfo is None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_clock_new(
    h: usize,
    m: usize,
    s: usize,
    us: usize,
    _tzinfo: u8,
    fold: usize,
) -> *const Object {
    let result = (|| {
        let mut parts = [0i64; 5];
        for (i, (value, (name, max))) in [h, m, s, us, fold]
            .into_iter()
            .zip([
                ("hour", 23),
                ("minute", 59),
                ("second", 59),
                ("microsecond", 999999),
                ("fold", 1),
            ])
            .enumerate()
        {
            // SAFETY: the caller roots each exact integer until validation finishes.
            let n = unsafe { Int::borrow(value) }
                .big()
                .to_i64()
                .ok_or_else(|| {
                    Error::new(
                        Kind::OverflowError,
                        "Python int too large to convert to C long",
                    )
                })?;
            let n = i32::try_from(n).map_err(|_| {
                Error::new(
                    Kind::OverflowError,
                    if n < 0 {
                        "signed integer is less than minimum"
                    } else {
                        "signed integer is greater than maximum"
                    },
                )
            })?;
            if !(0..=max).contains(&n) {
                let message = if name == "fold" {
                    format!("fold must be either 0 or 1, not {n}")
                } else {
                    format!("{name} must be in 0..{max}, not {n}")
                };
                return Err(Error::new(Kind::ValueError, message));
            }
            parts[i] = i64::from(n);
        }
        Ok(crate::ObjectOwner::into_raw(crate::ObjectOwner::new(
            Object(StorageCell::new(Storage::ClockTime(Clock(parts)))),
        )))
    })();
    finish(result, std::ptr::null())
}

/// # Safety
/// The caller roots the time and supplies an immediate field index in 0..5.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_clock_part(clock: *const Object, index: usize) -> usize {
    // SAFETY: both the clock and the statically chosen index are exact and live.
    unsafe {
        object(clock).0.with(|s| match s {
            Storage::ClockTime(c) => {
                Int::small(c.0[Int::borrow(index).as_small().unwrap() as usize])
                    .unwrap()
                    .into_raw()
            }
            _ => unreachable!(),
        })
    }
}

/// # Safety
/// Both inputs root immutable naive times; fold does not participate in ordering.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_clock_compare(a: *const Object, b: *const Object) -> i32 {
    // SAFETY: neither time changes during these two shared reads.
    unsafe {
        object(a).0.with(|a| {
            object(b).0.with(|b| match (a, b) {
                (Storage::ClockTime(a), Storage::ClockTime(b)) => a.0[..4].cmp(&b.0[..4]) as i32,
                _ => unreachable!(),
            })
        })
    }
}

/// # Safety
/// Both strings and integers are rooted, and each nonzero optional equals a
/// rooted object of its declared type. The returned handler owns its stream.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_file_new(
    filename: *const Str,
    when: *const Str,
    interval: usize,
    count: usize,
    _encoding: usize,
    delay: bool,
    utc: bool,
    clock: usize,
    _errors: usize,
) -> *const Object {
    // SAFETY: the input is a live UTF-8 path; normalization transfers a string owner.
    let abs = unsafe { posixpath::imre_posix_normalize(filename.cast(), 0, 1) };
    if abs.is_null() {
        return std::ptr::null();
    }
    // SAFETY: all borrowed arguments remain rooted while the handler acquires them.
    let (filename, when, interval, backups, clock) = unsafe {
        (
            StrOwner::from_raw(abs.cast::<Str>()),
            (&*when).to_uppercase(),
            Int::borrow(interval),
            (*Int::borrow(count)).clone(),
            (clock != 0).then(|| crate::ObjectOwner::acquire_word(clock)),
        )
    };
    let result = (|| {
        let stream = if delay {
            None
        } else {
            // SAFETY: opening an exact text stream transfers its owner on success.
            Some(unsafe { crate::ObjectOwner::from_raw(files::open(&filename, 2, false, false)?) })
        };
        let period = Period::parse(&when)?;
        let interval = Int::new(interval.big().as_ref() * period.seconds());
        let mut handler = Handler {
            filename,
            stream,
            when: StrOwner::new(Str::from(when)),
            period,
            interval,
            backups,
            utc,
            delay,
            clock,
            rollover_at: Int::small(0).unwrap(),
        };
        let now = fs::metadata(handler.filename.as_str())
            .map(|s| {
                (i128::from(s.mtime()) * 1_000_000_000 + i128::from(s.mtime_nsec())) / 1_000_000_000
            })
            .unwrap_or_else(|_| {
                i128::from(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                )
            });
        handler.rollover_at = handler.compute_int(&Int::new(BigInt::from(now)))?;
        Ok(crate::ObjectOwner::into_raw(crate::ObjectOwner::new(
            Object(StorageCell::new(Storage::TimedFileHandler(Box::new(
                handler,
            )))),
        )))
    })();
    finish(result, std::ptr::null())
}

macro_rules! read {
    ($name:ident, $ty:ty, |$h:ident| $value:expr) => {
        /// # Safety
        /// The argument is a rooted timed file handler. Resource results transfer an owner.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(handler: *const Object) -> $ty {
            // SAFETY: the caller roots the handler for this nonreentrant read.
            unsafe {
                object(handler).0.with(|s| {
                    let $h = s.timed();
                    $value
                })
            }
        }
    };
}
read!(imre_log_file_when, *const Str, |h| StrOwner::into_raw(
    h.when.clone()
));
read!(imre_log_file_filename, *const Str, |h| StrOwner::into_raw(
    h.filename.clone()
));
read!(imre_log_file_suffix, *const Str, |h| StrOwner::into_raw(
    StrOwner::new(Str::from(h.period.suffix()))
));
read!(imre_log_file_count, usize, |h| h.backups.clone().into_raw());
read!(imre_log_file_interval, usize, |h| h
    .interval
    .clone()
    .into_raw());
read!(imre_log_file_rollover_at, usize, |h| h
    .rollover_at
    .clone()
    .into_raw());
read!(imre_log_file_stream, usize, |h| h
    .stream
    .clone()
    .map_or(0, |s| crate::ObjectOwner::into_raw(s) as usize));
read!(imre_log_file_at_time, usize, |h| h
    .clock
    .clone()
    .map_or(0, |c| crate::ObjectOwner::into_raw(c) as usize));
read!(imre_log_file_utc, bool, |h| h.utc);
read!(imre_log_file_delay, bool, |h| h.delay);
read!(imre_log_file_deletions, *const Object, |h| finish(
    h.files_to_delete().map(crate::ObjectOwner::into_raw),
    std::ptr::null()
));

/// # Safety
/// Both arguments remain rooted for the exact integer computation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_file_compute_int(handler: *const Object, now: usize) -> usize {
    // SAFETY: the caller roots the exact handler and integer through the calculation.
    finish(
        unsafe {
            object(handler)
                .0
                .with(|s| s.timed().compute_int(&Int::borrow(now)))
        }
        .map(Int::into_raw),
        0,
    )
}

/// # Safety
/// The handler remains rooted throughout the calendar lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_file_compute_float(handler: *const Object, now: f64) -> f64 {
    // SAFETY: the caller roots the exact handler through the calculation.
    finish(
        unsafe { object(handler).0.with(|s| s.timed().compute_float(now)) },
        0.0,
    )
}

/// # Safety
/// Root the exact handler until its stream has been detached and closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_file_close(handler: *const Object) {
    // SAFETY: the removed stream owns the descriptor while close updates any external aliases.
    unsafe {
        if let Some(stream) = object(handler).0.with_mut(|s| s.timed_mut().stream.take()) {
            files::imre_text_close(crate::ObjectOwner::as_ptr(&stream));
        }
    }
}

/// # Safety
/// Root the exact handler until the stream flush completes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_file_flush(handler: *const Object) {
    // SAFETY: the retained stream is distinct from its handler; flush cannot reenter.
    unsafe {
        object(handler).0.with(|s| {
            if let Some(stream) = &s.timed().stream {
                files::imre_text_flush(crate::ObjectOwner::as_ptr(stream));
            }
        });
    }
}

/// # Safety
/// Root the timed handler while consulting the file type and adjusting its deadline.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_file_should_roll(handler: *const Object) -> bool {
    // SAFETY: the generated code roots the exact handler throughout this nonreentrant operation.
    finish(
        unsafe { object(handler).0.with_mut(|s| s.timed_mut().should_roll()) },
        false,
    )
}

/// # Safety
/// Root the timed handler across descriptor close, backup removal and reopening.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_file_roll(handler: *const Object) {
    // SAFETY: default rotation has no callbacks; storage is exclusively borrowed until the result.
    finish(
        unsafe { object(handler).0.with_mut(|s| s.timed_mut().roll()) },
        (),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_rollover_preserves_float_rounding() {
        let mut handler = Handler {
            filename: crate::StrOwner::new(Str::default()),
            stream: None,
            when: crate::StrOwner::new(Str::default()),
            period: Period::Week(0),
            interval: Int::new(BigInt::from(7 * DAY)),
            backups: Int::small(0).unwrap(),
            rollover_at: Int::small(0).unwrap(),
            utc: true,
            delay: true,
            clock: None,
        };
        assert_eq!(
            handler.compute_float(3.001332515850663e-11).ok(),
            Some(432000.0)
        );
        handler.period = Period::Midnight;
        handler.interval = Int::new(BigInt::from(17_592_186_044_387_i64) * DAY);
        assert_eq!(
            handler.compute_float(-1.5).ok(),
            Some(1.5199648742349504e18)
        );
    }
}
