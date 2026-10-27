//! Python source decoding: detect a PEP 263 cookie in the first two binary
//! lines, then translate only TextIOWrapper's three physical line endings.
use std::borrow::Cow;

use super::super::{
    Error, Kind,
    charmaps::{Charmap, NAMES},
};

const BOM: &[u8] = b"\xef\xbb\xbf";

fn cookie(line: &[u8]) -> Option<&str> {
    let line = trim_space(line).strip_prefix(b"#")?;
    for (i, _) in line.windows(6).enumerate().filter(|(_, w)| *w == b"coding") {
        let rest = &line[i + 6..];
        if !matches!(rest.first(), Some(b':' | b'=')) {
            continue;
        }
        let rest = rest[1..]
            .iter()
            .position(|b| !matches!(b, b' ' | b'\t'))
            .map(|i| &rest[i + 1..])
            .unwrap_or_default();
        let end = rest
            .iter()
            .position(|b| !b.is_ascii_alphanumeric() && !matches!(b, b'_' | b'-' | b'.'))
            .unwrap_or(rest.len());
        if end != 0 {
            return std::str::from_utf8(&rest[..end]).ok();
        }
    }
    None
}

fn trim_space(line: &[u8]) -> &[u8] {
    let i = line
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t' | b'\x0c'))
        .unwrap_or(line.len());
    &line[i..]
}

fn normal_name(name: &str) -> &str {
    let first = &name[..name.len().min(12)];
    let simple = first.to_ascii_lowercase().replace('_', "-");
    for (prefix, canonical) in [
        ("utf-8", "utf-8"),
        ("latin-1", "iso-8859-1"),
        ("iso-8859-1", "iso-8859-1"),
        ("iso-latin-1", "iso-8859-1"),
    ] {
        if simple == prefix
            || simple
                .strip_prefix(prefix)
                .is_some_and(|s| s.starts_with('-'))
        {
            return canonical;
        }
    }
    name
}

fn registry_name(name: &str) -> String {
    let mut out = String::new();
    let mut separator = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' {
            if separator && !out.is_empty() {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    out
}

// CPython registers the OS code pages in addition to the portable registry.
#[cfg(windows)]
fn platform_codec(name: &str) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "IsValidCodePage"]
        fn is_valid_code_page(page: u32) -> i32;
    }
    matches!(name, "mbcs" | "oem" | "ansi" | "dbcs")
        || name
            .strip_prefix("cp")
            .and_then(|s| s.parse().ok())
            .is_some_and(|page| {
                // SAFETY: IsValidCodePage accepts any integer and borrows no memory.
                unsafe { is_valid_code_page(page) != 0 }
            })
}

#[cfg(not(windows))]
fn platform_codec(_name: &str) -> bool {
    false
}

/// The file reader drops malformed source; compile reports the reason to Python.
pub(crate) enum SourceDecodeError {
    Null,
    Invalid(String, Option<SourceRange>),
    NonText(String),
    Unsupported(String),
}

pub(crate) struct SourceRange {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
    // The tokenizer constructs its location before compile attaches the filename.
    pub tokenizer: bool,
}

fn decode_source(bytes: &[u8]) -> Result<Cow<'_, str>, SourceDecodeError> {
    let bom = bytes.starts_with(BOM);
    let bytes = bytes.strip_prefix(BOM).unwrap_or(bytes);
    let mut lines = bytes.split_inclusive(|b| *b == b'\n');
    let first = lines.next().unwrap_or_default();
    let mut checked = first.len();
    let mut declaration_line = first;
    let mut lineno = 1;
    let declared = cookie(first).or_else(|| {
        if matches!(trim_space(first).first(), None | Some(b'#' | b'\r' | b'\n')) {
            let second = lines.next().unwrap_or_default();
            checked += second.len();
            declaration_line = second;
            lineno = 2;
            cookie(second)
        } else {
            None
        }
    });
    let name = declared.map(normal_name).unwrap_or("utf-8");
    if bom && name != "utf-8" {
        return Err(SourceDecodeError::Invalid(
            format!("encoding problem: {name} with BOM"),
            Some(SourceRange {
                line: lineno,
                start: 0,
                // CPython includes the line terminator in its second cookie span.
                end: declaration_line.len()
                    - usize::from(lineno == 1 && declaration_line.ends_with(b"\n")),
                text: String::from_utf8_lossy(
                    declaration_line
                        .strip_suffix(b"\n")
                        .unwrap_or(declaration_line),
                )
                .into_owned(),
                tokenizer: true,
            }),
        ));
    }
    let normalized;
    let mode = match name {
        "utf-8" | "iso-8859-1" => name,
        _ => {
            normalized = registry_name(name);
            match NAMES
                .lines()
                .filter_map(|l| l.split_once(' '))
                .find(|(alias, _)| *alias == normalized)
            {
                Some((_, kind)) => kind,
                None if platform_codec(&normalized) => "unsupported",
                None => {
                    return Err(SourceDecodeError::Invalid(
                        format!("unknown encoding: {name}"),
                        None,
                    ));
                }
            }
        }
    };
    if bytes[..checked].contains(&0) {
        return Err(SourceDecodeError::Null);
    }
    match mode {
        "utf-8" | "utf-8-sig" => std::str::from_utf8(bytes).map(Cow::Borrowed).map_err(|err| {
            if name != "utf-8" {
                let (_, _, message) = super::super::bytes::utf8_error_details(bytes, err);
                return SourceDecodeError::Invalid(message, None);
            }
            let pos = err.valid_up_to();
            let line = 1 + bytes[..pos].iter().filter(|&&b| b == b'\n').count();
            let start = bytes[..pos].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
            let offset = 1 + std::str::from_utf8(&bytes[start..pos]).unwrap().chars().count();
            let raw_line = bytes[start..].split(|&b| b == b'\n').next().unwrap_or_default();
            SourceDecodeError::Invalid(format!("Non-UTF-8 code starting with '\\x{:02x}' on line {line}, but no encoding declared; see https://peps.python.org/pep-0263/ for details", bytes[pos]), Some(SourceRange { line, start: offset, end: offset, text: String::from_utf8_lossy(raw_line).into_owned(), tokenizer: true }))
        }),
        "ascii" => {
            if let Some(pos) = bytes.iter().position(|b| !b.is_ascii()) {
                return Err(SourceDecodeError::Invalid(format!("'ascii' codec can't decode byte 0x{:02x} in position {pos}: ordinal not in range(128)", bytes[pos]), None));
            }
            Ok(Cow::Borrowed(std::str::from_utf8(bytes).unwrap()))
        }
        "iso8859-1" | "iso-8859-1" => Ok(Cow::Owned(bytes.iter().map(|b| char::from(*b)).collect())),
        "nontext" => Err(SourceDecodeError::NonText(format!("'{name}' is not a text encoding; use codecs.decode() to handle arbitrary codecs"))),
        "unsupported" => Err(SourceDecodeError::Unsupported(name.to_owned())),
        index => {
            let table = Charmap::at(index.parse().unwrap());
            let mut text = String::with_capacity(bytes.len());
            for (pos, &b) in bytes.iter().enumerate() {
                let ch = table.decode(b).ok_or_else(|| SourceDecodeError::Invalid(format!("'charmap' codec can't decode byte 0x{b:02x} in position {pos}: character maps to <undefined>"), None))?;
                text.push(ch);
            }
            Ok(Cow::Owned(text))
        }
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<Option<Cow<'_, str>>, Error> {
    match decode_source(bytes) {
        Ok(text) => Ok(Some(text)),
        Err(SourceDecodeError::Null | SourceDecodeError::Invalid(..)) => Ok(None),
        Err(SourceDecodeError::NonText(msg)) => Err(Error::new(Kind::LookupError, msg)),
        Err(SourceDecodeError::Unsupported(name)) => Err(Error::new(
            Kind::NotImplementedError,
            format!("linecache: native source decoder for '{name}' is unavailable"),
        )),
    }
}

/// Compile scans physical lines after universal newline translation, before decoding.
pub(crate) fn decode_compile_source(bytes: &[u8]) -> Result<String, SourceDecodeError> {
    if bytes.contains(&0) {
        return Err(SourceDecodeError::Null);
    }
    let mut normalized;
    let input = if bytes.contains(&b'\r') || !bytes.ends_with(b"\n") {
        normalized = bytes.to_vec();
        super::super::codecs::normalize_newlines(&mut normalized);
        if !normalized.ends_with(b"\n") {
            normalized.push(b'\n');
        }
        &normalized
    } else {
        bytes
    };
    let text = decode_source(input)?;
    if text.starts_with('\u{feff}') {
        // Ruff accepts an initial BOM; the Python tokenizer already consumed the signature.
        return Err(SourceDecodeError::Invalid(
            "invalid non-printable character U+FEFF".into(),
            Some(SourceRange {
                line: 1,
                start: 1,
                end: 1,
                text: text.lines().next().unwrap_or_default().to_owned(),
                tokenizer: false,
            }),
        ));
    }
    Ok(if text.contains('\r') {
        super::super::codecs::universal_newlines(text.into_owned())
    } else {
        text.into_owned()
    })
}

/// Keep any other Unicode separators in the line, including NEL and U+2028.
pub(super) fn lines(source: &str) -> impl Iterator<Item = &str> {
    let mut tail = source;
    std::iter::from_fn(move || {
        if tail.is_empty() {
            return None;
        }
        if let Some(i) = tail.find(['\r', '\n']) {
            let head = &tail[..i];
            let skip = if tail[i..].starts_with("\r\n") { 2 } else { 1 };
            tail = &tail[i + skip..];
            Some(head)
        } else {
            Some(std::mem::take(&mut tail))
        }
    })
}
