//! Whole-file text conversion uses the same descriptor, buffering, and close path as binary I/O.

use std::borrow::Cow;

use super::super::bytes::decode_error;
use super::{Error, Int, Kind, Object, Rc, Result, Str, finish, object, with_path_file};

#[derive(Clone, Copy)]
enum Codec {
    Utf8 { signature: bool },
    Latin1,
    Ascii,
}

impl Codec {
    fn lookup(name: Option<&str>) -> Result<Self> {
        let value = name.unwrap_or("utf-8");
        let key = value.to_ascii_lowercase().replace(['-', ' '], "_");
        match key.as_str() {
            "utf8" | "utf_8" | "u8" | "utf" | "utf8_ucs2" | "utf8_ucs4" | "cp65001" | "locale" => {
                Ok(Self::Utf8 { signature: false })
            }
            "utf_8_sig" => Ok(Self::Utf8 { signature: true }),
            "latin1" | "latin" | "iso8859" | "latin_1" | "l1" | "iso8859_1" | "iso_8859_1"
            | "iso_8859_1_1987" | "8859" | "cp819" | "ibm819" | "iso_ir_100" | "csisolatin1" => {
                Ok(Self::Latin1)
            }
            "ascii" | "us" | "us_ascii" | "646" | "ansi_x3.4_1968" | "ansi_x3_4_1968"
            | "ansi_x3.4_1986" | "cp367" | "csascii" | "ibm367" | "iso646_us"
            | "iso_646.irv_1991" | "iso_ir_6" => Ok(Self::Ascii),
            _ => Err(Error::new(
                Kind::LookupError,
                format!("unknown encoding: {value}"),
            )),
        }
    }

    fn decode(self, data: &[u8], errors: Option<&str>) -> Result<String> {
        let capacity = if matches!(self, Self::Latin1) {
            data.len()
                .checked_add(data.iter().filter(|byte| !byte.is_ascii()).count())
                .ok_or_else(Error::allocation)?
        } else {
            data.len()
        };
        let mut text = String::new();
        text.try_reserve_exact(capacity)
            .map_err(|_| Error::allocation())?;
        match self {
            Self::Latin1 => {
                text.extend(data.iter().copied().map(char::from));
                Ok(text)
            }
            Self::Ascii => {
                for (index, &byte) in data.iter().enumerate() {
                    if byte.is_ascii() {
                        text.try_reserve(1).map_err(|_| Error::allocation())?;
                        text.push(char::from(byte));
                    } else {
                        decoding_error(&mut text, errors, &[byte], || {
                            Error::new(
                                Kind::UnicodeDecodeError,
                                format!(
                                    "'ascii' codec can't decode byte 0x{byte:02x} in position {index}: ordinal not in range(128)"
                                ),
                            )
                        })?;
                    }
                }
                Ok(text)
            }
            Self::Utf8 { signature } => {
                let data = if signature {
                    data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data)
                } else {
                    data
                };
                let mut remaining = data;
                loop {
                    match std::str::from_utf8(remaining) {
                        Ok(valid) => {
                            text.try_reserve(valid.len())
                                .map_err(|_| Error::allocation())?;
                            text.push_str(valid);
                            return Ok(text);
                        }
                        Err(error) => {
                            let valid = error.valid_up_to();
                            text.try_reserve(valid).map_err(|_| Error::allocation())?;
                            text.push_str(std::str::from_utf8(&remaining[..valid]).unwrap());
                            let end = error.error_len().map_or(remaining.len(), |len| valid + len);
                            decoding_error(&mut text, errors, &remaining[valid..end], || {
                                decode_error(data, error)
                            })?;
                            remaining = &remaining[end..];
                        }
                    }
                }
            }
        }
    }

    fn encode<'a>(self, text: &'a str, errors: Option<&str>) -> Result<Cow<'a, [u8]>> {
        let (codec, limit) = match self {
            Self::Utf8 { signature: false } => return Ok(Cow::Borrowed(text.as_bytes())),
            Self::Utf8 { signature: true } => {
                let mut out = Vec::new();
                let len = text.len().checked_add(3).ok_or_else(Error::allocation)?;
                out.try_reserve_exact(len)
                    .map_err(|_| Error::allocation())?;
                out.extend_from_slice(b"\xef\xbb\xbf");
                out.extend_from_slice(text.as_bytes());
                return Ok(Cow::Owned(out));
            }
            Self::Ascii => ("ascii", 128),
            Self::Latin1 => ("latin-1", 256),
        };
        let mut out = Vec::new();
        out.try_reserve_exact(text.len())
            .map_err(|_| Error::allocation())?;
        let mut chars = text.chars().enumerate().peekable();
        while let Some((start, ch)) = chars.next() {
            if u32::from(ch) < limit {
                out.try_reserve(1).map_err(|_| Error::allocation())?;
                out.push(ch as u8);
                continue;
            }
            match errors.unwrap_or("strict") {
                "ignore" => (),
                "replace" => {
                    out.try_reserve(1).map_err(|_| Error::allocation())?;
                    out.push(b'?');
                }
                "backslashreplace" | "xmlcharrefreplace" => {
                    let replacement = if errors == Some("backslashreplace") {
                        escaped(ch)
                    } else {
                        compact_str::format_compact!("&#{};", u32::from(ch))
                    };
                    out.try_reserve(replacement.len())
                        .map_err(|_| Error::allocation())?;
                    out.extend_from_slice(replacement.as_bytes());
                }
                "strict" | "surrogatepass" | "surrogateescape" => {
                    let mut end = start;
                    while let Some((i, _)) = chars.next_if(|(_, c)| u32::from(*c) >= limit) {
                        end = i;
                    }
                    let at = if start == end {
                        format!("character '{}' in position {start}", escaped(ch))
                    } else {
                        format!("characters in position {start}-{end}")
                    };
                    return Err(Error::new(
                        Kind::UnicodeEncodeError,
                        format!("'{codec}' codec can't encode {at}: ordinal not in range({limit})"),
                    ));
                }
                name => return Err(unknown_handler(name)),
            }
        }
        Ok(Cow::Owned(out))
    }
}

fn escaped(ch: char) -> Str {
    let code = u32::from(ch);
    match code {
        0..=255 => compact_str::format_compact!("\\x{code:02x}"),
        256..=65535 => compact_str::format_compact!("\\u{code:04x}"),
        _ => compact_str::format_compact!("\\U{code:08x}"),
    }
}

fn unknown_handler(name: &str) -> Error {
    Error::new(
        Kind::LookupError,
        format!("unknown error handler name '{name}'"),
    )
}

fn decoding_error(
    text: &mut String,
    errors: Option<&str>,
    bytes: &[u8],
    strict: impl FnOnce() -> Error,
) -> Result<()> {
    match errors.unwrap_or("strict") {
        "strict" | "surrogatepass" => return Err(strict()),
        "replace" => {
            text.try_reserve(3).map_err(|_| Error::allocation())?;
            text.push('\u{fffd}');
        }
        "ignore" => (),
        "backslashreplace" => {
            let len = bytes.len().checked_mul(4).ok_or_else(Error::allocation)?;
            text.try_reserve(len).map_err(|_| Error::allocation())?;
            for byte in bytes {
                use std::fmt::Write;
                write!(text, "\\x{byte:02x}").unwrap();
            }
        }
        name => return Err(unknown_handler(name)),
    }
    Ok(())
}

/// CR and CRLF shrink to a single ASCII byte, so conversion can reuse the decoded buffer.
fn universal_newlines(text: String) -> String {
    let mut bytes = text.into_bytes();
    let mut read = 0;
    let mut written = 0;
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
    String::from_utf8(bytes).expect("ASCII newline conversion preserves UTF-8")
}

fn output_newlines(text: &str, replacement: &str) -> Result<String> {
    let extra = if replacement.len() == 2 {
        text.bytes().filter(|&byte| byte == b'\n').count()
    } else {
        0
    };
    let mut result = String::new();
    let len = text
        .len()
        .checked_add(extra)
        .ok_or_else(Error::allocation)?;
    result
        .try_reserve_exact(len)
        .map_err(|_| Error::allocation())?;
    for part in text.split_inclusive('\n') {
        if let Some(line) = part.strip_suffix('\n') {
            result.push_str(line);
            result.push_str(replacement);
        } else {
            result.push_str(part);
        }
    }
    Ok(result)
}

fn options(encoding: Option<&str>, newline: Option<&str>) -> Result<Codec> {
    if let Some(value) = newline
        && !matches!(value, "" | "\n" | "\r" | "\r\n")
    {
        return Err(Error::new(
            Kind::ValueError,
            format!("illegal newline value: {value}"),
        ));
    }
    Codec::lookup(encoding)
}

// An optional native string is an owned pointer word, or zero for None.
unsafe fn string_option<'a>(word: usize) -> Option<&'a str> {
    // SAFETY: the caller roots each non-null string until its I/O operation returns.
    unsafe { (word as *const Str).as_ref() }.map(Str::as_str)
}

pub(super) fn read(
    path: &str,
    encoding: Option<&str>,
    errors: Option<&str>,
    newline: Option<&str>,
) -> Result<Str> {
    with_path_file(path, 0, true, |file| {
        let codec = options(encoding, newline)?;
        let data = file.input()?.read_bytes(None)?;
        let text = codec.decode(&data, errors)?;
        let text = if newline.is_none() && text.contains('\r') {
            universal_newlines(text)
        } else {
            text
        };
        Ok(Str::from(text))
    })
}

/// # Safety
/// Root the path and each nonzero optional string through the read and close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_read_text_options(
    path: *const Object,
    encoding: usize,
    errors: usize,
    newline: usize,
) -> *const Str {
    // SAFETY: the path and options remain immutably rooted for synchronous I/O.
    let (path, encoding, errors, newline) = unsafe {
        (
            object(path),
            string_option(encoding),
            string_option(errors),
            string_option(newline),
        )
    };
    // SAFETY: no language callback can mutate the rooted path during file I/O.
    let result = unsafe {
        path.0
            .with(|storage| read(storage.path(), encoding, errors, newline))
    };
    finish(
        result.map(|text| Rc::into_raw(Rc::new(text))),
        std::ptr::null(),
    )
}

/// # Safety
/// Root the path, text and each nonzero optional string through the write and close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_text_options(
    path: *const Object,
    text: *const Str,
    encoding: usize,
    errors: usize,
    newline: usize,
) -> usize {
    // SAFETY: the path, data and options remain immutably rooted for synchronous I/O.
    let (path, text, encoding, errors, newline) = unsafe {
        (
            object(path),
            &*text,
            string_option(encoding),
            string_option(errors),
            string_option(newline),
        )
    };
    // SAFETY: no language callback can mutate the rooted path during file I/O.
    let result = unsafe {
        path.0.with(|storage| {
            with_path_file(storage.path(), 1, true, |file| {
                let codec = options(encoding, newline)?;
                let converted;
                let value = if let Some(replacement @ ("\r" | "\r\n")) = newline {
                    converted = output_newlines(text, replacement)?;
                    &converted
                } else {
                    text.as_str()
                };
                file.write_bytes(&codec.encode(value, errors)?)?;
                Ok(Int::from_usize(text.chars().count()).into_raw())
            })
        })
    };
    finish(result, 0)
}

/// # Safety
/// The diagnostic string stays rooted until the pending TypeError owns its message.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_type_error(message: *const Str) -> usize {
    // SAFETY: the compiler supplies an immutable rooted string.
    finish(
        Err(Error::new(Kind::TypeError, unsafe { &*message }.as_str())),
        0,
    )
}

/// # Safety
/// The data is a rooted optional str/bytes union; tag zero is str and tag one is bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_bytes_union(path: *const Object, data: usize) -> usize {
    if data & 7 == 1 {
        // SAFETY: removing the tag restores the rooted bytes pointer.
        return unsafe {
            super::super::paths::imre_path_write_bytes(path, (data & !7) as *const _)
        };
    }
    let actual = if data == 0 { "NoneType" } else { "str" };
    finish(
        Err(Error::new(
            Kind::TypeError,
            format!("memoryview: a bytes-like object is required, not '{actual}'"),
        )),
        0,
    )
}

/// # Safety
/// The path, optional str/bytes word, and text options stay rooted through this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_text_union(
    path: *const Object,
    data: usize,
    encoding: usize,
    errors: usize,
    newline: usize,
) -> usize {
    if data != 0 && data & 7 == 0 {
        // SAFETY: the nonzero untagged word is the rooted string.
        return unsafe {
            imre_path_write_text_options(path, data as *const Str, encoding, errors, newline)
        };
    }
    let actual = if data == 0 { "NoneType" } else { "bytes" };
    finish(
        Err(Error::new(
            Kind::TypeError,
            format!("data must be str, not {actual}"),
        )),
        0,
    )
}
