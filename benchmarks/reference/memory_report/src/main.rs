//! Independent, schema-directed implementation of the two-file memory report.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};

use num_bigint::{BigInt, BigUint, Sign};
use num_traits::{ToPrimitive, Zero};
use serde::{Deserialize, Deserializer, de};
use serde_json::value::RawValue;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Integer {
    Small(i64),
    Big(BigInt),
}

impl Default for Integer {
    fn default() -> Self {
        Self::Small(0)
    }
}

impl Integer {
    fn from_big(value: BigInt) -> Self {
        value.to_i64().map_or_else(|| Self::Big(value), Self::Small)
    }

    fn big(&self) -> BigInt {
        match self {
            Self::Small(value) => BigInt::from(*value),
            Self::Big(value) => value.clone(),
        }
    }

    fn add(&self, other: &Self) -> Self {
        if let (Self::Small(left), Self::Small(right)) = (self, other)
            && let Some(value) = left.checked_add(*right)
        {
            return Self::Small(value);
        }
        Self::from_big(self.big() + other.big())
    }

    fn subtract(&self, other: &Self) -> Self {
        if let (Self::Small(left), Self::Small(right)) = (self, other)
            && let Some(value) = left.checked_sub(*right)
        {
            return Self::Small(value);
        }
        Self::from_big(self.big() - other.big())
    }

    fn magnitude(&self) -> Self {
        match self {
            Self::Small(value) => value
                .checked_abs()
                .map_or_else(|| Self::Big(-BigInt::from(*value)), Self::Small),
            Self::Big(value) if value.sign() == Sign::Minus => Self::Big(-value),
            Self::Big(value) => Self::Big(value.clone()),
        }
    }

    fn float(&self) -> Result<f64, Failure> {
        let value = match self {
            Self::Small(value) => *value as f64,
            Self::Big(value) => value.to_f64().unwrap_or(f64::INFINITY),
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(Failure(
                "OverflowError",
                "integer is too large for float".into(),
            ))
        }
    }
}

impl Ord for Integer {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Small(left), Self::Small(right)) => left.cmp(right),
            _ => self.big().cmp(&other.big()),
        }
    }
}

impl PartialOrd for Integer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'de> Deserialize<'de> for Integer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Borrow the validated token instead of converting through f64 or a
        // serde_json::Value. Objects cannot impersonate private number tokens.
        let raw = <&RawValue>::deserialize(deserializer)?.get();
        let digits = raw.strip_prefix('-').unwrap_or(raw);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(de::Error::custom("expected an integer JSON token"));
        }
        if let Ok(value) = raw.parse::<i64>() {
            return Ok(Self::Small(value));
        }
        BigInt::parse_bytes(raw.as_bytes(), 10)
            .map(Self::from_big)
            .ok_or_else(|| de::Error::custom("invalid JSON integer"))
    }
}

trait Count: Clone + Default + Ord {
    type Magnitude: Ord;
    fn add(&self, other: &Self) -> Option<Self>;
    fn subtract(&self, other: &Self) -> Option<Self>;
    fn magnitude(&self) -> Self::Magnitude;
    fn integer(&self) -> Integer;
}

impl Count for i64 {
    type Magnitude = u64;
    fn add(&self, other: &Self) -> Option<Self> {
        self.checked_add(*other)
    }
    fn subtract(&self, other: &Self) -> Option<Self> {
        self.checked_sub(*other)
    }
    fn magnitude(&self) -> u64 {
        self.unsigned_abs()
    }
    fn integer(&self) -> Integer {
        Integer::Small(*self)
    }
}

impl Count for Integer {
    type Magnitude = Integer;
    fn add(&self, other: &Self) -> Option<Self> {
        Some(Integer::add(self, other))
    }
    fn subtract(&self, other: &Self) -> Option<Self> {
        Some(Integer::subtract(self, other))
    }
    fn magnitude(&self) -> Integer {
        Integer::magnitude(self)
    }
    fn integer(&self) -> Integer {
        self.clone()
    }
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "N: Deserialize<'de> + Default"))]
struct MemoryItem<N> {
    name: String,
    #[serde(default)]
    metadata_bytes: N,
    #[serde(default)]
    fields_bytes: N,
}

impl<N: Count> MemoryItem<N> {
    fn total(&self) -> Option<N> {
        self.metadata_bytes.add(&self.fields_bytes)
    }
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "N: Deserialize<'de> + Default"))]
#[allow(dead_code)]
struct MemoryReport<N> {
    total_bytes: N,
    struct_metadata_bytes: N,
    struct_fields_bytes: N,
    memo_metadata_bytes: N,
    memo_fields_bytes: N,
    #[serde(default)]
    structs: Vec<MemoryItem<N>>,
    #[serde(default)]
    queries: Vec<MemoryItem<N>>,
}

struct Difference<'a, N: Count> {
    name: &'a str,
    old: N,
    new: N,
    magnitude: N::Magnitude,
}

#[derive(Debug)]
struct Failure(&'static str, String);

fn depth_check(text: &str) -> Result<(), Failure> {
    let mut depth = 0_i32;
    let (mut quoted, mut escaped) = (false, false);
    for byte in text.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b'[' || byte == b'{' {
            depth += 1;
            if depth > 128 {
                return Err(Failure(
                    "ValueError",
                    "JSON nesting exceeds 128 levels".into(),
                ));
            }
        } else if byte == b']' || byte == b'}' {
            depth -= 1;
        }
    }
    Ok(())
}

fn read(path: &str) -> Result<String, Failure> {
    let bytes = fs::read(path).map_err(|error| Failure("OSError", error.to_string()))?;
    let text =
        String::from_utf8(bytes).map_err(|error| Failure("ValueError", error.to_string()))?;
    depth_check(&text)?;
    Ok(text)
}

fn decode<'de, N: Deserialize<'de> + Default>(text: &'de str) -> Result<MemoryReport<N>, Failure> {
    let mut decoder = serde_json::Deserializer::from_str(text);
    decoder.disable_recursion_limit();
    let report = MemoryReport::deserialize(&mut decoder)
        .map_err(|error| Failure("ValueError", error.to_string()))?;
    decoder
        .end()
        .map_err(|error| Failure("ValueError", error.to_string()))?;
    Ok(report)
}

fn differences<'a, N: Count>(
    old: &'a MemoryReport<N>,
    new: &'a MemoryReport<N>,
) -> Option<Vec<Difference<'a, N>>> {
    let old_count = old.structs.len() + old.queries.len();
    let new_count = new.structs.len() + new.queries.len();
    let mut old_by_name = HashMap::with_capacity(old_count);
    let mut new_by_name = HashMap::with_capacity(new_count);
    let mut names = Vec::with_capacity(old_count + new_count);
    for item in old.structs.iter().chain(&old.queries) {
        if old_by_name.insert(item.name.as_str(), item).is_none() {
            names.push(item.name.as_str());
        }
    }
    for item in new.structs.iter().chain(&new.queries) {
        if new_by_name.insert(item.name.as_str(), item).is_none()
            && !old_by_name.contains_key(item.name.as_str())
        {
            names.push(item.name.as_str());
        }
    }
    let mut result = Vec::with_capacity(names.len());
    for name in names {
        let old = old_by_name
            .get(name)
            .map_or_else(|| Some(N::default()), |item| item.total())?;
        let new = new_by_name
            .get(name)
            .map_or_else(|| Some(N::default()), |item| item.total())?;
        if old != new {
            let magnitude = new.subtract(&old)?.magnitude();
            result.push(Difference {
                name,
                old,
                new,
                magnitude,
            });
        }
    }
    result.sort_by(|left, right| right.magnitude.cmp(&left.magnitude));
    Some(result)
}

fn format_bytes(value: &Integer) -> Result<String, Failure> {
    let mut value = value.float()?;
    for unit in ["B", "kB", "MB"] {
        if value.abs() < 1024.0 {
            return Ok(format!("{value:.2}{unit}"));
        }
        value /= 1024.0;
    }
    Ok(format!("{value:.2}GB"))
}

fn round_quotient(numerator: BigUint, denominator: BigUint) -> u64 {
    let quotient = &numerator / &denominator;
    let remainder = numerator % &denominator;
    let mut result = quotient.to_u64().expect("at most 53 significant bits");
    let twice = remainder << 1_u32;
    if twice > denominator || (twice == denominator && result & 1 != 0) {
        result += 1;
    }
    result
}

fn ratio(numerator: &Integer, denominator: &Integer) -> Result<f64, Failure> {
    if let (Integer::Small(left), Integer::Small(right)) = (numerator, denominator)
        && left.unsigned_abs() <= 1 << 53
        && right.unsigned_abs() <= 1 << 53
    {
        return Ok(*left as f64 / *right as f64);
    }
    // Round the exact rational to binary64, including ties and subnormals.
    // Converting numerator and denominator separately would double-round.
    let left = numerator.big();
    let right = denominator.big();
    let sign = u64::from(left.sign() != right.sign()) << 63;
    let (left, right) = (left.magnitude(), right.magnitude());
    if left.is_zero() {
        return Ok(f64::from_bits(sign));
    }
    let mut exponent = left.bits() as i64 - right.bits() as i64;
    let below = if exponent >= 0 {
        left < &(right << exponent as usize)
    } else {
        &(left << (-exponent) as usize) < right
    };
    exponent -= i64::from(below);
    if exponent > 1023 {
        return Err(Failure(
            "OverflowError",
            "integer division result is too large".into(),
        ));
    }
    let shift = if exponent < -1022 {
        1074
    } else {
        52 - exponent
    };
    let mut significand = if shift >= 0 {
        round_quotient(left << shift as usize, right.clone())
    } else {
        round_quotient(left.clone(), right << (-shift) as usize)
    };
    if exponent < -1022 {
        return Ok(f64::from_bits(sign | significand));
    }
    if significand == 1 << 53 {
        significand >>= 1;
        exponent += 1;
    }
    if exponent > 1023 {
        return Err(Failure(
            "OverflowError",
            "integer division result is too large".into(),
        ));
    }
    Ok(f64::from_bits(
        sign | ((exponent + 1023) as u64) << 52 | (significand - (1 << 52)),
    ))
}

fn format_diff(old: &Integer, new: &Integer) -> Result<String, Failure> {
    let difference = new.subtract(old);
    let zero = Integer::default();
    if difference == zero {
        return Ok("-".into());
    }
    let sign = if difference > zero { "+" } else { "" };
    if *old == zero {
        return Ok(format!("{sign}{} (new)", format_bytes(&difference)?));
    }
    let percent = ratio(&difference, old)? * 100.0;
    Ok(format!(
        "{sign}{percent:.2}% ({})",
        format_bytes(&difference.magnitude())?
    ))
}

fn outcome(old: &Integer, new: &Integer) -> &'static str {
    match new.cmp(old) {
        Ordering::Greater => "⏫",
        Ordering::Less => "⬇️",
        Ordering::Equal => "✅",
    }
}

fn render<N: Count>(
    name: &str,
    old: &MemoryReport<N>,
    new: &MemoryReport<N>,
) -> Result<Option<String>, Failure> {
    let mut lines = vec!["## Memory usage report".to_owned(), String::new()];
    let Some(change) = new.total_bytes.subtract(&old.total_bytes) else {
        return Ok(None);
    };
    if change.integer().magnitude() <= Integer::Small(10_000) {
        lines.push("Memory usage unchanged ✅".into());
        return Ok(Some(lines.join("\n")));
    }
    lines.extend(
        [
            "### Summary",
            "",
            "| Project | Old | New | Diff | Outcome |",
            "|---------|-----|-----|------|---------|",
        ]
        .map(str::to_owned),
    );
    lines.push(format!(
        "| {name} | {} | {} | {} | {} |",
        format_bytes(&old.total_bytes.integer())?,
        format_bytes(&new.total_bytes.integer())?,
        format_diff(&old.total_bytes.integer(), &new.total_bytes.integer())?,
        outcome(&old.total_bytes.integer(), &new.total_bytes.integer())
    ));
    lines.extend(
        [
            "",
            "### Significant changes",
            "",
            "<details>",
            "<summary>Click to expand detailed breakdown</summary>",
            "",
        ]
        .map(str::to_owned),
    );
    let Some(differences) = differences(old, new) else {
        return Ok(None);
    };
    if !differences.is_empty() {
        lines.extend([
            format!("### {name}"),
            String::new(),
            "| Name | Old | New | Diff | Outcome |".into(),
            "|------|-----|-----|------|---------|".into(),
        ]);
        for item in differences.iter().take(15) {
            lines.push(format!(
                "| `{}` | {} | {} | {} |{} |",
                item.name.replace(' ', ""),
                format_bytes(&item.old.integer())?,
                format_bytes(&item.new.integer())?,
                format_diff(&item.old.integer(), &item.new.integer())?,
                outcome(&item.old.integer(), &item.new.integer())
            ));
        }
        if differences.len() > 15 {
            lines.push(format!("| ... | | | *{} more* |", differences.len() - 15));
        }
        lines.push(String::new());
    }
    lines.extend(["</details>".into(), String::new()]);
    Ok(Some(lines.join("\n")))
}

fn project_name(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let mut suffix = None;
    let mut has_non_dot = false;
    for (index, character) in name.char_indices() {
        if character != '.' {
            has_non_dot = true;
        } else if has_non_dot {
            suffix = Some(index);
        }
    }
    suffix.map_or(name, |index| &name[..index])
}

fn run() -> Result<(), Failure> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut index = usize::from(
        arguments
            .first()
            .is_some_and(|argument| argument == "compare"),
    );
    let (mut old_path, mut new_path, mut output_path) = ("", "", None);
    while index < arguments.len() {
        let flag = &arguments[index];
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure("ValueError", format!("expected a value after {flag}")))?;
        match flag.as_str() {
            "--old" => old_path = value,
            "--new" => new_path = value,
            "--output" => output_path = Some(value),
            _ => return Err(Failure("ValueError", format!("unknown argument: {flag}"))),
        }
        index += 2;
    }
    if old_path.is_empty() || new_path.is_empty() {
        return Err(Failure(
            "ValueError",
            "expected --old OLD.json --new NEW.json [--output REPORT.md]".into(),
        ));
    }
    // The ordinary path stores only native i64 fields in contiguous records.
    // Retain input text so parsing or arithmetic overflow can retry with BigInt.
    let old_text = read(old_path)?;
    let old_native = decode::<i64>(&old_text).ok();
    let old_big = if old_native.is_none() {
        Some(decode::<Integer>(&old_text)?)
    } else {
        None
    };
    let new_text = read(new_path)?;
    let new_native = decode::<i64>(&new_text).ok();
    let new_big = if new_native.is_none() {
        Some(decode::<Integer>(&new_text)?)
    } else {
        None
    };
    let name = project_name(new_path);
    let native_result = if let (Some(old), Some(new)) = (&old_native, &new_native) {
        render(name, old, new)?
    } else {
        None
    };
    let rendered = if let Some(rendered) = native_result {
        rendered
    } else {
        let old = match old_big {
            Some(value) => value,
            None => decode::<Integer>(&old_text)?,
        };
        let new = match new_big {
            Some(value) => value,
            None => decode::<Integer>(&new_text)?,
        };
        render(name, &old, &new)?.expect("arbitrary-precision arithmetic cannot overflow")
    };
    if let Some(path) = output_path {
        fs::write(path, rendered).map_err(|error| Failure("OSError", error.to_string()))?;
    } else {
        let mut output = io::stdout().lock();
        writeln!(output, "{rendered}").map_err(|error| Failure("OSError", error.to_string()))?;
        output
            .flush()
            .map_err(|error| Failure("OSError", error.to_string()))?;
    }
    Ok(())
}

fn main() {
    if let Err(Failure(kind, message)) = run() {
        eprintln!("{kind}: {message}");
        std::process::exit(1);
    }
}
