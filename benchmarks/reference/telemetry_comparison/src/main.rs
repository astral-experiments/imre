//! Independent telemetry queries with checked native arithmetic and BigInt fallback.

use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};
use std::fs::File;
use std::hint::black_box;
use std::io::{Read, Write};
use std::os::fd::IntoRawFd;

type Result<T> = std::result::Result<T, Error>;

struct Error {
    category: &'static str,
    message: String,
}

impl Error {
    fn value(message: impl Into<String>) -> Self {
        Self {
            category: "ValueError",
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            category: "OSError",
            message: message.into(),
        }
    }
}

unsafe extern "C" {
    fn close(fd: std::ffi::c_int) -> std::ffi::c_int;
}

/// Transfer the sole descriptor owner and report a final close failure.
fn checked_close(file: File, message: &str) -> Result<()> {
    let descriptor = file.into_raw_fd();
    // SAFETY: into_raw_fd transferred exclusive ownership of this live descriptor.
    if unsafe { close(descriptor) } == 0 {
        Ok(())
    } else {
        Err(Error::io(message))
    }
}

/// Parse the application's ASCII grammar without narrowing valid integers.
fn number(token: &str, label: &str) -> Result<BigInt> {
    let digits = token.strip_prefix(['+', '-']).unwrap_or(token);
    if digits.is_empty() || digits.len() > 4300 || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return Err(Error::value(format!("{label}: invalid integer")));
    }
    BigInt::parse_bytes(token.as_bytes(), 10)
        .ok_or_else(|| Error::value(format!("{label}: invalid integer")))
}

/// Match Python's splitlines separators, including CRLF as one boundary.
fn lines(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((position, character)) = characters.next() {
        if matches!(
            character,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            result.push(&text[start..position]);
            start = position + character.len_utf8();
            if character == '\r' && characters.peek().is_some_and(|(_, next)| *next == '\n') {
                characters.next();
                start += 1;
            }
        }
    }
    if start < text.len() {
        result.push(&text[start..]);
    }
    result
}

fn fields(line: &str) -> Vec<&str> {
    line.split(|character: char| {
        character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
    })
    .filter(|part| !part.is_empty())
    .collect()
}

struct Sample<T> {
    timestamp: T,
    value: T,
}

/// Close before parsing; a close failure overrides an earlier read/decode error.
fn load(path: &str, label: &str) -> Result<Vec<Sample<BigInt>>> {
    if path.as_bytes().contains(&0) {
        return Err(Error::value(format!("{label}: invalid path")));
    }
    let message = format!("{label}: cannot read");
    let mut file = File::open(path).map_err(|_| Error::io(&message))?;
    let work = (|| {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| Error::io(&message))?;
        String::from_utf8(bytes).map_err(|_| Error::value(format!("{label}: invalid UTF-8")))
    })();
    checked_close(file, &message)?;
    let text = work?;
    let rows = lines(&text);
    let first = rows
        .first()
        .ok_or_else(|| Error::value(format!("{label}: missing count")))?;
    let first = fields(first);
    if first.len() != 1 {
        return Err(Error::value(format!("{label}: expected count")));
    }
    let count = number(first[0], label)?;
    if count.is_negative() {
        return Err(Error::value(format!("{label}: negative count")));
    }
    // Unaddressable counts still inspect available rows before the EOF error.
    let count = count.to_usize().unwrap_or(usize::MAX);
    let mut result: Vec<Sample<BigInt>> = Vec::new();
    let mut position = 1;
    while result.len() < count {
        let row = rows
            .get(position)
            .ok_or_else(|| Error::value(format!("{label}: truncated")))?;
        let row = fields(row);
        if row.len() != 2 {
            return Err(Error::value(format!(
                "{label}: expected timestamp and value"
            )));
        }
        let timestamp = number(row[0], label)?;
        let value = number(row[1], label)?;
        if result
            .last()
            .is_some_and(|previous| timestamp <= previous.timestamp)
        {
            return Err(Error::value(format!("{label}: timestamps must increase")));
        }
        result.push(Sample { timestamp, value });
        position += 1;
    }
    if position != rows.len() {
        return Err(Error::value(format!("{label}: trailing data")));
    }
    Ok(result)
}

/// Exhaust alignment once, retaining strict zip's first mismatching-row order.
fn align(reference: &[Sample<BigInt>], observed: &[Sample<BigInt>]) -> Result<()> {
    for (expected, actual) in reference.iter().zip(observed) {
        if expected.timestamp != actual.timestamp {
            return Err(Error::value("traces: timestamps differ"));
        }
    }
    if reference.len() > observed.len() {
        return Err(Error::value("zip() argument 2 is shorter than argument 1"));
    }
    if reference.len() < observed.len() {
        return Err(Error::value("zip() argument 2 is longer than argument 1"));
    }
    Ok(())
}

fn native_samples(samples: &[Sample<BigInt>]) -> Option<Vec<Sample<i64>>> {
    samples
        .iter()
        .map(|sample| {
            Some(Sample {
                timestamp: sample.timestamp.to_i64()?,
                value: sample.value.to_i64()?,
            })
        })
        .collect()
}

struct Summary<T> {
    samples: usize,
    signed: T,
    absolute: T,
    squared: T,
    alert: bool,
    within: bool,
    latest: Option<T>,
}

enum Answer<T> {
    Report(Summary<T>),
    Flag(bool),
    Latest(Option<T>),
}

/// Query aligned native records; `None` requests an arbitrary-precision retry.
fn native_query(
    operation: &str,
    reference: &[Sample<i64>],
    observed: &[Sample<i64>],
    threshold: i128,
) -> Option<Answer<i128>> {
    let delta = |expected: &Sample<i64>, actual: &Sample<i64>| {
        i128::from(actual.value) - i128::from(expected.value)
    };
    let alert = || {
        reference
            .iter()
            .zip(observed)
            .any(|(left, right)| delta(left, right).abs() > threshold)
    };
    let within = || {
        reference
            .iter()
            .zip(observed)
            .all(|(left, right)| delta(left, right).abs() <= threshold)
    };
    let latest = || {
        reference
            .iter()
            .rev()
            .zip(observed.iter().rev())
            .find_map(|(left, right)| {
                (delta(left, right).abs() > threshold).then_some(i128::from(left.timestamp))
            })
    };
    Some(match operation {
        "alert" => Answer::Flag(alert()),
        "within" => Answer::Flag(within()),
        "latest" => Answer::Latest(latest()),
        "report" => {
            // Keep the Python program's separate reduction/query passes. Every
            // arithmetic overflow restarts that call with arbitrary precision.
            let signed = reference
                .iter()
                .zip(observed)
                .try_fold(0_i128, |sum, (left, right)| {
                    sum.checked_add(delta(left, right))
                })?;
            let absolute = reference
                .iter()
                .zip(observed)
                .try_fold(0_i128, |sum, (left, right)| {
                    sum.checked_add(delta(left, right).abs())
                })?;
            let squared =
                reference
                    .iter()
                    .zip(observed)
                    .try_fold(0_i128, |sum, (left, right)| {
                        let difference = delta(left, right);
                        sum.checked_add(difference.checked_mul(difference)?)
                    })?;
            Answer::Report(Summary {
                samples: reference.len(),
                signed,
                absolute,
                squared,
                alert: alert(),
                within: within(),
                latest: latest(),
            })
        }
        _ => unreachable!(),
    })
}

/// Evaluate the same independent query passes when native storage cannot suffice.
fn big_query(
    operation: &str,
    reference: &[Sample<BigInt>],
    observed: &[Sample<BigInt>],
    threshold: &BigInt,
) -> Answer<BigInt> {
    let alert = || {
        reference
            .iter()
            .zip(observed)
            .any(|(left, right)| (&right.value - &left.value).abs() > *threshold)
    };
    let within = || {
        reference
            .iter()
            .zip(observed)
            .all(|(left, right)| (&right.value - &left.value).abs() <= *threshold)
    };
    let latest = || {
        reference
            .iter()
            .rev()
            .zip(observed.iter().rev())
            .find_map(|(left, right)| {
                ((&right.value - &left.value).abs() > *threshold).then(|| left.timestamp.clone())
            })
    };
    match operation {
        "alert" => Answer::Flag(alert()),
        "within" => Answer::Flag(within()),
        "latest" => Answer::Latest(latest()),
        "report" => {
            let signed = reference
                .iter()
                .zip(observed)
                .map(|(left, right)| &right.value - &left.value)
                .sum();
            let absolute = reference
                .iter()
                .zip(observed)
                .map(|(left, right)| (&right.value - &left.value).abs())
                .sum();
            let squared = reference
                .iter()
                .zip(observed)
                .map(|(left, right)| {
                    let difference = &right.value - &left.value;
                    &difference * &difference
                })
                .sum();
            Answer::Report(Summary {
                samples: reference.len(),
                signed,
                absolute,
                squared,
                alert: alert(),
                within: within(),
                latest: latest(),
            })
        }
        _ => unreachable!(),
    }
}

enum Integer {
    Small(i128),
    Big(BigInt),
}

impl Integer {
    fn add_native(&mut self, value: i128) {
        match self {
            Self::Small(current) => {
                if let Some(result) = current.checked_add(value) {
                    *current = result;
                } else {
                    *self = Self::Big(BigInt::from(*current) + value);
                }
            }
            Self::Big(current) => *current += value,
        }
    }

    fn add_big(&mut self, value: BigInt) {
        match self {
            Self::Small(current) => *self = Self::Big(BigInt::from(*current) + value),
            Self::Big(current) => *current += value,
        }
    }
}

impl std::fmt::Display for Integer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Small(value) => value.fmt(formatter),
            Self::Big(value) => value.fmt(formatter),
        }
    }
}

fn add_native(answer: &Answer<i128>, checksum: &mut Integer) {
    match answer {
        Answer::Report(summary) => {
            checksum.add_native(summary.signed);
            checksum.add_native(summary.absolute);
            checksum.add_native(summary.squared);
        }
        Answer::Flag(flag) => checksum.add_native(i128::from(*flag)),
        Answer::Latest(timestamp) => checksum.add_native(timestamp.unwrap_or(0)),
    }
}

fn add_big(answer: &Answer<BigInt>, checksum: &mut Integer) {
    match answer {
        Answer::Report(summary) => {
            checksum.add_big(&summary.signed + &summary.absolute + &summary.squared)
        }
        Answer::Flag(flag) => checksum.add_native(i128::from(*flag)),
        Answer::Latest(timestamp) => {
            if let Some(value) = timestamp {
                checksum.add_big(value.clone());
            }
        }
    }
}

fn boolean(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

/// Match CPython's default decimal-output limit before opening the report.
fn decimal<T: std::fmt::Display>(value: &T) -> Result<String> {
    let text = value.to_string();
    if text.strip_prefix('-').unwrap_or(&text).len() > 4300 {
        return Err(Error::value(
            "Exceeds the limit (4300 digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit",
        ));
    }
    Ok(text)
}

fn optional<T: std::fmt::Display>(value: &Option<T>) -> Result<String> {
    match value {
        Some(value) => decimal(value),
        None => Ok("None".to_owned()),
    }
}

fn render<T: std::fmt::Display>(answer: &Answer<T>) -> Result<String> {
    Ok(match answer {
        Answer::Flag(value) => format!("{}\n", boolean(*value)),
        Answer::Latest(value) => format!("{}\n", optional(value)?),
        Answer::Report(summary) => format!(
            "TELEMETRY\nSAMPLES {}\nSIGNED_ERROR {}\nABSOLUTE_ERROR {}\nSQUARED_ERROR {}\nALERT {}\nWITHIN {}\nLATEST {}\nEND\n",
            summary.samples,
            decimal(&summary.signed)?,
            decimal(&summary.absolute)?,
            decimal(&summary.squared)?,
            boolean(summary.alert),
            boolean(summary.within),
            optional(&summary.latest)?
        ),
    })
}

fn run(arguments: &[String]) -> Result<String> {
    if arguments.len() != 7 {
        return Err(Error::value(
            "usage: telemetry_comparison MODE REFERENCE OBSERVED REPORT THRESHOLD REPEATS",
        ));
    }
    let threshold = number(&arguments[5], "threshold")?;
    let repetitions = number(&arguments[6], "repetitions")?;
    if threshold.is_negative() {
        return Err(Error::value("threshold must be nonnegative"));
    }
    if repetitions <= BigInt::zero() {
        return Err(Error::value("repetitions must be positive"));
    }
    let reference = load(&arguments[2], "reference")?;
    let observed = load(&arguments[3], "observed")?;
    align(&reference, &observed)?;
    let operation = arguments[1].as_str();
    if !matches!(operation, "report" | "alert" | "within" | "latest") {
        return Err(Error::value("unknown telemetry operation"));
    }
    let native = native_samples(&reference)
        .zip(native_samples(&observed))
        .zip(threshold.to_i128());
    let mut checksum = Integer::Small(0);
    let mut final_native = None;
    let mut final_big = None;
    // The CLI accepts arbitrary-precision repetition counts just as Python does.
    // Chunking avoids converting a valid but unaddressable count to a smaller one.
    let mut remaining = repetitions;
    while remaining > BigInt::zero() {
        let count = remaining.to_u64().unwrap_or(u64::MAX);
        for _ in 0..count {
            if let Some(((left, right), limit)) = &native {
                if let Some(answer) = native_query(
                    operation,
                    black_box(left),
                    black_box(right),
                    black_box(*limit),
                ) {
                    let answer = black_box(answer);
                    add_native(&answer, &mut checksum);
                    final_native = Some(answer);
                    final_big = None;
                    continue;
                }
            }
            let answer = black_box(big_query(
                operation,
                black_box(&reference),
                black_box(&observed),
                black_box(&threshold),
            ));
            add_big(&answer, &mut checksum);
            final_big = Some(answer);
            final_native = None;
        }
        remaining -= count;
    }
    let result = if let Some(answer) = final_native {
        render(&answer)?
    } else {
        render(&final_big.unwrap())?
    };
    let checksum = decimal(&checksum)?;
    if arguments[4].as_bytes().contains(&0) {
        return Err(Error::value("report: invalid path"));
    }
    let mut output = File::create(&arguments[4]).map_err(|_| Error::io("report: cannot write"))?;
    let work = output
        .write_all(result.as_bytes())
        .and_then(|_| output.flush());
    checked_close(output, "report: cannot write")?;
    work.map_err(|_| Error::io("report: cannot write"))?;
    Ok(checksum)
}

fn main() {
    match run(&std::env::args().collect::<Vec<_>>()) {
        Ok(checksum) => println!("OK\n{checksum}"),
        Err(error) => println!("ERROR\n{}\n{}", error.category, error.message),
    }
}
