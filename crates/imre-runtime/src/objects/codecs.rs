//! Native text codecs shared by the registry and text files.
pub(super) mod writer;

use super::bytes::decode_error;
use super::charmaps::Charmap;
use super::exceptions::Result;
use super::{Bytes, Error, Kind, Object, Rc, Storage, StorageCell, Str, finish, object};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;

pub(super) struct CodePage {
    index: usize,
    decode: Charmap,
    encode: HashMap<char, u8>,
}

impl CodePage {
    /// Reuse the pinned source decoding table in both directions.
    fn new(index: usize, name: &str) -> Self {
        let decode = Charmap::named(name).unwrap();
        let encode = (0..=u8::MAX)
            .filter_map(|byte| decode.decode(byte).map(|ch| (ch, byte)))
            .collect();
        Self {
            index,
            decode,
            encode,
        }
    }
}

static CP1251: LazyLock<CodePage> = LazyLock::new(|| CodePage::new(4, "cp1251"));
static CP424: LazyLock<CodePage> = LazyLock::new(|| CodePage::new(5, "cp424"));

thread_local! {
    static REGISTRY: RefCell<HashMap<String, Rc<Object>>> = RefCell::default();
    static NAMES: [Rc<Str>; 6] = ["ascii", "iso8859-1", "utf-8", "utf-8-sig", "cp1251", "cp424"].map(|s| Rc::new(Str::new(s)));
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
    let index = unsafe { codec(info) }.index();
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
    CodePage(&'static CodePage),
}

impl Codec {
    fn index(self) -> usize {
        match self {
            Self::Ascii => 0,
            Self::Latin1 => 1,
            Self::Utf8 { signature: false } => 2,
            Self::Utf8 { signature: true } => 3,
            Self::CodePage(page) => page.index,
        }
    }

    pub(super) fn lookup(value: &str) -> Result<Self> {
        Self::by_key(&normalize(value), value)
    }

    fn by_key(key: &str, value: &str) -> Result<Self> {
        match key {
            "utf8" | "utf_8" | "u8" | "utf" | "utf8_ucs2" | "utf8_ucs4" | "cp65001" => {
                Ok(Self::Utf8 { signature: false })
            }
            "utf_8_sig" => Ok(Self::Utf8 { signature: true }),
            "cp1251" | "1251" | "windows_1251" => Ok(Self::CodePage(&CP1251)),
            "cp424" | "424" | "csibm424" | "ebcdic_cp_he" | "ibm424" => Ok(Self::CodePage(&CP424)),
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
            Self::CodePage(page) => {
                let mut text = String::with_capacity(data.len());
                for (i, &byte) in data.iter().enumerate() {
                    if let Some(ch) = page.decode.decode(byte) {
                        text.push(ch);
                    } else {
                        decoding_error(&mut text, errors, &[byte], || {
                            Error::new(
                                Kind::UnicodeDecodeError,
                                format!(
                                    "'charmap' codec can't decode byte 0x{byte:02x} in position {i}: character maps to <undefined>"
                                ),
                            )
                        })?;
                    }
                }
                Ok(text)
            }
            Self::Latin1 => Ok(data.iter().copied().map(char::from).collect()),
            Self::Ascii => {
                let mut text = String::with_capacity(data.len());
                for (index, &byte) in data.iter().enumerate() {
                    if byte.is_ascii() {
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
        let (codec, reason) = match self {
            Self::Utf8 { signature: false } => return Ok(Cow::Borrowed(text.as_bytes())),
            Self::Utf8 { signature: true } => {
                return Ok(Cow::Owned([b"\xef\xbb\xbf", text.as_bytes()].concat()));
            }
            Self::Ascii => ("ascii", "ordinal not in range(128)"),
            Self::Latin1 => ("latin-1", "ordinal not in range(256)"),
            Self::CodePage(_) => ("charmap", "character maps to <undefined>"),
        };
        let encoded = |ch: char| match self {
            Self::Ascii => ch.is_ascii().then_some(ch as u8),
            Self::Latin1 => u8::try_from(u32::from(ch)).ok(),
            Self::CodePage(page) => page.encode.get(&ch).copied(),
            Self::Utf8 { .. } => unreachable!(),
        };
        let mut out = Vec::with_capacity(text.len());
        let mut chars = text.chars().enumerate().peekable();
        while let Some((start, ch)) = chars.next() {
            if let Some(byte) = encoded(ch) {
                out.push(byte);
                continue;
            }
            let replacement: Cow<'_, str> = match errors.unwrap_or("strict") {
                "ignore" => continue,
                "replace" => "?".into(),
                "backslashreplace" => escaped(ch).into(),
                "xmlcharrefreplace" => format!("&#{};", u32::from(ch)).into(),
                "namereplace" => unicode_names2::name(ch)
                    .map_or_else(|| escaped(ch), |name| format!("\\N{{{name}}}"))
                    .into(),
                "strict" | "surrogatepass" | "surrogateescape" => {
                    let mut end = start;
                    while let Some((i, _)) = chars.next_if(|(_, c)| encoded(*c).is_none()) {
                        end = i;
                    }
                    return Err(encoding_error(codec, reason, start, end, ch));
                }
                name => return Err(unknown_handler(name)),
            };
            for c in replacement.chars() {
                out.push(
                    encoded(c).ok_or_else(|| encoding_error(codec, reason, start, start, ch))?,
                );
            }
        }
        Ok(Cow::Owned(out))
    }
}

fn encoding_error(codec: &str, reason: &str, start: usize, end: usize, ch: char) -> Error {
    let at = if start == end {
        format!("character '{}' in position {start}", escaped(ch))
    } else {
        format!("characters in position {start}-{end}")
    };
    Error::new(
        Kind::UnicodeEncodeError,
        format!("'{codec}' codec can't encode {at}: {reason}"),
    )
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
