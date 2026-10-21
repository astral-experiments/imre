//! Independent payout pipeline with checked i64 arithmetic and BigInt fallback.

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

struct Record<T> {
    identifier: T,
    amount: T,
    risk: T,
}

fn load(path: &str) -> Result<Vec<Record<BigInt>>> {
    if path.as_bytes().contains(&0) {
        return Err(Error::value("records: invalid path"));
    }
    let mut file = File::open(path).map_err(|_| Error::io("records: cannot read"))?;
    let work = (|| {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| Error::io("records: cannot read"))?;
        String::from_utf8(bytes).map_err(|_| Error::value("records: invalid UTF-8"))
    })();
    checked_close(file, "records: cannot read")?;
    let text = work?;
    let rows = lines(&text);
    let first = fields(
        rows.first()
            .ok_or_else(|| Error::value("records: missing count"))?,
    );
    if first.len() != 1 {
        return Err(Error::value("records: expected count"));
    }
    let count = number(first[0], "records")?;
    if count.is_negative() {
        return Err(Error::value("records: negative count"));
    }
    let count = count.to_usize().unwrap_or(usize::MAX);
    let mut result: Vec<Record<BigInt>> = Vec::new();
    let mut position = 1;
    while result.len() < count {
        let row = fields(
            rows.get(position)
                .ok_or_else(|| Error::value("records: truncated"))?,
        );
        if row.len() != 3 {
            return Err(Error::value("records: expected id, amount and risk"));
        }
        let identifier = number(row[0], "records")?;
        let amount = number(row[1], "records")?;
        let risk = number(row[2], "records")?;
        if identifier <= BigInt::zero() {
            return Err(Error::value("records: id must be positive"));
        }
        if result
            .last()
            .is_some_and(|previous| identifier <= previous.identifier)
        {
            return Err(Error::value("records: ids must increase"));
        }
        if risk.is_negative() {
            return Err(Error::value("records: risk must be nonnegative"));
        }
        result.push(Record {
            identifier,
            amount,
            risk,
        });
        position += 1;
    }
    if position != rows.len() {
        return Err(Error::value("records: trailing data"));
    }
    Ok(result)
}

struct Summary<T> {
    count: usize,
    total: T,
    largest: T,
}

/// A failed checked operation requests a pure-query retry with arbitrary precision.
fn native_query(records: &[Record<i64>], limit: i64, fee: i64) -> Option<Summary<i64>> {
    let mut summary = Summary {
        count: 0,
        total: 0_i64,
        largest: 0,
    };
    for amount in records
        .iter()
        .filter(|record| record.risk <= limit)
        .map(|record| record.amount.checked_sub(fee))
    {
        let amount = amount?;
        if amount > 0 {
            summary.count += 1;
            summary.total = summary.total.checked_add(amount)?;
            summary.largest = summary.largest.max(amount);
        }
    }
    Some(summary)
}

fn big_query(records: &[Record<BigInt>], limit: &BigInt, fee: &BigInt) -> Summary<BigInt> {
    let mut summary = Summary {
        count: 0,
        total: BigInt::zero(),
        largest: BigInt::zero(),
    };
    for amount in records
        .iter()
        .filter(|record| &record.risk <= limit)
        .map(|record| &record.amount - fee)
        .filter(|amount| amount > &BigInt::zero())
    {
        summary.count += 1;
        summary.total += &amount;
        if amount > summary.largest {
            summary.largest = amount;
        }
    }
    summary
}

enum Integer {
    Small(i64),
    Big(BigInt),
}

impl Integer {
    fn add_native(&mut self, value: i64) {
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

fn decimal(value: &impl std::fmt::Display) -> Result<String> {
    let text = value.to_string();
    if text.strip_prefix('-').unwrap_or(&text).len() > 4300 {
        return Err(Error::value(
            "Exceeds the limit (4300 digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit",
        ));
    }
    Ok(text)
}

fn render(summary: &Summary<impl std::fmt::Display>) -> Result<String> {
    Ok(format!(
        "PAYOUTS\nCOUNT {}\nTOTAL {}\nLARGEST {}\nEND\n",
        summary.count,
        decimal(&summary.total)?,
        decimal(&summary.largest)?
    ))
}

fn run(arguments: &[String]) -> Result<String> {
    if arguments.len() != 7 {
        return Err(Error::value(
            "usage: record_pipeline MODE INPUT REPORT LIMIT FEE REPEATS",
        ));
    }
    let limit = number(&arguments[4], "limit")?;
    let fee = number(&arguments[5], "fee")?;
    let repetitions = number(&arguments[6], "repetitions")?;
    if limit.is_negative() || fee.is_negative() {
        return Err(Error::value("limit and fee must be nonnegative"));
    }
    if repetitions <= BigInt::zero() {
        return Err(Error::value("repetitions must be positive"));
    }
    let effective_fee = match arguments[1].as_str() {
        "gross" => BigInt::zero(),
        "net" => fee,
        _ => return Err(Error::value("unknown payout mode")),
    };
    let records = load(&arguments[2])?;
    let native: Option<Vec<Record<i64>>> = records
        .iter()
        .map(|record| {
            Some(Record {
                identifier: record.identifier.to_i64()?,
                amount: record.amount.to_i64()?,
                risk: record.risk.to_i64()?,
            })
        })
        .collect();
    let native = native.zip(limit.to_i64()).zip(effective_fee.to_i64());
    let mut checksum = Integer::Small(0);
    let mut final_native = None;
    let mut final_big = None;
    let mut remaining = repetitions;
    while remaining > BigInt::zero() {
        let count = remaining.to_u64().unwrap_or(u64::MAX);
        for _ in 0..count {
            if let Some(((records, limit), fee)) = &native {
                if let Some(answer) =
                    native_query(black_box(records), black_box(*limit), black_box(*fee))
                {
                    let answer = black_box(answer);
                    checksum
                        .add_native(i64::try_from(answer.count).expect("addressable record count"));
                    checksum.add_native(answer.total);
                    checksum.add_native(answer.largest);
                    final_native = Some(answer);
                    final_big = None;
                    continue;
                }
            }
            let answer = black_box(big_query(
                black_box(&records),
                black_box(&limit),
                black_box(&effective_fee),
            ));
            checksum.add_big(BigInt::from(answer.count) + &answer.total + &answer.largest);
            final_big = Some(answer);
            final_native = None;
        }
        remaining -= count;
    }
    let report = if let Some(answer) = final_native {
        render(&answer)?
    } else {
        render(&final_big.unwrap())?
    };
    let checksum = decimal(&checksum)?;
    if arguments[3].as_bytes().contains(&0) {
        return Err(Error::value("report: invalid path"));
    }
    let mut output = File::create(&arguments[3]).map_err(|_| Error::io("report: cannot write"))?;
    let work = output
        .write_all(report.as_bytes())
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
