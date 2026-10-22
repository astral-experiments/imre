//! Independent offline implementation of uv's CPython catalog transformation.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::rc::Rc;
use std::str::FromStr;

use flate2::bufread::GzDecoder;
use memchr::{memchr, memchr2, memchr3};
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::ser::{Formatter, PrettyFormatter};

#[derive(Debug)]
struct Failure(&'static str, String);

fn invalid(message: impl Into<String>) -> Failure {
    Failure("ValueError", message.into())
}

fn file_error(error: io::Error) -> Failure {
    if error.kind() == io::ErrorKind::InvalidInput {
        return Failure("ValueError", error.to_string());
    }
    let kind = match error.raw_os_error() {
        Some(2) => "FileNotFoundError",
        Some(13) => "PermissionError",
        Some(17) => "FileExistsError",
        Some(20) => "NotADirectoryError",
        Some(21) => "IsADirectoryError",
        _ => "OSError",
    };
    Failure(kind, error.to_string())
}

/// Concatenate independently decoded gzip members and accept Python's post-member zero padding.
struct GzipMembers<R> {
    source: Option<R>,
    member: Option<GzDecoder<R>>,
}

impl<R: BufRead> Read for GzipMembers<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let mut source = if let Some(member) = &mut self.member {
                let count = member.read(buffer)?;
                if count != 0 {
                    return Ok(count);
                }
                let mut source = self.member.take().unwrap().into_inner();
                loop {
                    let chunk = source.fill_buf()?;
                    let zeros = chunk.iter().take_while(|&&byte| byte == 0).count();
                    if zeros == 0 {
                        break;
                    }
                    source.consume(zeros);
                }
                source
            } else if let Some(source) = self.source.take() {
                source
            } else {
                return Ok(0);
            };
            if source.fill_buf()?.is_empty() {
                return Ok(0);
            }
            self.member = Some(GzDecoder::new(source));
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Integer {
    Small(i64),
    Big(BigInt),
}

impl Integer {
    /// Parse Python's decimal spelling, including Unicode digits and underscores.
    /// The common i64 representation preserves hash equality with wider inputs.
    fn parse(text: &str) -> Result<Self, Failure> {
        let text = text.trim_matches(char::is_whitespace);
        if let Ok(value) = text.parse::<i64>() {
            return Ok(Self::Small(value));
        }
        let mut normalized = String::with_capacity(text.len());
        let mut characters = text.chars().peekable();
        if matches!(characters.peek(), Some('+' | '-')) {
            normalized.push(characters.next().unwrap_or('+'));
        }
        let mut digit_before = false;
        for character in characters {
            if character == '_' && digit_before {
                digit_before = false;
                continue;
            }
            let digit =
                decimal_digit(character).ok_or_else(|| invalid("invalid decimal integer"))?;
            normalized.push(char::from(b'0' + digit));
            digit_before = true;
        }
        if !digit_before {
            return Err(invalid("invalid decimal integer"));
        }
        let value =
            BigInt::from_str(&normalized).map_err(|_| invalid("invalid decimal integer"))?;
        Ok(value.to_i64().map_or_else(|| Self::Big(value), Self::Small))
    }

    fn big(&self) -> BigInt {
        match self {
            Self::Small(value) => BigInt::from(*value),
            Self::Big(value) => value.clone(),
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

impl fmt::Display for Integer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Small(value) => value.fmt(formatter),
            Self::Big(value) => value.fmt(formatter),
        }
    }
}

impl Serialize for Integer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Small(value) => serializer.serialize_i64(*value),
            Self::Big(value) => {
                let number = serde_json::Number::from_str(&value.to_string())
                    .map_err(serde::ser::Error::custom)?;
                number.serialize(serializer)
            }
        }
    }
}

/// Unicode 16 decimal blocks, regenerated from CPython 3.14's unicodedata.
fn decimal_digit(character: char) -> Option<u8> {
    const ZEROES: &[u32] = &[
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
        0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
        0xff10, 0x104a0, 0x10d30, 0x10d40, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450,
        0x114d0, 0x11650, 0x116c0, 0x116d0, 0x116da, 0x11730, 0x118e0, 0x11950, 0x11bf0, 0x11c50,
        0x11d50, 0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70, 0x1ccf0, 0x1d7ce,
        0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
    ];
    let codepoint = u32::from(character);
    let end = ZEROES.partition_point(|zero| *zero <= codepoint);
    let digit = codepoint.checked_sub(*ZEROES.get(end.checked_sub(1)?)?)?;
    (digit < 10).then_some(digit as u8)
}

#[derive(Deserialize)]
struct Artifact {
    url: String,
    platform: String,
    variant: String,
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    version: String,
    artifacts: Vec<Artifact>,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct Version {
    major: Integer,
    minor: Integer,
    patch: Integer,
    prerelease: String,
}

impl Version {
    fn parse(text: &str) -> Result<Self, Failure> {
        let pieces = text.splitn(4, '.').collect::<Vec<_>>();
        if pieces.len() != 3 {
            return Err(invalid("expected three version components"));
        }
        let mut patch = pieces[2];
        let mut prerelease = String::new();
        for kind in ["a", "b", "rc"] {
            if let Some((base, number)) = patch.split_once(kind) {
                patch = base;
                prerelease = format!("{kind}{number}");
                break;
            }
        }
        Ok(Self {
            major: Integer::parse(pieces[0])?,
            minor: Integer::parse(pieces[1])?,
            patch: Integer::parse(patch)?,
            prerelease,
        })
    }
}

#[derive(Serialize)]
struct Arch {
    family: String,
    variant: Option<String>,
}

impl Arch {
    fn parse(text: &str) -> Result<Self, Failure> {
        let mapped = match text {
            "ppc64" => "powerpc64",
            "ppc64le" => "powerpc64le",
            _ => text,
        };
        if mapped
            .find('_')
            .is_none_or(|first| !mapped[first + 1..].contains('_'))
        {
            return Ok(Self {
                family: mapped.to_owned(),
                variant: None,
            });
        }
        let pieces = mapped.split('_').collect::<Vec<_>>();
        let family = pieces[..pieces.len().min(2)].join("_");
        let variant = pieces.get(2).map(|value| (*value).to_owned());
        if variant.as_deref() == Some("") {
            return Err(invalid("architecture variants must not be empty"));
        }
        Ok(Self { family, variant })
    }

    fn key(&self) -> String {
        match &self.variant {
            Some(variant) => format!("{}_{variant}", self.family),
            None => self.family.clone(),
        }
    }
}

struct Download {
    version: Rc<Version>,
    target: Rc<Target>,
    url: String,
    sha256: Option<String>,
    build: Rc<str>,
    #[allow(dead_code)]
    filename: String,
}

/// Percent decoding leaves malformed escapes and '+' unchanged, as unquote does.
fn unquote(text: &str) -> String {
    let bytes = text.as_bytes();
    let Some(mut escape) = memchr(b'%', bytes) else {
        return text.to_owned();
    };
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut position = 0;
    loop {
        decoded.extend_from_slice(&bytes[position..escape]);
        if let Some(hex) = bytes.get(escape + 1..escape + 3)
            && let (Some(high), Some(low)) =
                ((hex[0] as char).to_digit(16), (hex[1] as char).to_digit(16))
        {
            decoded.push((high * 16 + low) as u8);
            position = escape + 3;
        } else {
            decoded.push(b'%');
            position = escape + 1;
        }
        let Some(next) = memchr(b'%', &bytes[position..]) else {
            break;
        };
        escape = position + next;
    }
    decoded.extend_from_slice(&bytes[position..]);
    match String::from_utf8(decoded) {
        Ok(value) => value,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    }
}

#[derive(Eq, Hash, PartialEq)]
struct SelectionKey {
    os: String,
    arch: String,
    libc: String,
    variant: Option<&'static str>,
}

struct Target {
    arch: Arch,
    key: Rc<SelectionKey>,
    priority: (usize, i32),
}

fn parse_target(mut platform: &str, raw_variant: &str) -> Result<Option<Target>, Failure> {
    let mut options = Vec::new();
    for suffix in ["-debug", "-freethreaded"] {
        if let Some(base) = platform.strip_suffix(suffix) {
            platform = base;
            options.push(&suffix[1..]);
        }
    }
    let mut flavor = raw_variant;
    if raw_variant.contains('+') {
        let parts = raw_variant.split('+').collect::<Vec<_>>();
        let last = parts.last().copied().unwrap_or("");
        if ["full", "install_only", "install_only_stripped"].contains(&last) {
            options.extend_from_slice(&parts[..parts.len() - 1]);
            flavor = last;
        }
    }
    if options.contains(&"static") {
        return Ok(None);
    }
    let pieces = platform.split('-').collect::<Vec<_>>();
    if pieces.len() < 3 || (pieces[2] == "linux" && pieces.len() < 4) {
        return Ok(None);
    }
    let arch = Arch::parse(pieces[0])?;
    let os = pieces[2].to_owned();
    let libc = if pieces[2] == "linux" {
        pieces[3]
    } else {
        "none"
    };
    let variant = match (
        options.contains(&"freethreaded"),
        options.contains(&"debug"),
    ) {
        (true, true) => Some("freethreaded+debug"),
        (true, false) => Some("freethreaded"),
        (false, true) => Some("debug"),
        (false, false) => None,
    };
    let flavor_priority = [
        "install_only_stripped",
        "install_only",
        "shared-pgo",
        "shared-noopt",
        "static-noopt",
    ]
    .iter()
    .position(|candidate| *candidate == flavor)
    .unwrap_or(6);
    let build_priority = -(i32::from(options.contains(&"lto"))
        + i32::from(options.contains(&"pgo"))
        + i32::from(!options.contains(&"static")));
    let key = Rc::new(SelectionKey {
        os,
        arch: arch.key(),
        libc: libc.to_owned(),
        variant,
    });
    Ok(Some(Target {
        arch,
        key,
        priority: (flavor_priority, build_priority),
    }))
}

#[derive(Default)]
struct TargetCache {
    by_platform: HashMap<String, HashMap<String, Rc<Target>>>,
    entries: usize,
}

impl TargetCache {
    /// Borrow both raw labels on hits and cap retained normalized pairs across releases.
    fn get(&mut self, platform: &str, variant: &str) -> Result<Option<Rc<Target>>, Failure> {
        if let Some(target) = self
            .by_platform
            .get(platform)
            .and_then(|variants| variants.get(variant))
        {
            return Ok(Some(Rc::clone(target)));
        }
        let Some(target) = parse_target(platform, variant)? else {
            return Ok(None);
        };
        let target = Rc::new(target);
        if self.entries < 512 {
            self.by_platform
                .entry(platform.to_owned())
                .or_default()
                .insert(variant.to_owned(), Rc::clone(&target));
            self.entries += 1;
        }
        Ok(Some(target))
    }
}

#[derive(Default)]
struct Group {
    positions: HashMap<Rc<SelectionKey>, usize>,
    selected: Vec<Download>,
}

impl Group {
    fn offer(
        &mut self,
        version: &Rc<Version>,
        build: &Rc<str>,
        target: Rc<Target>,
        artifact: Artifact,
        filename: String,
    ) {
        let existing = self.positions.get(&target.key).copied();
        if let Some(i) = existing
            && target.priority >= self.selected[i].target.priority
        {
            return;
        }
        let download = Download {
            version: Rc::clone(version),
            build: Rc::clone(build),
            target,
            url: artifact.url,
            sha256: artifact.sha256,
            filename,
        };
        if let Some(i) = existing {
            self.selected[i] = download;
        } else {
            self.positions
                .insert(Rc::clone(&download.target.key), self.selected.len());
            self.selected.push(download);
        }
    }
}

fn python_space(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

fn line_break(character: char) -> bool {
    matches!(
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
    )
}

#[derive(Default)]
struct Selection {
    positions: HashMap<Rc<Version>, usize>,
    groups: Vec<Group>,
    targets: TargetCache,
}

impl Selection {
    fn offer_line(&mut self, line: &str) -> Result<(), Failure> {
        let line = line.trim_matches(python_space);
        if line.is_empty() {
            return Ok(());
        }
        let mut record: Release =
            serde_json::from_str(line).map_err(|error| invalid(error.to_string()))?;
        let (version, date) = record
            .version
            .split_once('+')
            .unwrap_or((&record.version, ""));
        let version = Rc::new(Version::parse(version)?);
        let release = Integer::parse(date)?;
        let build: Rc<str> = release.to_string().into();
        let mut position = self.positions.get(&version).copied();
        record
            .artifacts
            .sort_by(|left, right| left.url.cmp(&right.url));
        for artifact in record.artifacts {
            let filename = unquote(artifact.url.rsplit('/').next().unwrap_or(""));
            let Some(target) = self.targets.get(&artifact.platform, &artifact.variant)? else {
                continue;
            };
            if release < Integer::Small(20250311) && target.key.libc == "musl" {
                continue;
            }
            let position = *position.get_or_insert_with(|| {
                self.groups.push(Group::default());
                let position = self.groups.len() - 1;
                self.positions.insert(Rc::clone(&version), position);
                position
            });
            self.groups[position].offer(&version, &build, target, artifact, filename);
        }
        Ok(())
    }

    fn offer_physical_line(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        let line = std::str::from_utf8(bytes)
            .map_err(|error| Failure("UnicodeDecodeError", error.to_string()))?;
        // Keep Python's wider str.splitlines() boundaries within each file line.
        if memchr3(0x0b, 0x0c, 0x1c, bytes).is_some()
            || memchr3(0x1d, 0x1e, 0xc2, bytes).is_some()
            || memchr(0xe2, bytes).is_some()
        {
            for part in line.split(line_break) {
                self.offer_line(part)?;
            }
            Ok(())
        } else {
            self.offer_line(line)
        }
    }
}

/// Keep at most one physical row while recognizing LF, CRLF, and bare CR.
fn find_downloads(mut reader: impl BufRead, gzip: bool) -> Result<Vec<Download>, Failure> {
    let mut selection = Selection::default();
    let mut partial = Vec::new();
    let mut pending = None;
    let mut process_line = |bytes: &[u8]| {
        if pending.is_none() {
            pending = selection.offer_physical_line(bytes).err();
        } else if !matches!(pending, Some(Failure("UnicodeDecodeError", _)))
            && let Err(error) = std::str::from_utf8(bytes)
        {
            // The original bulk read validates all input before parsing any record.
            pending = Some(Failure("UnicodeDecodeError", error.to_string()));
        }
    };
    loop {
        let chunk = reader.fill_buf().map_err(|error| match error.kind() {
            io::ErrorKind::UnexpectedEof if gzip => Failure("EOFError", error.to_string()),
            io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData if gzip => {
                Failure("BadGzipFile", error.to_string())
            }
            _ => file_error(error),
        })?;
        if chunk.is_empty() {
            process_line(&partial);
            break;
        }
        let count = if let Some(end) = memchr2(b'\r', b'\n', chunk) {
            if partial.is_empty() {
                process_line(&chunk[..end]);
            } else {
                partial.extend_from_slice(&chunk[..end]);
                process_line(&partial);
                partial.clear();
            }
            end + 1
        } else {
            partial.extend_from_slice(chunk);
            chunk.len()
        };
        reader.consume(count);
    }
    if let Some(error) = pending {
        return Err(error);
    }
    Ok(selection
        .groups
        .into_iter()
        .flat_map(|group| group.selected)
        .collect())
}

fn prerelease_key(text: &str) -> Result<(u8, Integer), Failure> {
    for (prefix, kind) in [("a", 0), ("b", 1), ("rc", 2)] {
        if let Some(number) = text.strip_prefix(prefix) {
            return Ok((kind, Integer::parse(number)?));
        }
    }
    Ok((3, Integer::Small(0)))
}

fn variant_order(variant: Option<&str>) -> u8 {
    match variant {
        None => 0,
        Some("freethreaded") => 1,
        Some("freethreaded+debug") => 2,
        Some("debug") => 3,
        _ => unreachable!("variants are constructed by parse_target"),
    }
}

fn download_key(download: &Download) -> String {
    let version = &download.version;
    let suffix = download
        .target
        .key
        .variant
        .map(|value| format!("+{value}"))
        .unwrap_or_default();
    format!(
        "cpython-{}.{}.{}{}{suffix}-{}-{}-{}",
        version.major,
        version.minor,
        version.patch,
        version.prerelease,
        download.target.key.os,
        download.target.key.arch,
        download.target.key.libc,
    )
}

#[derive(Serialize)]
struct Entry<'a> {
    name: &'static str,
    arch: &'a Arch,
    os: &'a str,
    libc: &'a str,
    major: &'a Integer,
    minor: &'a Integer,
    patch: &'a Integer,
    prerelease: &'a str,
    url: &'a str,
    sha256: &'a Option<String>,
    variant: Option<&'a str>,
    build: &'a str,
}

struct Catalog(Vec<(String, Download)>);

impl Serialize for Catalog {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, download) in &self.0 {
            map.serialize_entry(
                key,
                &Entry {
                    name: "cpython",
                    arch: &download.target.arch,
                    os: &download.target.key.os,
                    libc: &download.target.key.libc,
                    major: &download.version.major,
                    minor: &download.version.minor,
                    patch: &download.version.patch,
                    prerelease: &download.version.prerelease,
                    url: &download.url,
                    sha256: &download.sha256,
                    variant: download.target.key.variant,
                    build: &download.build,
                },
            )?;
        }
        map.end()
    }
}

struct AsciiPretty(PrettyFormatter<'static>);

macro_rules! pretty_method {
    ($name:ident) => {
        fn $name<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
            self.0.$name(writer)
        }
    };
    ($name:ident, $argument:ident: $kind:ty) => {
        fn $name<W: ?Sized + Write>(&mut self, writer: &mut W, $argument: $kind) -> io::Result<()> {
            self.0.$name(writer, $argument)
        }
    };
}

impl Formatter for AsciiPretty {
    pretty_method!(begin_array);
    pretty_method!(end_array);
    pretty_method!(begin_array_value, first: bool);
    pretty_method!(end_array_value);
    pretty_method!(begin_object);
    pretty_method!(end_object);
    pretty_method!(begin_object_key, first: bool);
    pretty_method!(begin_object_value);
    pretty_method!(end_object_value);

    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        if fragment.is_ascii() && !fragment.as_bytes().contains(&0x7f) {
            return writer.write_all(fragment.as_bytes());
        }
        let mut start = 0;
        for (position, character) in fragment.char_indices() {
            if character >= '\u{7f}' {
                writer.write_all(&fragment.as_bytes()[start..position])?;
                let mut units = [0; 2];
                for unit in character.encode_utf16(&mut units) {
                    write!(writer, "\\u{unit:04x}")?;
                }
                start = position + character.len_utf8();
            }
        }
        writer.write_all(&fragment.as_bytes()[start..])
    }
}

/// Preserve stable winner order and emit the complete Python-compatible JSON.
fn generate(reader: impl BufRead, gzip: bool) -> Result<Vec<u8>, Failure> {
    let mut keyed = Vec::new();
    for download in find_downloads(reader, gzip)? {
        keyed.push((prerelease_key(&download.version.prerelease)?, download));
    }
    keyed.sort_by(|(left_pre, left), (right_pre, right)| {
        right
            .version
            .major
            .cmp(&left.version.major)
            .then_with(|| right.version.minor.cmp(&left.version.minor))
            .then_with(|| right.version.patch.cmp(&left.version.patch))
            .then_with(|| right_pre.cmp(left_pre))
            .then_with(|| {
                variant_order(left.target.key.variant).cmp(&variant_order(right.target.key.variant))
            })
            .then_with(|| left.target.key.os.cmp(&right.target.key.os))
            .then_with(|| left.target.arch.family.cmp(&right.target.arch.family))
            .then_with(|| {
                left.target
                    .arch
                    .variant
                    .as_deref()
                    .unwrap_or("")
                    .cmp(right.target.arch.variant.as_deref().unwrap_or(""))
            })
            .then_with(|| left.target.key.libc.cmp(&right.target.key.libc))
    });
    let mut positions = HashMap::new();
    let mut catalog = Catalog(Vec::new());
    for (_, download) in keyed {
        if (&download.version.major, &download.version.minor)
            < (&Integer::Small(3), &Integer::Small(8))
        {
            continue;
        }
        let key = download_key(&download);
        if let Some(&position) = positions.get(&key) {
            catalog.0[position] = (key, download);
        } else {
            positions.insert(key.clone(), catalog.0.len());
            catalog.0.push((key, download));
        }
    }
    let mut output = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut output, AsciiPretty(PrettyFormatter::new()));
    catalog
        .serialize(&mut serializer)
        .map_err(|error| invalid(error.to_string()))?;
    output.push(b'\n');
    Ok(output)
}

/// Match lexical POSIX Path construction without resolving '..' or symlinks.
fn normalized_path(text: &str) -> String {
    let root = if text.starts_with("//") && !text.starts_with("///") {
        "//"
    } else if text.starts_with('/') {
        "/"
    } else {
        ""
    };
    let tail = text
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/");
    let result = format!("{root}{tail}");
    if result.is_empty() {
        ".".into()
    } else {
        result
    }
}

fn run() -> Result<(), Failure> {
    let arguments = std::env::args().collect::<Vec<_>>();
    let repeated = arguments.len() == 5 && arguments[1] == "--compute";
    if !repeated && arguments.len() != 3 {
        return Err(invalid("expected INPUT.ndjson OUTPUT.json"));
    }
    let rounds = if repeated {
        let value = Integer::parse(&arguments[2])?.big();
        if value < BigInt::from(0) {
            return Err(invalid("rounds must be nonnegative"));
        }
        value
    } else {
        BigInt::from(1)
    };
    let input_index = if repeated { 3 } else { 1 };
    let source = normalized_path(&arguments[input_index]);
    let mut checksum = BigInt::from(0);
    let rendered = if repeated {
        let raw = fs::read(&source).map_err(file_error)?;
        let text = String::from_utf8(raw)
            .map_err(|error| Failure("UnicodeDecodeError", error.to_string()))?;
        let mut remaining = rounds;
        let mut rendered = Vec::new();
        while remaining > BigInt::from(0) {
            rendered = generate(std::hint::black_box(text.as_bytes()), false)?;
            checksum += std::hint::black_box(rendered.len());
            remaining -= 1;
        }
        rendered
    } else {
        let file = fs::File::open(&source).map_err(file_error)?;
        if source.ends_with(".gz") {
            let input = GzipMembers {
                source: Some(BufReader::new(file)),
                member: None,
            };
            generate(BufReader::new(input), true)?
        } else {
            generate(BufReader::new(file), false)?
        }
    };
    let output_name = normalized_path(&arguments[input_index + 1]);
    let output = Path::new(&output_name);
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(file_error)?;
    }
    fs::write(output, rendered).map_err(file_error)?;
    if repeated {
        println!("{checksum}");
    }
    Ok(())
}

fn main() {
    if let Err(Failure(kind, message)) = run() {
        eprintln!("{kind}: {message}");
        std::process::exit(1);
    }
}
