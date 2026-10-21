//! Independent offline implementation of uv's CPython catalog transformation.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::rc::Rc;
use std::str::FromStr;

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
    arch: Arch,
    os: String,
    libc: String,
    url: String,
    sha256: Option<String>,
    variant: Option<&'static str>,
    build: String,
    priority: (usize, i32),
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

fn parse_artifact(
    version: &Rc<Version>,
    release: &Integer,
    artifact: Artifact,
) -> Result<Option<Download>, Failure> {
    let filename = unquote(artifact.url.rsplit('/').next().unwrap_or(""));
    let mut platform = artifact.platform.as_str();
    let mut options = Vec::new();
    for suffix in ["-debug", "-freethreaded"] {
        if let Some(base) = platform.strip_suffix(suffix) {
            platform = base;
            options.push(&suffix[1..]);
        }
    }
    let parts = artifact.variant.split('+').collect::<Vec<_>>();
    let last = parts.last().copied().unwrap_or("");
    let flavor = if ["full", "install_only", "install_only_stripped"].contains(&last) {
        options.extend_from_slice(&parts[..parts.len() - 1]);
        last
    } else {
        &artifact.variant
    };
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
    if *release < Integer::Small(20250311) && libc == "musl" {
        return Ok(None);
    }
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
    Ok(Some(Download {
        version: Rc::clone(version),
        arch,
        os,
        libc: libc.to_owned(),
        url: artifact.url,
        sha256: artifact.sha256,
        variant,
        build: release.to_string(),
        priority: (flavor_priority, build_priority),
        filename,
    }))
}

type SelectionKey = (String, String, String, Option<&'static str>);

#[derive(Default)]
struct Group {
    positions: HashMap<SelectionKey, usize>,
    selected: Vec<Download>,
}

impl Group {
    fn offer(&mut self, download: Download) {
        let key = (
            download.os.clone(),
            download.arch.key(),
            download.libc.clone(),
            download.variant,
        );
        if let Some(&position) = self.positions.get(&key) {
            if download.priority < self.selected[position].priority {
                self.selected[position] = download;
            }
        } else {
            self.positions.insert(key, self.selected.len());
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

fn find_downloads(text: &str) -> Result<Vec<Download>, Failure> {
    let mut positions: HashMap<Rc<Version>, usize> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    // Most release indexes use CR/LF only. The byte guards also catch the UTF-8
    // leading bytes of NEL/LS/PS, so Unicode separators retain the scalar path.
    let bytes = text.as_bytes();
    let ordinary_lines = memchr3(0x0b, 0x0c, 0x1c, bytes).is_none()
        && memchr3(0x1d, 0x1e, 0xc2, bytes).is_none()
        && memchr(0xe2, bytes).is_none();
    let mut remaining = text;
    while !remaining.is_empty() {
        let boundary = if ordinary_lines {
            memchr2(b'\r', b'\n', remaining.as_bytes()).map(|index| (index, 1))
        } else {
            remaining
                .char_indices()
                .find(|(_, character)| line_break(*character))
                .map(|(index, character)| (index, character.len_utf8()))
        };
        let line = if let Some((index, width)) = boundary {
            let line = &remaining[..index];
            remaining = &remaining[index + width..];
            line
        } else {
            let line = remaining;
            remaining = "";
            line
        };
        let line = line.trim_matches(python_space);
        if line.is_empty() {
            continue;
        }
        let mut record: Release =
            serde_json::from_str(line).map_err(|error| invalid(error.to_string()))?;
        let (version, date) = record
            .version
            .split_once('+')
            .unwrap_or((&record.version, ""));
        let version = Rc::new(Version::parse(version)?);
        let release = Integer::parse(date)?;
        let mut position = positions.get(&version).copied();
        record
            .artifacts
            .sort_by(|left, right| left.url.cmp(&right.url));
        for artifact in record.artifacts {
            let Some(download) = parse_artifact(&version, &release, artifact)? else {
                continue;
            };
            let position = *position.get_or_insert_with(|| {
                groups.push(Group::default());
                let position = groups.len() - 1;
                positions.insert(Rc::clone(&version), position);
                position
            });
            groups[position].offer(download);
        }
    }
    Ok(groups
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
        _ => unreachable!("variants are constructed by parse_artifact"),
    }
}

fn download_key(download: &Download) -> String {
    let version = &download.version;
    let suffix = download
        .variant
        .map(|value| format!("+{value}"))
        .unwrap_or_default();
    format!(
        "cpython-{}.{}.{}{}{suffix}-{}-{}-{}",
        version.major,
        version.minor,
        version.patch,
        version.prerelease,
        download.os,
        download.arch.key(),
        download.libc,
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
                    arch: &download.arch,
                    os: &download.os,
                    libc: &download.libc,
                    major: &download.version.major,
                    minor: &download.version.minor,
                    patch: &download.version.patch,
                    prerelease: &download.version.prerelease,
                    url: &download.url,
                    sha256: &download.sha256,
                    variant: download.variant,
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
fn generate(text: &str) -> Result<Vec<u8>, Failure> {
    let mut keyed = Vec::new();
    for download in find_downloads(text)? {
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
            .then_with(|| variant_order(left.variant).cmp(&variant_order(right.variant)))
            .then_with(|| left.os.cmp(&right.os))
            .then_with(|| left.arch.family.cmp(&right.arch.family))
            .then_with(|| {
                left.arch
                    .variant
                    .as_deref()
                    .unwrap_or("")
                    .cmp(right.arch.variant.as_deref().unwrap_or(""))
            })
            .then_with(|| left.libc.cmp(&right.libc))
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
    let raw = fs::read(normalized_path(&arguments[input_index])).map_err(file_error)?;
    let text =
        String::from_utf8(raw).map_err(|error| Failure("UnicodeDecodeError", error.to_string()))?;
    let mut rendered = Vec::new();
    let mut checksum = BigInt::from(0);
    let mut remaining = rounds;
    while remaining > BigInt::from(0) {
        rendered = generate(std::hint::black_box(&text))?;
        checksum += std::hint::black_box(rendered.len());
        remaining -= 1;
    }
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
