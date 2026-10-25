//! Native text codecs shared by the registry and text files.

mod cp1251;

use super::bytes::decode_error;
use super::exceptions::Result;
use super::{Bytes, Error, Kind, Object, Rc, Storage, StorageCell, Str, finish, object};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static REGISTRY: RefCell<HashMap<String, Rc<Object>>> = RefCell::default();
    static NAMES: [Rc<Str>; 5] = ["ascii", "iso8859-1", "utf-8", "utf-8-sig", "cp1251"].map(|s| Rc::new(Str::new(s)));
}

/// Collapse non-ASCII-alphanumeric separators, retaining dots in codec names.
fn normalize(name: &str) -> String {
    let mut normalized = String::new();
    let mut separated = false;
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'.' {
            if separated && !normalized.is_empty() {
                normalized.push('_');
            }
            normalized.push(char::from(byte.to_ascii_lowercase()));
            separated = false;
        } else {
            separated = true;
        }
    }
    normalized
}

/// # Safety
/// The caller roots the encoding name until the registry returns an owned entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_lookup(name: *const Str) -> *const Object {
    // SAFETY: the native name is borrowed from the generated call.
    let name = unsafe { &*name };
    if name.contains('\0') {
        return Error::new(Kind::ValueError, "embedded null character").returning(std::ptr::null());
    }
    let key = normalize(name);
    REGISTRY.with_borrow_mut(|entries| {
        if let Some(entry) = entries.get(&key) {
            return Rc::into_raw(entry.clone());
        }
        match Codec::by_key(&key, name) {
            Ok(codec) => {
                let info = Rc::new(Object(StorageCell::new(Storage::CodecInfo(codec))));
                entries.insert(key, info.clone());
                Rc::into_raw(info)
            }
            Err(e) => e.returning(std::ptr::null()),
        }
    })
}

/// # Safety
/// A rooted registry entry retains its codec throughout the read.
unsafe fn codec(info: *const Object) -> Codec {
    // SAFETY: the closed native type admits only CodecInfo storage.
    unsafe {
        object(info).0.with(|s| match s {
            Storage::CodecInfo(value) => *value,
            _ => unreachable!("expected a codec registry entry"),
        })
    }
}

/// # Safety
/// The caller roots the codec info until its canonical name is read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_name(info: *const Object) -> *const Str {
    // SAFETY: the codec info was returned by the registry.
    let index = match unsafe { codec(info) } {
        Codec::Ascii => 0,
        Codec::Latin1 => 1,
        Codec::Utf8 { signature: false } => 2,
        Codec::Utf8 { signature: true } => 3,
        Codec::Cp1251 => 4,
    };
    NAMES.with(|names| Rc::into_raw(names[index].clone()))
}

/// # Safety
/// The caller roots a codec info and two native strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_encode(
    info: *const Object,
    text: *const Str,
    errors: *const Str,
) -> *const Bytes {
    // SAFETY: the typed operands stay live until conversion is complete.
    let result = unsafe { codec(info).encode(&*text, Some(&*errors)) };
    finish(
        result.map(|b| Rc::into_raw(Rc::new(b.into_owned()))),
        std::ptr::null(),
    )
}

/// # Safety
/// The caller roots a codec info, byte buffer, and error handler name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_decode(
    info: *const Object,
    bytes: *const Bytes,
    errors: *const Str,
) -> *const Str {
    // SAFETY: the generated call retains all three operands.
    let result = unsafe { codec(info).decode(&*bytes, Some(&*errors)) };
    finish(
        result.map(|s| Rc::into_raw(Rc::new(s.into()))),
        std::ptr::null(),
    )
}

#[derive(Clone, Copy)]
pub(super) enum Codec {
    Utf8 { signature: bool },
    Latin1,
    Ascii,
    Cp1251,
}

impl Codec {
    pub(super) fn lookup(value: &str) -> Result<Self> {
        Self::by_key(&normalize(value), value)
    }

    fn by_key(key: &str, value: &str) -> Result<Self> {
        match key {
            "utf8" | "utf_8" | "u8" | "utf" | "utf8_ucs2" | "utf8_ucs4" | "cp65001" => {
                Ok(Self::Utf8 { signature: false })
            }
            "utf_8_sig" => Ok(Self::Utf8 { signature: true }),
            "cp1251" | "1251" | "windows_1251" => Ok(Self::Cp1251),
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

    pub(super) fn decode(self, data: &[u8], errors: Option<&str>) -> Result<String> {
        match self {
            Self::Latin1 => Ok(data.iter().copied().map(char::from).collect()),
            Self::Ascii | Self::Cp1251 => {
                let mut text = String::with_capacity(data.len());
                for (index, &byte) in data.iter().enumerate() {
                    let mapped = match self {
                        Self::Ascii => byte.is_ascii().then(|| char::from(byte)),
                        Self::Cp1251 => cp1251::decode(byte),
                        _ => unreachable!(),
                    };
                    if let Some(ch) = mapped {
                        text.push(ch);
                    } else {
                        decoding_error(&mut text, errors, &[byte], || {
                            let (codec, reason) = self.mapping_error();
                            Error::new(
                                Kind::UnicodeDecodeError,
                                format!(
                                    "'{codec}' codec can't decode byte 0x{byte:02x} in position {index}: {reason}"
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
                let mut text = String::with_capacity(data.len());
                loop {
                    match std::str::from_utf8(remaining) {
                        Ok(valid) => {
                            text.push_str(valid);
                            return Ok(text);
                        }
                        Err(error) => {
                            let valid = error.valid_up_to();
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

    pub(super) fn encode<'a>(self, text: &'a str, errors: Option<&str>) -> Result<Cow<'a, [u8]>> {
        match self {
            Self::Utf8 { signature: false } => return Ok(Cow::Borrowed(text.as_bytes())),
            Self::Utf8 { signature: true } => {
                return Ok(Cow::Owned([b"\xef\xbb\xbf", text.as_bytes()].concat()));
            }
            Self::Ascii | Self::Latin1 | Self::Cp1251 => (),
        }
        let mut out = Vec::with_capacity(text.len());
        let mut chars = text.chars().enumerate().peekable();
        while let Some((start, ch)) = chars.next() {
            if let Some(byte) = self.encode_byte(ch) {
                out.push(byte);
                continue;
            }
            match errors.unwrap_or("strict") {
                "ignore" => (),
                "replace" => out.push(b'?'),
                "backslashreplace" => out.extend(escaped(ch).as_bytes()),
                "xmlcharrefreplace" => out.extend(format!("&#{};", u32::from(ch)).as_bytes()),
                "strict" | "surrogatepass" | "surrogateescape" => {
                    let mut end = start;
                    while let Some((i, _)) = chars.next_if(|(_, c)| self.encode_byte(*c).is_none())
                    {
                        end = i;
                    }
                    let at = if start == end {
                        format!("character '{}' in position {start}", escaped(ch))
                    } else {
                        format!("characters in position {start}-{end}")
                    };
                    let (codec, reason) = self.mapping_error();
                    return Err(Error::new(
                        Kind::UnicodeEncodeError,
                        format!("'{codec}' codec can't encode {at}: {reason}"),
                    ));
                }
                name => return Err(unknown_handler(name)),
            }
        }
        Ok(Cow::Owned(out))
    }

    fn encode_byte(self, ch: char) -> Option<u8> {
        match self {
            Self::Ascii => ch.is_ascii().then_some(ch as u8),
            Self::Latin1 => u8::try_from(ch).ok(),
            Self::Cp1251 => cp1251::encode(ch),
            Self::Utf8 { .. } => unreachable!(),
        }
    }

    fn mapping_error(self) -> (&'static str, &'static str) {
        match self {
            Self::Ascii => ("ascii", "ordinal not in range(128)"),
            Self::Latin1 => ("latin-1", "ordinal not in range(256)"),
            Self::Cp1251 => ("charmap", "character maps to <undefined>"),
            Self::Utf8 { .. } => unreachable!(),
        }
    }
}

fn escaped(ch: char) -> String {
    let code = u32::from(ch);
    match code {
        0..=255 => format!("\\x{code:02x}"),
        256..=65535 => format!("\\u{code:04x}"),
        _ => format!("\\U{code:08x}"),
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
        "replace" => text.push('\u{fffd}'),
        "ignore" => (),
        "xmlcharrefreplace" | "namereplace" => {
            return Err(Error::new(
                Kind::TypeError,
                "don't know how to handle UnicodeDecodeError in error callback",
            ));
        }
        "backslashreplace" => {
            for byte in bytes {
                use std::fmt::Write;
                write!(text, "\\x{byte:02x}").unwrap();
            }
        }
        name => return Err(unknown_handler(name)),
    }
    Ok(())
}
