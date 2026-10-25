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

/// An absent result is a syntax/decode error, as caught by linecache.updatecache.
/// A registered codec with no native decoder stays distinguishable from bad input.
pub(super) fn decode(bytes: &[u8]) -> Result<Option<Cow<'_, str>>, Error> {
    let bom = bytes.starts_with(BOM);
    let bytes = bytes.strip_prefix(BOM).unwrap_or(bytes);
    let mut lines = bytes.split_inclusive(|b| *b == b'\n');
    let first = lines.next().unwrap_or_default();
    let mut checked = first.len();
    let declared = cookie(first).or_else(|| {
        if matches!(trim_space(first).first(), None | Some(b'#' | b'\r' | b'\n')) {
            let second = lines.next().unwrap_or_default();
            checked += second.len();
            cookie(second)
        } else {
            None
        }
    });
    let name = declared.map(normal_name).unwrap_or("utf-8");
    if bom && name != "utf-8" {
        return Ok(None);
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
                None => return Ok(None),
            }
        }
    };
    if bytes[..checked].contains(&0) {
        return Ok(None);
    }
    Ok(match mode {
        "utf-8" | "utf-8-sig" => std::str::from_utf8(bytes).ok().map(Cow::Borrowed),
        "ascii" => bytes
            .is_ascii()
            .then(|| Cow::Borrowed(std::str::from_utf8(bytes).unwrap())),
        "iso8859-1" | "iso-8859-1" => {
            Some(Cow::Owned(bytes.iter().map(|b| char::from(*b)).collect()))
        }
        "nontext" => {
            return Err(Error::new(
                Kind::LookupError,
                format!(
                    "'{name}' is not a text encoding; use codecs.decode() to handle arbitrary codecs"
                ),
            ));
        }
        "unsupported" => {
            return Err(Error::new(
                Kind::NotImplementedError,
                format!("linecache: native source decoder for '{name}' is unavailable"),
            ));
        }
        index => {
            let table = Charmap::at(index.parse().unwrap());
            let mut text = String::with_capacity(bytes.len());
            for b in bytes {
                let Some(ch) = table.decode(*b) else {
                    return Ok(None);
                };
                text.push(ch);
            }
            Some(Cow::Owned(text))
        }
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
