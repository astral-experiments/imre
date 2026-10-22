//! Independent, schema-directed implementation of file and directory memory reports.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::{CString, c_char, c_int};
use std::fs;
use std::io::{self, Write};
use std::os::fd::IntoRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    let bytes =
        fs::read(normalize_path(path)).map_err(|error| Failure("OSError", error.to_string()))?;
    let text =
        String::from_utf8(bytes).map_err(|error| Failure("ValueError", error.to_string()))?;
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

struct Project<'a, N> {
    name: &'a str,
    old: &'a MemoryReport<N>,
    new: &'a MemoryReport<N>,
}

fn render<N: Count>(projects: &[Project<'_, N>]) -> Result<Option<String>, Failure> {
    if projects.is_empty() {
        return Ok(Some("No memory reports to compare.".into()));
    }
    let mut ranked = Vec::with_capacity(projects.len());
    for project in projects {
        let Some(change) = project.new.total_bytes.subtract(&project.old.total_bytes) else {
            return Ok(None);
        };
        ranked.push((project, change));
    }
    ranked.sort_by(|left, right| right.1.cmp(&left.1));
    let mut lines = vec!["## Memory usage report".to_owned(), String::new()];
    if !ranked
        .iter()
        .any(|(_, change)| change.integer().magnitude() > Integer::Small(10_000))
    {
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
    for (project, _) in &ranked {
        let (name, old, new) = (project.name, project.old, project.new);
        lines.push(format!(
            "| {name} | {} | {} | {} | {} |",
            format_bytes(&old.total_bytes.integer())?,
            format_bytes(&new.total_bytes.integer())?,
            format_diff(&old.total_bytes.integer(), &new.total_bytes.integer())?,
            outcome(&old.total_bytes.integer(), &new.total_bytes.integer())
        ));
    }
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
    for (project, _) in &ranked {
        let Some(differences) = differences(project.old, project.new) else {
            return Ok(None);
        };
        if differences.is_empty() {
            continue;
        }
        lines.extend([
            format!("### {}", project.name),
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

/// POSIX Path removes empty and dot components, but preserves dot-dot and the
/// special double-slash root without resolving symlinks or touching the disk.
fn normalize_path(path: &str) -> String {
    let root = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else if path.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<_> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() && root.is_empty() {
        ".".into()
    } else {
        format!("{root}{}", parts.join("/"))
    }
}

struct Input<'a> {
    text: Cow<'a, str>,
    native: Option<MemoryReport<i64>>,
    big: Option<MemoryReport<Integer>>,
}

impl<'a> Input<'a> {
    fn parse(text: impl Into<Cow<'a, str>>) -> Result<Self, Failure> {
        let text = text.into();
        depth_check(&text)?;
        let native = decode::<i64>(&text).ok();
        let big = if native.is_none() {
            Some(decode::<Integer>(&text)?)
        } else {
            None
        };
        Ok(Self { text, native, big })
    }

    fn into_big(self) -> Result<MemoryReport<Integer>, Failure> {
        self.big.map_or_else(|| decode::<Integer>(&self.text), Ok)
    }
}

/// Glob follows pathlib's nonrecursive *.json selection, including hidden files
/// and matching directories. Directory scanning errors are suppressed by glob;
/// errors opening or decoding a selected entry still propagate.
fn load_directory(path: &str) -> Result<HashMap<String, Input<'static>>, Failure> {
    let mut reports = HashMap::new();
    let Ok(entries) =
        fs::read_dir(path).and_then(|entries| entries.collect::<io::Result<Vec<_>>>())
    else {
        return Ok(reports);
    };
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Failure("ValueError", "path is not UTF-8".into()))?;
        if name.ends_with(".json") {
            let path = entry.path();
            let path = path
                .to_str()
                .ok_or_else(|| Failure("ValueError", "path is not UTF-8".into()))?;
            reports.insert(project_name(&name).to_owned(), Input::parse(read(path)?)?);
        }
    }
    Ok(reports)
}

/// The common path stores only native i64 fields. Retained source text permits
/// an exact BigInt retry if any parsed field or later arithmetic needs it.
fn render_inputs(inputs: Vec<(String, Input<'_>, Input<'_>)>) -> Result<String, Failure> {
    let native: Option<Vec<_>> = inputs
        .iter()
        .map(|(name, old, new)| {
            Some(Project {
                name,
                old: old.native.as_ref()?,
                new: new.native.as_ref()?,
            })
        })
        .collect();
    if let Some(projects) = native
        && let Some(rendered) = render(&projects)?
    {
        return Ok(rendered);
    }
    let mut values = Vec::with_capacity(inputs.len());
    for (name, old, new) in inputs {
        values.push((name, old.into_big()?, new.into_big()?));
    }
    let projects: Vec<_> = values
        .iter()
        .map(|(name, old, new)| Project { name, old, new })
        .collect();
    Ok(render(&projects)?.expect("arbitrary-precision arithmetic cannot overflow"))
}

/// Project names are sorted before stable ranking, exactly as in directory mode.
fn common_inputs<'a>(
    mut old: HashMap<String, Input<'a>>,
    mut new: HashMap<String, Input<'a>>,
) -> Vec<(String, Input<'a>, Input<'a>)> {
    let mut names: Vec<_> = old
        .keys()
        .filter(|name| new.contains_key(*name))
        .cloned()
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let before = old.remove(&name).unwrap();
            let after = new.remove(&name).unwrap();
            (name, before, after)
        })
        .collect()
}

fn preload(path: &str) -> Result<HashMap<String, String>, Failure> {
    let mut texts = HashMap::new();
    let Ok(entries) =
        fs::read_dir(path).and_then(|entries| entries.collect::<io::Result<Vec<_>>>())
    else {
        return Ok(texts);
    };
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Failure("ValueError", "path is not UTF-8".into()))?;
        if name.ends_with(".json") {
            let path = entry.path();
            texts.insert(
                project_name(&name).to_owned(),
                read(
                    path.to_str()
                        .ok_or_else(|| Failure("ValueError", "path is not UTF-8".into()))?,
                )?,
            );
        }
    }
    Ok(texts)
}

fn parse_preloaded(texts: &HashMap<String, String>) -> Result<HashMap<String, Input<'_>>, Failure> {
    texts
        .iter()
        .map(|(name, text)| {
            Ok((
                name.clone(),
                Input::parse(std::hint::black_box(text.as_str()))?,
            ))
        })
        .collect()
}

/// Repeats the same parse/compare/render work over immutable preloaded text.
/// Black boxes prevent the independent native baseline from hoisting a result;
/// the checksum and complete last report are checked outside the timed process.
fn compute(rounds: usize, old_directory: &str, new_directory: &str) -> Result<(), Failure> {
    let old_texts = preload(old_directory)?;
    let new_texts = preload(new_directory)?;
    let (mut checksum, mut rendered) = (0_usize, String::new());
    for _ in 0..rounds {
        let old = parse_preloaded(&old_texts)?;
        let new = parse_preloaded(&new_texts)?;
        rendered = render_inputs(common_inputs(old, new))?;
        checksum += std::hint::black_box(rendered.chars().count());
    }
    let mut output = io::stdout().lock();
    writeln!(output, "{checksum}\n{rendered}")
        .map_err(|error| Failure("OSError", error.to_string()))?;
    output
        .flush()
        .map_err(|error| Failure("OSError", error.to_string()))?;
    Ok(())
}

#[derive(Deserialize)]
struct PreparedProject {
    name: String,
    cwd: String,
    argv: Vec<String>,
}

#[derive(Deserialize)]
struct PreparedProjects {
    projects: Vec<PreparedProject>,
}

unsafe extern "C" {
    fn mkdtemp(template: *mut c_char) -> *mut c_char;
    fn close(fd: c_int) -> c_int;
}

/// A private sibling workspace with explicit, fallible normal-path cleanup.
struct Workspace {
    path: PathBuf,
    clean_on_drop: bool,
}

impl Workspace {
    /// Let libc exclusively create a mode-0700 directory, subject to umask.
    fn create(parent: &Path, prefix: &str) -> Result<Self, Failure> {
        let template = parent.join(format!("{prefix}XXXXXX"));
        let mut template = CString::new(template.as_os_str().as_bytes())
            .map_err(|error| Failure("ValueError", error.to_string()))?
            .into_bytes_with_nul();
        // SAFETY: the writable buffer ends in six X bytes and a NUL, and remains
        // alive while mkdtemp substitutes its same-length exclusive name.
        if unsafe { mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
            return Err(process_error(io::Error::last_os_error()));
        }
        template.pop();
        let path = PathBuf::from(std::ffi::OsString::from_vec(template));
        Ok(Self {
            path,
            clean_on_drop: true,
        })
    }

    /// Propagate cleanup failures after either successful or failed work.
    fn finish(mut self) -> Result<(), Failure> {
        self.clean_on_drop = false;
        self.cleanup().map_err(process_error)
    }

    /// Recover owner access only after cleanup of this flat workspace is denied.
    fn cleanup(&self) -> io::Result<()> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::other("Cannot call rmtree on a symlink"));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
        match fs::remove_dir_all(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                if !fs::symlink_metadata(&self.path)?.is_dir() {
                    return Err(error);
                }
                fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700))?;
                fs::remove_dir_all(&self.path)
            }
            result => result,
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if self.clean_on_drop {
            let _ = self.cleanup();
        }
    }
}

/// Match a with-statement: cleanup runs on both result paths and may fail itself.
fn with_workspace<T>(
    parent: &Path,
    prefix: &str,
    work: impl FnOnce(&Path) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let workspace = Workspace::create(parent, prefix)?;
    let result = work(&workspace.path);
    workspace.finish()?;
    result
}

/// Finish the write and explicitly check close before publishing its directory entry.
fn write_text(path: &Path, text: &str) -> Result<(), Failure> {
    let mut file = fs::File::create(path).map_err(process_error)?;
    let written = file.write_all(text.as_bytes());
    let descriptor = file.into_raw_fd();
    // SAFETY: into_raw_fd transfers this live descriptor; close consumes it once.
    if unsafe { close(descriptor) } != 0 {
        return Err(process_error(io::Error::last_os_error()));
    }
    written.map_err(process_error)
}

/// Replace the report only after a complete sibling write and successful close.
fn publish_report(target: &str, rendered: &str) -> Result<(), Failure> {
    let target = PathBuf::from(normalize_path(target));
    let parent = target.parent().unwrap_or(&target);
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    with_workspace(parent, ".relar-report-", |workspace| {
        let staged = workspace.join("report.md");
        write_text(&staged, rendered)?;
        fs::rename(staged, &target).map_err(process_error)
    })
}

/// Stage both decoded streams before replacing their separate transcript entries.
fn publish_capture(
    workspace: &Path,
    directory: &Path,
    role: &str,
    stdout: &str,
    stderr: &str,
) -> Result<(), Failure> {
    let staged_stdout = workspace.join("stdout.txt");
    let staged_stderr = workspace.join("stderr.txt");
    write_text(&staged_stdout, stdout)?;
    write_text(&staged_stderr, stderr)?;
    fs::rename(staged_stdout, directory.join(format!("{role}.stdout.txt")))
        .map_err(process_error)?;
    fs::rename(staged_stderr, directory.join(format!("{role}.stderr.txt"))).map_err(process_error)
}

/// Map process launch failures to the same catchable operating-system categories.
fn process_error(error: io::Error) -> Failure {
    let category = match error.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::NotADirectory => "NotADirectoryError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        io::ErrorKind::AlreadyExists => "FileExistsError",
        io::ErrorKind::InvalidInput => "ValueError",
        _ => "OSError",
    };
    Failure(category, error.to_string())
}

/// Text capture uses strict UTF-8 and Python's universal-newline conversion.
fn captured_text(bytes: Vec<u8>) -> Result<String, Failure> {
    let text = String::from_utf8(bytes)
        .map_err(|error| Failure("UnicodeDecodeError", error.to_string()))?;
    Ok(if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    })
}

/// Launch the same argv, cwd, and complete environment as the typed application.
/// Command::output drains both pipes, and explicit stdin inheritance preserves run().
fn collect_report(
    project: &PreparedProject,
    executable: &str,
    environment: &HashMap<String, String>,
    capture: Option<(&Path, &Path)>,
    role: &str,
) -> Result<Input<'static>, Failure> {
    let arguments: Vec<_> = project
        .argv
        .iter()
        .map(|argument| {
            if argument == "{ty}" {
                executable
            } else {
                argument
            }
        })
        .collect();
    let output = Command::new(arguments[0])
        .args(&arguments[1..])
        .current_dir(&project.cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(process_error)?;
    let stdout = captured_text(output.stdout)?;
    let stderr = captured_text(output.stderr)?;
    if let Some((directory, workspace)) = capture {
        publish_capture(workspace, directory, role, &stdout, &stderr)?;
    }
    if !output.status.success() {
        return Err(Failure(
            "CalledProcessError",
            format!("command {arguments:?} failed: {}", output.status),
        ));
    }
    Input::parse(stdout)
}

/// Validate the complete manifest before launching children, then run each old/new
/// pair in manifest order and sort names before the renderer's stable ranking.
fn prepared_inputs(
    path: &str,
    old_ty: &str,
    new_ty: &str,
    capture_directory: Option<&str>,
) -> Result<Vec<(String, Input<'static>, Input<'static>)>, Failure> {
    if !old_ty.starts_with('/') || !new_ty.starts_with('/') {
        return Err(Failure(
            "ValueError",
            "--old-ty and --new-ty must be absolute executable paths".into(),
        ));
    }
    let text = captured_text(read(path)?.into_bytes())?;
    depth_check(&text)?;
    let manifest: PreparedProjects =
        serde_json::from_str(&text).map_err(|error| Failure("ValueError", error.to_string()))?;
    if manifest.projects.is_empty() {
        return Err(Failure(
            "ValueError",
            "prepared-project manifest must contain at least one project".into(),
        ));
    }
    let mut names = HashSet::new();
    for project in &manifest.projects {
        if project.name.is_empty() || !names.insert(&project.name) {
            return Err(Failure(
                "ValueError",
                "prepared project names must be nonempty and unique".into(),
            ));
        }
        if project.cwd.is_empty() {
            return Err(Failure(
                "ValueError",
                "prepared project cwd must be nonempty".into(),
            ));
        }
        if project.argv.is_empty() || !project.argv.iter().any(|argument| argument == "{ty}") {
            return Err(Failure(
                "ValueError",
                "prepared project argv must contain a {ty} placeholder".into(),
            ));
        }
    }
    let mut environment = std::env::vars_os()
        .map(|(key, value)| {
            Ok((
                key.into_string().map_err(|_| {
                    Failure("UnicodeDecodeError", "environment key is not UTF-8".into())
                })?,
                value.into_string().map_err(|_| {
                    Failure(
                        "UnicodeDecodeError",
                        "environment value is not UTF-8".into(),
                    )
                })?,
            ))
        })
        .collect::<Result<HashMap<_, _>, Failure>>()?;
    environment.insert("TY_MEMORY_REPORT".into(), "json".into());
    environment.insert("TY_MAX_PARALLELISM".into(), "1".into());
    let collect = |capture: Option<(&Path, &Path)>| {
        let mut inputs = Vec::with_capacity(manifest.projects.len());
        for (index, project) in manifest.projects.into_iter().enumerate() {
            let directory = capture.map(|(root, _)| root.join(format!("project-{index}")));
            if let Some(directory) = &directory {
                fs::create_dir(directory).map_err(process_error)?;
            }
            let capture = directory
                .as_deref()
                .zip(capture.map(|(_, workspace)| workspace));
            let old = collect_report(&project, old_ty, &environment, capture, "old")?;
            let new = collect_report(&project, new_ty, &environment, capture, "new")?;
            inputs.push((project.name, old, new));
        }
        inputs.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(inputs)
    };
    if let Some(path) = capture_directory {
        let directory = PathBuf::from(normalize_path(path));
        fs::create_dir(&directory).map_err(process_error)?;
        with_workspace(&directory, ".relar-capture-", |workspace| {
            let staged = workspace.join("projects.json");
            write_text(&staged, &text)?;
            fs::rename(staged, directory.join("projects.json")).map_err(process_error)?;
            collect(Some((&directory, workspace)))
        })
    } else {
        collect(None)
    }
}

fn run() -> Result<(), Failure> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|argument| argument == "compute")
    {
        if arguments.len() != 4 {
            return Err(Failure(
                "ValueError",
                "compute requires ROUNDS OLD_DIR NEW_DIR".into(),
            ));
        }
        let rounds = arguments[1]
            .parse::<usize>()
            .map_err(|_| Failure("ValueError", "rounds must be nonnegative".into()))?;
        return compute(rounds, &arguments[2], &arguments[3]);
    }
    let run_mode = arguments.first().is_some_and(|argument| argument == "run");
    let mut index = usize::from(
        arguments
            .first()
            .is_some_and(|argument| argument == "compare" || run_mode),
    );
    let (mut old_path, mut new_path, mut old_directory, mut new_directory, mut output_path) =
        ("", "", "", "", None);
    let (mut projects_path, mut old_ty, mut new_ty) = ("", "", "");
    let mut capture_directory = None;
    while index < arguments.len() {
        let flag = &arguments[index];
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure("ValueError", format!("expected a value after {flag}")))?;
        match flag.as_str() {
            "--projects" if run_mode => projects_path = value,
            "--old-ty" if run_mode => old_ty = value,
            "--new-ty" if run_mode => new_ty = value,
            "--capture-dir" if run_mode => capture_directory = Some(value.as_str()),
            "--old" if !run_mode => old_path = value,
            "--new" if !run_mode => new_path = value,
            "--old-dir" if !run_mode => old_directory = value,
            "--new-dir" if !run_mode => new_directory = value,
            "--output" => output_path = Some(value),
            _ => return Err(Failure("ValueError", format!("unknown argument: {flag}"))),
        }
        index += 2;
    }
    let inputs = if run_mode {
        if projects_path.is_empty() || old_ty.is_empty() || new_ty.is_empty() {
            return Err(Failure(
                "ValueError",
                "expected run --projects MANIFEST --old-ty ABS --new-ty ABS [--capture-dir DIR] [--output REPORT.md]"
                    .into(),
            ));
        }
        if capture_directory.is_some()
            && output_path.is_some_and(|path| {
                Path::new(&normalize_path(path))
                    .file_name()
                    .is_some_and(|name| {
                        [
                            "projects.json",
                            "old.stdout.txt",
                            "old.stderr.txt",
                            "new.stdout.txt",
                            "new.stderr.txt",
                        ]
                        .iter()
                        .any(|reserved| name == *reserved)
                    })
            })
        {
            return Err(Failure(
                "ValueError",
                "output basename is reserved for captured transcripts".into(),
            ));
        }
        prepared_inputs(projects_path, old_ty, new_ty, capture_directory)?
    } else if !old_path.is_empty() && !new_path.is_empty() {
        vec![(
            project_name(&normalize_path(new_path)).to_owned(),
            Input::parse(read(old_path)?)?,
            Input::parse(read(new_path)?)?,
        )]
    } else if !old_directory.is_empty() && !new_directory.is_empty() {
        common_inputs(
            load_directory(old_directory)?,
            load_directory(new_directory)?,
        )
    } else {
        return Err(Failure(
            "ValueError",
            "expected --old/--new or --old-dir/--new-dir [--output REPORT.md]".into(),
        ));
    };
    if inputs.is_empty() {
        return Err(Failure(
            "ValueError",
            "No common projects found between old and new directories".into(),
        ));
    }
    let rendered = render_inputs(inputs)?;
    if let Some(path) = output_path {
        publish_report(path, &rendered)?;
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
