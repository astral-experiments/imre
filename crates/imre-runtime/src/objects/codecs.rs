//! Native text codecs shared by the registry and text files.
pub(super) mod writer;

use super::bytes::{ascii_decode_error, decode_error, owned_input};
use super::charmaps::Charmap;
use super::exceptions::Result;
use super::{Bytes, Error, Kind, Object, Storage, StorageCell, Str, finish, object};
use crate::{BytesOwner, ObjectOwner, StrOwner};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;

static CP1251: LazyLock<Charmap> = LazyLock::new(|| Charmap::named("cp1251").unwrap());
static CP1251_ENCODE: LazyLock<HashMap<char, u8>> = LazyLock::new(|| {
    (0..=u8::MAX)
        .filter_map(|b| CP1251.decode(b).map(|ch| (ch, b)))
        .collect()
});

thread_local! {
    static REGISTRY: RefCell<HashMap<String, ObjectOwner>> = RefCell::default();
    static NAMES: [StrOwner; 5] = ["ascii", "iso8859-1", "utf-8", "utf-8-sig", "cp1251"].map(|s| StrOwner::new(Str::new(s)));
}

/// Collapse non-ASCII-alphanumeric separators, retaining dots in codec names.
fn normalize(name: &str) -> Result<String> {
    let mut normalized = String::new();
    normalized
        .try_reserve_exact(name.len())
        .map_err(|_| Error::allocation())?;
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
    Ok(normalized)
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
    let key = match normalize(name) {
        Ok(key) => key,
        Err(error) => return error.returning(std::ptr::null()),
    };
    REGISTRY.with_borrow_mut(|entries| {
        if let Some(entry) = entries.get(&key) {
            return ObjectOwner::into_raw(entry.clone());
        }
        match Codec::by_key(&key, name) {
            Ok(codec) => {
                if entries.try_reserve(1).is_err() {
                    return Error::allocation().returning(std::ptr::null());
                }
                let Ok(info) =
                    ObjectOwner::try_new(Object(StorageCell::new(Storage::CodecInfo(codec))))
                else {
                    return Error::memory_error().returning(std::ptr::null());
                };
                entries.insert(key, info.clone());
                ObjectOwner::into_raw(info)
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
    NAMES.with(|names| StrOwner::into_raw(names[index].clone()))
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
    let result = unsafe {
        codec(info).encode(&*text, Some(&*errors), || {
            Ok(StrOwner::acquire_word(text as usize))
        })
    };
    finish(result.and_then(encoded_bytes), std::ptr::null())
}

/// Return an owned byte buffer for a borrowed or freshly encoded result.
fn encoded_bytes(bytes: Cow<'_, [u8]>) -> Result<*const Bytes> {
    let owner = match bytes {
        Cow::Borrowed(bytes) => owned_input(bytes)?,
        Cow::Owned(bytes) => BytesOwner::try_new(bytes).map_err(|_| Error::memory_error())?,
    };
    Ok(BytesOwner::into_raw(owner))
}

/// Encode a string using the same codec implementation as registry entries and files.
///
/// # Safety
/// All three operands are rooted strings for the duration of the conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_encode_options(
    text: *const Str,
    encoding: *const Str,
    errors: *const Str,
) -> *const Bytes {
    // SAFETY: the typed compiler passes three live, immutable strings.
    let (encoding, errors) = unsafe { (&*encoding, &*errors) };
    if encoding.contains('\0') || errors.contains('\0') {
        return Error::new(Kind::ValueError, "embedded null character").returning(std::ptr::null());
    }
    let result = Codec::lookup(encoding).and_then(|codec| {
        // SAFETY: the caller roots the original string through conversion and error construction.
        unsafe { codec.encode(&*text, Some(errors), || Ok(StrOwner::acquire_word(text as usize))) }
    });
    finish(result.and_then(encoded_bytes), std::ptr::null())
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
        result.and_then(|s| {
            StrOwner::try_new(s.into())
                .map(StrOwner::into_raw)
                .map_err(|_| Error::allocation())
        }),
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
    fn index(self) -> usize {
        match self {
            Self::Ascii => 0,
            Self::Latin1 => 1,
            Self::Utf8 { signature: false } => 2,
            Self::Utf8 { signature: true } => 3,
            Self::Cp1251 => 4,
        }
    }

    pub(super) fn lookup(value: &str) -> Result<Self> {
        Self::by_key(&normalize(value)?, value)
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
            _ => Err(option_error(
                Kind::LookupError,
                "unknown encoding: ",
                value,
                "",
            )),
        }
    }

    /// Text streams accept the locale sentinel in addition to registry names.
    pub(super) fn for_stream(encoding: Option<&str>) -> Result<Self> {
        match encoding {
            None | Some("locale") => Ok(Self::Utf8 { signature: false }),
            Some(name) => Self::lookup(name),
        }
    }

    /// Decode a complete text stream, applying universal newlines when requested.
    pub(super) fn decode_stream(
        self,
        data: &[u8],
        errors: Option<&str>,
        universal: bool,
    ) -> Result<String> {
        let text = self.decode(data, errors)?;
        Ok(if universal && text.contains('\r') {
            universal_newlines(text)
        } else {
            text
        })
    }

    pub(super) fn decode(self, data: &[u8], errors: Option<&str>) -> Result<String> {
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
            Self::Cp1251 => {
                for (index, &byte) in data.iter().enumerate() {
                    if let Some(ch) = CP1251.decode(byte) {
                        text.try_reserve(ch.len_utf8())
                            .map_err(|_| Error::allocation())?;
                        text.push(ch);
                    } else {
                        decoding_error(&mut text, errors, &[byte], || match owned_input(data) {
                            Ok(bytes) => {
                                let reason = "character maps to <undefined>";
                                Error::unicode_decode(
                                    "charmap",
                                    bytes,
                                    index..index + 1,
                                    reason,
                                    format!(
                                        "'charmap' codec can't decode byte 0x{byte:02x} in position {index}: {reason}"
                                    ),
                                )
                            }
                            Err(error) => error,
                        })?;
                    }
                }
                Ok(text)
            }
            Self::Ascii => {
                for (index, &byte) in data.iter().enumerate() {
                    if byte.is_ascii() {
                        text.try_reserve(1).map_err(|_| Error::allocation())?;
                        text.push(char::from(byte));
                    } else {
                        decoding_error(&mut text, errors, &[byte], || match owned_input(data) {
                            Ok(bytes) => ascii_decode_error(bytes, index),
                            Err(error) => error,
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

    pub(super) fn encode<'a>(
        self,
        text: &'a str,
        errors: Option<&str>,
        error_input: impl FnOnce() -> Result<StrOwner>,
    ) -> Result<Cow<'a, [u8]>> {
        let (codec, reason) = match self {
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
            Self::Ascii => ("ascii", "ordinal not in range(128)"),
            Self::Latin1 => ("latin-1", "ordinal not in range(256)"),
            Self::Cp1251 => ("charmap", "character maps to <undefined>"),
        };
        let encoded = |ch: char| match self {
            Self::Ascii => ch.is_ascii().then_some(ch as u8),
            Self::Latin1 => u8::try_from(u32::from(ch)).ok(),
            Self::Cp1251 => CP1251_ENCODE.get(&ch).copied(),
            Self::Utf8 { .. } => unreachable!(),
        };
        let mut out = Vec::new();
        out.try_reserve_exact(text.len())
            .map_err(|_| Error::allocation())?;
        let mut chars = text.chars().enumerate().peekable();
        while let Some((start, ch)) = chars.next() {
            if let Some(byte) = encoded(ch) {
                out.try_reserve(1).map_err(|_| Error::allocation())?;
                out.push(byte);
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
                    while let Some((i, _)) = chars.next_if(|(_, c)| encoded(*c).is_none()) {
                        end = i;
                    }
                    let at = if start == end {
                        format!("character '{}' in position {start}", escaped(ch))
                    } else {
                        format!("characters in position {start}-{end}")
                    };
                    return Err(Error::unicode_encode(
                        codec,
                        error_input()?,
                        start..end + 1,
                        reason,
                        format!("'{codec}' codec can't encode {at}: {reason}"),
                    ));
                }
                name => return Err(unknown_handler(name)),
            }
        }
        Ok(Cow::Owned(out))
    }
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

fn escaped(ch: char) -> Str {
    let code = u32::from(ch);
    match code {
        0..=255 => compact_str::format_compact!("\\x{code:02x}"),
        256..=65535 => compact_str::format_compact!("\\u{code:04x}"),
        _ => compact_str::format_compact!("\\U{code:08x}"),
    }
}

/// Format potentially long user-supplied codec and file options with a fallible reservation.
pub(super) fn option_error(kind: Kind, prefix: &str, value: &str, suffix: &str) -> Error {
    let capacity = prefix
        .len()
        .saturating_add(value.len())
        .saturating_add(suffix.len());
    let Ok(mut message) = Str::try_with_capacity(capacity) else {
        return Error::allocation();
    };
    message.push_str(prefix);
    message.push_str(value);
    message.push_str(suffix);
    Error::new(kind, message)
}

fn unknown_handler(name: &str) -> Error {
    // CPython includes at most 400 UTF-8 bytes, ending before any partial character.
    let name = &name[..name.floor_char_boundary(400)];
    option_error(Kind::LookupError, "unknown error handler name '", name, "'")
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
        "namereplace" | "xmlcharrefreplace" => {
            return Err(Error::new(
                Kind::TypeError,
                "don't know how to handle UnicodeDecodeError in error callback",
            ));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_registry_errors_retain_text_and_copy_bytes() {
        let name = Str::new("ascii");
        let policy = Str::new("strict");
        let text = StrOwner::new(Str::new("xé€"));
        let bytes = BytesOwner::new(vec![b'a', 0xff]);
        // SAFETY: all ABI operands remain live until the returned owners are released.
        unsafe {
            let codec = ObjectOwner::from_raw(imre_codec_lookup(&name));
            assert!(
                imre_codec_encode(
                    ObjectOwner::as_ptr(&codec),
                    StrOwner::as_ptr(&text),
                    &policy
                )
                .is_null()
            );
            let error = super::super::exceptions::take_pending().unwrap();
            assert_eq!(StrOwner::strong_count(&text), 2);
            drop(error);
            assert_eq!(StrOwner::strong_count(&text), 1);
            assert!(
                imre_codec_decode(
                    ObjectOwner::as_ptr(&codec),
                    BytesOwner::as_ptr(&bytes),
                    &policy
                )
                .is_null()
            );
            let _error = super::super::exceptions::take_pending().unwrap();
            assert_eq!(BytesOwner::strong_count(&bytes), 1);
        }
    }
}
