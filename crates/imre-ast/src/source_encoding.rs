//! Python source decoding: detect a PEP 263 cookie in the first two binary
//! lines, then translate only TextIOWrapper's three physical line endings.
use std::borrow::Cow;

mod charmaps;
pub use charmaps::Charmap;
use charmaps::NAMES;

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
    let mut prefix = [0; 12];
    for (out, byte) in prefix.iter_mut().zip(name.bytes()) {
        *out = if byte == b'_' {
            b'-'
        } else {
            byte.to_ascii_lowercase()
        };
    }
    let simple = &prefix[..name.len().min(prefix.len())];
    for (prefix, canonical) in [
        ("utf-8", "utf-8"),
        ("latin-1", "iso-8859-1"),
        ("iso-8859-1", "iso-8859-1"),
        ("iso-latin-1", "iso-8859-1"),
    ] {
        if simple == prefix.as_bytes()
            || simple
                .strip_prefix(prefix.as_bytes())
                .is_some_and(|s| s.starts_with(b"-"))
        {
            return canonical;
        }
    }
    name
}

fn registry_name(name: &str) -> Result<String, SourceDecodeError> {
    let mut out = String::new();
    out.try_reserve_exact(name.len())?;
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
    Ok(out)
}

fn codec_kind(key: &str) -> Option<&'static str> {
    NAMES
        .lines()
        .filter_map(|line| line.split_once(' '))
        .find_map(|(alias, kind)| (alias == key).then_some(kind))
}

/// Match a text stream's plain UTF-8 codec, without the tokenizer's cookie shortcuts.
pub fn is_utf8_codec(name: &str) -> bool {
    if name == "utf-8" {
        return true;
    }
    if name.contains('\0') {
        return false;
    }
    let Ok(mut key) = registry_name(name) else {
        return false;
    };
    if key.contains('.') {
        // encodings.search_function also tries dots as underscores for aliases.
        // utf_8 itself is a module name, so utf.8 is not a UTF-8 alias.
        key = key.replace('.', "_");
        if key == "utf_8" {
            return false;
        }
    }
    codec_kind(&key) == Some("utf-8")
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
#[derive(Debug)]
pub enum SourceDecodeError {
    Allocation,
    Null,
    Invalid(String, Option<SourceRange>),
    NonText(String),
    Unsupported(String),
}

impl From<std::collections::TryReserveError> for SourceDecodeError {
    fn from(_: std::collections::TryReserveError) -> Self {
        Self::Allocation
    }
}

#[derive(Debug)]
pub struct SourceRange {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
    // The tokenizer constructs its location before compile attaches the filename.
    pub tokenizer: bool,
}

fn decode_single_byte(
    bytes: &[u8],
    decode: impl Fn(usize, u8) -> Result<char, SourceDecodeError>,
) -> Result<Cow<'_, str>, SourceDecodeError> {
    let mut text = String::new();
    text.try_reserve_exact(bytes.len())?;
    for (pos, &byte) in bytes.iter().enumerate() {
        let ch = decode(pos, byte)?;
        text.try_reserve(ch.len_utf8())?;
        text.push(ch);
    }
    Ok(Cow::Owned(text))
}

/// The selected cookie, physical line and byte count scanned in the source header.
fn header<'a>(
    lines: &mut impl Iterator<Item = &'a [u8]>,
) -> (Option<&'a str>, &'a [u8], usize, usize) {
    let first = lines.next().unwrap_or_default();
    let encoding = cookie(first);
    if encoding.is_none() && matches!(trim_space(first).first(), None | Some(b'#' | b'\r' | b'\n'))
    {
        let second = lines.next().unwrap_or_default();
        (cookie(second), second, 2, first.len() + second.len())
    } else {
        (encoding, first, 1, first.len())
    }
}

pub fn declared_encoding(source: &str) -> Option<&str> {
    let bytes = source.as_bytes();
    header(
        &mut bytes
            .strip_prefix(BOM)
            .unwrap_or(bytes)
            .split_inclusive(|b| *b == b'\n'),
    )
    .0
}

pub fn decode_source(
    bytes: &[u8],
    utf8_error: impl FnOnce(&[u8], std::str::Utf8Error) -> String,
) -> Result<Cow<'_, str>, SourceDecodeError> {
    decode_source_with(bytes, utf8_error, None)
}

fn decode_source_with<'a>(
    bytes: &'a [u8],
    utf8_error: impl FnOnce(&[u8], std::str::Utf8Error) -> String,
    file: Option<&std::path::Path>,
) -> Result<Cow<'a, str>, SourceDecodeError> {
    let bom = bytes.starts_with(BOM);
    let bytes = bytes.strip_prefix(BOM).unwrap_or(bytes);
    let (declared, declaration_line, lineno, checked) =
        header(&mut bytes.split_inclusive(|b| *b == b'\n'));
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
            normalized = registry_name(name)?;
            match codec_kind(&normalized) {
                Some(kind) => kind,
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
            if name != "utf-8" && file.is_none() {
                let message = utf8_error(bytes, err);
                return SourceDecodeError::Invalid(message, None);
            }
            let pos = err.valid_up_to();
            let line = 1 + bytes[..pos].iter().filter(|&&b| b == b'\n').count();
            let start = bytes[..pos].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
            let offset = 1 + std::str::from_utf8(&bytes[start..pos]).unwrap().chars().count();
            let raw_line = bytes[start..].split(|&b| b == b'\n').next().unwrap_or_default();
            let byte = bytes[pos];
            let message = if file.is_some() && (declared.is_some() || bom) {
                // While looking for a second-line cookie, the tokenizer retains line one.
                let position = if line == 2 && lineno == 2 { pos } else { pos - start };
                let length = err.error_len().unwrap_or(bytes.len() - pos);
                let reason = if (0xc2..=0xf4).contains(&byte) { "invalid continuation byte" } else { "invalid start byte" };
                let span = if length == 1 { format!("byte 0x{byte:02x} in position {position}") }
                    else { format!("bytes in position {position}-{}", position + length - 1) };
                format!("(unicode error) 'utf-8' codec can't decode {span}: {reason}")
            } else {
                let origin = file.map_or_else(String::new, |p| format!(" in file {}", p.display()));
                format!("Non-UTF-8 code starting with '\\x{byte:02x}'{origin} on line {line}, but no encoding declared; see https://peps.python.org/pep-0263/ for details")
            };
            SourceDecodeError::Invalid(message, Some(SourceRange { line, start: offset, end: offset, text: String::from_utf8_lossy(raw_line).into_owned(), tokenizer: true }))
        }),
        "ascii" => {
            if let Some(pos) = bytes.iter().position(|b| !b.is_ascii()) {
                return Err(SourceDecodeError::Invalid(format!("'ascii' codec can't decode byte 0x{:02x} in position {pos}: ordinal not in range(128)", bytes[pos]), None));
            }
            Ok(Cow::Borrowed(std::str::from_utf8(bytes).unwrap()))
        }
        "iso8859-1" | "iso-8859-1" => decode_single_byte(bytes, |_, byte| Ok(char::from(byte))),
        "nontext" => Err(SourceDecodeError::NonText(format!("'{name}' is not a text encoding; use codecs.decode() to handle arbitrary codecs"))),
        "unsupported" => Err(SourceDecodeError::Unsupported(name.to_owned())),
        index => {
            let table = Charmap::at(index.parse().unwrap());
            decode_single_byte(bytes, |pos, byte| {
                table.decode(byte).ok_or_else(|| {
                    let message = format!(
                        "'charmap' codec can't decode byte 0x{byte:02x} in position {pos}: character maps to <undefined>"
                    );
                    SourceDecodeError::Invalid(message, None)
                })
            })
        }
    }
}

/// Compile scans physical lines after universal newline translation, before decoding.
fn compile_input(mut bytes: Cow<'_, [u8]>) -> Result<Cow<'_, [u8]>, SourceDecodeError> {
    if bytes.contains(&0) {
        return Err(SourceDecodeError::Null);
    }
    if bytes.contains(&b'\r') || !bytes.ends_with(b"\n") {
        if let Cow::Borrowed(slice) = bytes {
            let mut normalized = Vec::new();
            normalized.try_reserve_exact(slice.len().saturating_add(1))?;
            normalized.extend_from_slice(slice);
            bytes = Cow::Owned(normalized);
        }
        let normalized = bytes.to_mut();
        normalize_newlines(normalized);
        if !normalized.ends_with(b"\n") {
            normalized.try_reserve_exact(1)?;
            normalized.push(b'\n');
        }
    }
    Ok(bytes)
}

fn compile_text<'a>(
    input: &'a [u8],
    utf8_error: impl FnOnce(&[u8], std::str::Utf8Error) -> String,
    file: Option<&std::path::Path>,
) -> Result<Cow<'a, str>, SourceDecodeError> {
    let text = decode_source_with(input, utf8_error, file)?;
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
        Cow::Owned(universal_newlines(text.into_owned()))
    } else {
        text
    })
}

fn owned_text(text: Cow<'_, str>) -> Result<String, SourceDecodeError> {
    match text {
        Cow::Owned(text) => Ok(text),
        Cow::Borrowed(slice) => {
            let mut text = String::new();
            text.try_reserve_exact(slice.len())?;
            text.push_str(slice);
            Ok(text)
        }
    }
}

/// Decode Python compile input, transferring a reader's buffer when the codec borrows it whole.
pub fn decode_compile_source<'a>(
    bytes: impl Into<Cow<'a, [u8]>>,
    utf8_error: impl FnOnce(&[u8], std::str::Utf8Error) -> String,
) -> Result<String, SourceDecodeError> {
    let input = compile_input(bytes.into())?;
    let text = compile_text(&input, utf8_error, None)?;
    if std::ptr::eq(text.as_bytes(), input.as_ref())
        && let Cow::Owned(mut bytes) = input
    {
        bytes.shrink_to_fit();
        // SAFETY: the complete allocation is exactly the UTF-8 slice returned by the decoder.
        return Ok(unsafe { String::from_utf8_unchecked(bytes) });
    }
    owned_text(text)
}

/// Keep the original read buffer when the codec preserved every prepared byte.
/// Otherwise retain the raw bytes for the loader's source identity.
pub fn decode_disk_source(
    bytes: Vec<u8>,
    path: &std::path::Path,
) -> Result<(String, Option<Vec<u8>>), SourceDecodeError> {
    let input = compile_input(Cow::Borrowed(&bytes))?;
    let decoded = compile_text(&input, |_, error| error.to_string(), Some(path))?;
    if std::ptr::eq(decoded.as_bytes(), input.as_ref()) || decoded.as_bytes() == input.as_ref() {
        return Ok((
            String::from_utf8(bytes).expect("only ASCII line endings changed in valid UTF-8"),
            None,
        ));
    }
    let mut text = owned_text(decoded)?;
    if !bytes.ends_with(b"\n") && !bytes.ends_with(b"\r") {
        text.pop();
    }
    let raw = (bytes != text.as_bytes()).then_some(bytes);
    Ok((text, raw))
}

/// CR and CRLF shrink to a single ASCII byte, so conversion can reuse the decoded buffer.
pub fn universal_newlines(mut text: String) -> String {
    // SAFETY: newline translation only replaces ASCII CR and CRLF with ASCII LF.
    normalize_newlines(unsafe { text.as_mut_vec() });
    text
}

/// Compact physical CR and CRLF endings in place, even before the encoding is known.
fn normalize_newlines(bytes: &mut Vec<u8>) {
    let Some(first) = bytes.iter().position(|&b| b == b'\r') else {
        return;
    };
    let mut read = first;
    let mut written = first;
    while read < bytes.len() {
        let byte = bytes[read];
        read += 1;
        bytes[written] = if byte == b'\r' {
            read += usize::from(bytes.get(read) == Some(&b'\n'));
            b'\n'
        } else {
            byte
        };
        written += 1;
    }
    bytes.truncate(written);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_input_transfers_the_whole_utf8_buffer() {
        let bytes = b"answer = 42\n".to_vec();
        let original = bytes.as_ptr();
        let text = decode_compile_source(bytes, |_, err| err.to_string()).unwrap();
        assert_eq!(text.as_ptr(), original);
        let bom = b"\xef\xbb\xbfanswer = 42\r\n".to_vec();
        assert_eq!(
            decode_compile_source(bom, |_, err| err.to_string()).unwrap(),
            text
        );
    }
}
