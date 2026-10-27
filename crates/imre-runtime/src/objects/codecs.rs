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

pub(crate) struct CodePage {
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

const CODEC_NAMES: [&str; 6] = [
    "ascii",
    "iso8859-1",
    "utf-8",
    "utf-8-sig",
    "cp1251",
    "cp424",
];

thread_local! {
    static REGISTRY: RefCell<HashMap<String, ObjectOwner>> = RefCell::default();
    static NAMES: [StrOwner; CODEC_NAMES.len()] = CODEC_NAMES.map(|name| {
        // Python interns identifiers in encoding modules, such as ascii and cp1251.
        if name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
            crate::sys::canonical(name)
        } else {
            StrOwner::new(Str::const_new(name))
        }
    });
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

/// An owned entry in the native registry, shared by typed and boxed callers.
#[derive(Clone)]
pub(crate) struct CodecInfo(ObjectOwner);

impl CodecInfo {
    pub(crate) fn lookup(name: &str) -> Result<Self> {
        if name.contains('\0') {
            return Err(Error::new(Kind::ValueError, "embedded null character"));
        }
        let key = normalize(name)?;
        REGISTRY.with_borrow_mut(|entries| {
            if let Some(entry) = entries.get(&key) {
                return Ok(Self(entry.clone()));
            }
            let codec = Codec::by_key(&key, name)?;
            entries.try_reserve(1).map_err(|_| Error::allocation())?;
            let info = ObjectOwner::try_new(Object(StorageCell::new(Storage::CodecInfo(codec))))
                .map_err(|_| Error::memory_error())?;
            entries.insert(key, info.clone());
            Ok(Self(info))
        })
    }

    pub(crate) fn as_ptr(&self) -> *const Object {
        ObjectOwner::as_ptr(&self.0)
    }

    pub(crate) fn codec(&self) -> Codec {
        // SAFETY: lookup constructs and retains only CodecInfo storage.
        unsafe { codec(self.as_ptr()) }
    }
}

/// # Safety
/// The caller roots the encoding name until the registry returns an owned entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_lookup(name: *const Str) -> *const Object {
    // SAFETY: the native name is borrowed from the generated call.
    let info = CodecInfo::lookup(unsafe { &*name });
    finish(
        info.map(|entry| ObjectOwner::into_raw(entry.0)),
        std::ptr::null(),
    )
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
    StrOwner::into_raw(unsafe { codec(info) }.name_owner())
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
pub(super) fn encoded_bytes(bytes: Cow<'_, [u8]>) -> Result<*const Bytes> {
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
    let result = Codec::for_encode(encoding, errors).and_then(|codec| {
        // SAFETY: the caller roots the original string through conversion and error construction.
        unsafe {
            codec.encode(&*text, Some(errors), || {
                Ok(StrOwner::acquire_word(text as usize))
            })
        }
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
pub(crate) enum Codec {
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

    /// Return the retained canonical name shared with registry entries.
    pub(crate) fn name_owner(self) -> StrOwner {
        NAMES.with(|names| names[self.index()].clone())
    }

    pub(crate) fn name(self) -> &'static str {
        CODEC_NAMES[self.index()]
    }

    pub(crate) fn lookup(value: &str) -> Result<Self> {
        Self::by_key(&normalize(value)?, value)
    }

    fn for_encode(encoding: &str, errors: &str) -> Result<Self> {
        if encoding.contains('\0') || errors.contains('\0') {
            return Err(Error::new(Kind::ValueError, "embedded null character"));
        }
        Self::lookup(encoding)
    }

    /// Boxed strings retain a native copy of the input only when the encoder raises.
    pub(crate) fn encode_str(text: &str, encoding: &str, errors: &str) -> Result<Vec<u8>> {
        let codec = Self::for_encode(encoding, errors)?;
        match codec.encode(text, Some(errors), || super::text::try_owned_string(text))? {
            Cow::Owned(bytes) => Ok(bytes),
            Cow::Borrowed(bytes) => {
                let mut output = super::bytes::allocate(bytes.len())?;
                output.extend_from_slice(bytes);
                Ok(output)
            }
        }
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

    pub(crate) fn decode(self, data: &[u8], errors: Option<&str>) -> Result<String> {
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
            Self::CodePage(page) => {
                for (index, &byte) in data.iter().enumerate() {
                    if let Some(ch) = page.decode.decode(byte) {
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

    pub(crate) fn encode<'a>(
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
            Self::CodePage(_) => ("charmap", "character maps to <undefined>"),
        };
        let encoded = |ch: char| match self {
            Self::Ascii => ch.is_ascii().then_some(ch as u8),
            Self::Latin1 => u8::try_from(u32::from(ch)).ok(),
            Self::CodePage(page) => page.encode.get(&ch).copied(),
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
            let replacement = match errors.unwrap_or("strict") {
                "ignore" => continue,
                "replace" => Str::const_new("?"),
                "backslashreplace" => escaped(ch),
                "xmlcharrefreplace" => compact_str::format_compact!("&#{};", u32::from(ch)),
                "namereplace" => unicode_names2::name(ch).map_or_else(
                    || escaped(ch),
                    |name| compact_str::format_compact!("\\N{{{name}}}"),
                ),
                "strict" | "surrogatepass" | "surrogateescape" => {
                    let mut end = start;
                    while let Some((i, _)) = chars.next_if(|(_, c)| encoded(*c).is_none()) {
                        end = i;
                    }
                    return Err(encoding_error(
                        codec,
                        reason,
                        error_input()?,
                        start,
                        end,
                        ch,
                    ));
                }
                name => return Err(unknown_handler(name)),
            };
            out.try_reserve(replacement.len())
                .map_err(|_| Error::allocation())?;
            for c in replacement.chars() {
                let Some(byte) = encoded(c) else {
                    return Err(encoding_error(
                        codec,
                        reason,
                        error_input()?,
                        start,
                        start,
                        ch,
                    ));
                };
                out.push(byte);
            }
        }
        Ok(Cow::Owned(out))
    }
}

/// Report the source string and scalar positions that the codec could not represent.
fn encoding_error(
    codec: &str,
    reason: &str,
    source: StrOwner,
    start: usize,
    end: usize,
    ch: char,
) -> Error {
    let at = if start == end {
        format!("character '{}' in position {start}", escaped(ch))
    } else {
        format!("characters in position {start}-{end}")
    };
    Error::unicode_encode(
        codec,
        source,
        start..end + 1,
        reason,
        format!("'{codec}' codec can't encode {at}: {reason}"),
    )
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
        "surrogateescape" => {
            return Err(Error::new(
                Kind::NotImplementedError,
                "native strings cannot represent surrogate-escaped input bytes",
            ));
        }
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
    fn strict_str_encode_retains_the_original_text_until_the_error_is_dropped() {
        let text = StrOwner::new(Str::new("xé€"));
        let name = Str::new("ascii");
        let policy = Str::new("strict");
        // SAFETY: all three arguments remain live until encoding returns.
        let result = unsafe { imre_str_encode_options(StrOwner::as_ptr(&text), &name, &policy) };
        assert!(result.is_null());
        let error = super::super::exceptions::take_pending().unwrap();
        assert_eq!(StrOwner::strong_count(&text), 2);
        drop(error);
        assert_eq!(StrOwner::strong_count(&text), 1);
    }

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
