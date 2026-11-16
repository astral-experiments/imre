//! Native text codecs shared by the registry and text files.
mod double_byte;
pub(crate) mod idna;
pub(crate) use double_byte::DoubleByte;
mod wide;
pub(super) mod writer;

use super::bytes::{DecodeIssue, ascii_decode_error, owned_input};
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
    fn new(index: usize, module_name: &'static str) -> Self {
        let decode = Charmap::named(module_name).unwrap();
        let encode = (0..=u8::MAX)
            .filter_map(|byte| decode.decode(byte).map(|ch| (ch, byte)))
            .collect();
        Self {
            index,
            decode,
            encode,
        }
    }

    /// Append mapped code points up to the next byte requiring an error handler.
    pub(crate) fn decode_prefix(
        &self,
        points: &mut Vec<u32>,
        data: &[u8],
        offset: usize,
        surrogateescape: bool,
    ) -> Result<Option<DecodeIssue>> {
        points
            .try_reserve(data.len() - offset)
            .map_err(|_| Error::allocation())?;
        for (index, &byte) in data.iter().enumerate().skip(offset) {
            points.push(match self.decode.decode(byte) {
                Some(ch) => u32::from(ch),
                None if surrogateescape && !byte.is_ascii() => 0xdc00 + u32::from(byte),
                None => return Ok(Some(charmap_issue(index))),
            });
        }
        Ok(None)
    }
}

static CP1251: LazyLock<CodePage> = LazyLock::new(|| CodePage::new(4, "cp1251"));
static CP424: LazyLock<CodePage> = LazyLock::new(|| CodePage::new(5, "cp424"));
static CP437: LazyLock<CodePage> = LazyLock::new(|| CodePage::new(6, "cp437"));
static KOI8_U: LazyLock<CodePage> = LazyLock::new(|| CodePage::new(8, "koi8_u"));
static CP1252: LazyLock<CodePage> = LazyLock::new(|| CodePage::new(9, "cp1252"));
static ISO8859_15: LazyLock<CodePage> = LazyLock::new(|| CodePage::new(10, "iso8859_15"));
static ISO8859_2: LazyLock<CodePage> = LazyLock::new(|| CodePage::new(12, "iso8859_2"));
static KOI8_R: LazyLock<CodePage> = LazyLock::new(|| CodePage::new(18, "koi8_r"));

const CODEC_NAMES: [&str; 20] = [
    "ascii",
    "iso8859-1",
    "utf-8",
    "utf-8-sig",
    "cp1251",
    "cp424",
    "cp437",
    "shift_jis",
    "koi8-u",
    "cp1252",
    "iso8859-15",
    "idna",
    "iso8859-2",
    "utf-16-le",
    "utf-16-be",
    "utf-32-le",
    "utf-32-be",
    "big5",
    "koi8-r",
    "euc_jp",
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
pub(crate) fn normalize(name: &str) -> Result<String> {
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

/// The encodings provider also tries dots as underscores in aliases, but not in module names.
fn matches_alias(key: &str, aliases: &[&str]) -> bool {
    aliases.contains(&key)
        || (key.contains('.') && aliases.contains(&key.replace('.', "_").as_str()))
}

/// An owned entry in the native codec registry for checked callers.
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
    let result = StrEncoder::lookup(encoding, errors).and_then(|encoder| {
        // SAFETY: the caller roots the original string through conversion and error construction.
        unsafe { encoder.encode(&*text, errors, || Ok(StrOwner::acquire_word(text as usize))) }
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
    let (codec, data, errors) = unsafe { (codec(info), &*bytes, &*errors) };
    let text = match codec {
        Codec::Latin1 => latin1_string(data),
        _ => codec.decode(data, Some(errors)).map(Str::from),
    };
    finish(text.and_then(crate::checked_owned_str), std::ptr::null())
}

/// Decode a statically selected Latin-1 buffer.
///
/// # Safety
/// The caller retains the immutable byte buffer through decoding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bytes_decode_latin1(value: *const Bytes) -> *const Str {
    // SAFETY: the caller roots the immutable buffer for this call.
    let text = latin1_string(unsafe { &*value });
    finish(text.and_then(crate::checked_owned_str), std::ptr::null())
}

/// Size the native string once and fill it with complete UTF-8 code points.
fn latin1_string(bytes: &[u8]) -> Result<Str> {
    let text = if bytes.is_ascii() {
        // SAFETY: each ASCII byte is a complete UTF-8 code point.
        Str::try_new(unsafe { std::str::from_utf8_unchecked(bytes) })
    } else {
        // Each non-ASCII Latin-1 byte expands to exactly two UTF-8 bytes.
        let extra = bytes.iter().filter(|byte| !byte.is_ascii()).count();
        let length = bytes.len() + extra;
        Str::try_with_capacity(length).map(|mut text| {
            // SAFETY: an ASCII byte takes one slot and every other byte takes two.
            // The capacity covers exactly these writes. They form complete UTF-8
            // sequences, and the new length is published only after initialization.
            unsafe {
                let mut output = text.spare_capacity_mut().as_mut_ptr().cast::<u8>();
                for &byte in bytes {
                    if byte.is_ascii() {
                        output.write(byte);
                        output = output.add(1);
                    } else {
                        output.write(0xc0 | (byte >> 6));
                        output.add(1).write(0x80 | (byte & 0x3f));
                        output = output.add(2);
                    }
                }
                text.set_len(length);
            }
            text
        })
    };
    text.map_err(|_| Error::allocation())
}

/// Explicit byte order never inserts a BOM. U+FEFF in the input encodes normally.
#[derive(Clone, Copy)]
pub(crate) enum Utf16Order {
    Little,
    Big,
}

impl Utf16Order {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Little => "utf-16-le",
            Self::Big => "utf-16-be",
        }
    }

    pub(crate) fn from_sign(order: i32) -> Self {
        if order < 0 || order == 0 && cfg!(target_endian = "little") {
            Self::Little
        } else {
            Self::Big
        }
    }

    pub(crate) fn unit(self, bytes: [u8; 2]) -> u16 {
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    /// Decode complete units, leaving the trailing partial unit or surrogate pair to the caller.
    pub(crate) fn decode_prefix(
        self,
        points: &mut Vec<u32>,
        bytes: &[u8],
        mut offset: usize,
        final_chunk: bool,
    ) -> Result<(usize, Option<DecodeIssue>)> {
        points
            .try_reserve((bytes.len() - offset).div_ceil(2))
            .map_err(|_| Error::allocation())?;
        while offset < bytes.len() {
            let rest = &bytes[offset..];
            let (reason, end) = if let Some((first, rest)) = rest.split_first_chunk::<2>() {
                let first = self.unit(*first);
                match first {
                    0xd800..=0xdbff => {
                        if let Some(second) = rest.first_chunk::<2>() {
                            let second = self.unit(*second);
                            if (0xdc00..=0xdfff).contains(&second) {
                                let high = u32::from(first - 0xd800);
                                let low = u32::from(second - 0xdc00);
                                points.push(0x10000 + (high << 10) + low);
                                offset += 4;
                                continue;
                            }
                            ("illegal UTF-16 surrogate", offset + 2)
                        } else if final_chunk {
                            ("unexpected end of data", bytes.len())
                        } else {
                            break;
                        }
                    }
                    0xdc00..=0xdfff => ("illegal encoding", offset + 2),
                    _ => {
                        points.push(u32::from(first));
                        offset += 2;
                        continue;
                    }
                }
            } else if final_chunk {
                ("truncated data", bytes.len())
            } else {
                break;
            };
            return Ok((
                offset,
                Some(DecodeIssue {
                    encoding: self.name(),
                    start: offset,
                    end,
                    reason,
                }),
            ));
        }
        Ok((offset, None))
    }

    pub(crate) fn append(
        self,
        bytes: &mut Vec<u8>,
        units: impl IntoIterator<Item = u16>,
    ) -> Result<()> {
        for unit in units {
            bytes.try_reserve(2).map_err(|_| Error::allocation())?;
            bytes.extend_from_slice(&match self {
                Self::Little => unit.to_le_bytes(),
                Self::Big => unit.to_be_bytes(),
            });
        }
        Ok(())
    }
}

/// Explicit byte order never inserts a BOM. U+FEFF in the input encodes normally.
#[derive(Clone, Copy)]
pub(crate) enum FixedUnicode {
    Utf16(Utf16Order),
    Utf32Le,
    Utf32Be,
}

impl FixedUnicode {
    pub(crate) fn by_key(key: &str) -> Option<Self> {
        match key {
            "utf_16_le" => Some(Self::Utf16(Utf16Order::Little)),
            "utf_16_be" => Some(Self::Utf16(Utf16Order::Big)),
            "utf_32_le" => Some(Self::Utf32Le),
            "utf_32_be" => Some(Self::Utf32Be),
            k if matches_alias(k, &["utf_16le", "unicodelittleunmarked"]) => {
                Some(Self::Utf16(Utf16Order::Little))
            }
            k if matches_alias(k, &["utf_16be", "unicodebigunmarked"]) => {
                Some(Self::Utf16(Utf16Order::Big))
            }
            k if matches_alias(k, &["utf_32le"]) => Some(Self::Utf32Le),
            k if matches_alias(k, &["utf_32be"]) => Some(Self::Utf32Be),
            _ => None,
        }
    }

    /// The unsuffixed stateless codecs use native order and prefix one BOM.
    pub(crate) fn encoding(key: &str) -> Option<(Self, bool)> {
        match key {
            "utf16" | "u16" | "utf_16" => Some((Self::Utf16(Utf16Order::from_sign(0)), true)),
            "utf32" | "u32" | "utf_32" => Some((
                if cfg!(target_endian = "little") {
                    Self::Utf32Le
                } else {
                    Self::Utf32Be
                },
                true,
            )),
            _ => Self::by_key(key).map(|format| (format, false)),
        }
    }

    pub(crate) fn width(self) -> usize {
        match self {
            Self::Utf16(_) => 2,
            Self::Utf32Le | Self::Utf32Be => 4,
        }
    }

    pub(crate) fn byte_order(self) -> i32 {
        match self {
            Self::Utf16(Utf16Order::Little) | Self::Utf32Le => -1,
            Self::Utf16(Utf16Order::Big) | Self::Utf32Be => 1,
        }
    }

    pub(crate) fn with_order(self, order: i32) -> Self {
        let order = Utf16Order::from_sign(order);
        match self {
            Self::Utf16(_) => Self::Utf16(order),
            _ if matches!(order, Utf16Order::Little) => Self::Utf32Le,
            _ => Self::Utf32Be,
        }
    }

    pub(crate) fn codec_name(self, signature: bool) -> &'static str {
        if !signature {
            return self.name();
        }
        match self {
            Self::Utf16(_) => "utf-16",
            Self::Utf32Le | Self::Utf32Be => "utf-32",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Utf16(order) => order.name(),
            Self::Utf32Le => "utf-32-le",
            Self::Utf32Be => "utf-32-be",
        }
    }

    pub(crate) fn append(
        self,
        bytes: &mut Vec<u8>,
        points: impl IntoIterator<Item = u32>,
    ) -> Result<()> {
        for point in points {
            match self {
                Self::Utf16(order) => {
                    // A lone surrogate requested by surrogatepass is already one code unit.
                    let mut units = [point as u16, 0];
                    let length =
                        char::from_u32(point).map_or(1, |ch| ch.encode_utf16(&mut units).len());
                    order.append(bytes, units[..length].iter().copied())?;
                }
                Self::Utf32Le | Self::Utf32Be => {
                    let word = if matches!(self, Self::Utf32Le) {
                        point.to_le_bytes()
                    } else {
                        point.to_be_bytes()
                    };
                    bytes.try_reserve(4).map_err(|_| Error::allocation())?;
                    bytes.extend_from_slice(&word);
                }
            }
        }
        Ok(())
    }
}

/// str.encode also supports stateless encoders without a full codec registry entry.
#[derive(Clone, Copy)]
pub(crate) enum StrEncoder {
    Registered(Codec),
    Wide {
        format: FixedUnicode,
        signature: bool,
    },
}

impl StrEncoder {
    pub(crate) fn lookup(encoding: &str, errors: &str) -> Result<Self> {
        check_encode_names(encoding, errors)?;
        Self::for_stream(encoding)
    }

    pub(crate) fn for_stream(encoding: &str) -> Result<Self> {
        let key = normalize(encoding)?;
        if let Some((format, signature)) = FixedUnicode::encoding(&key) {
            Ok(Self::Wide { format, signature })
        } else {
            Ok(Self::Registered(Codec::by_key(&key, encoding)?))
        }
    }

    pub(super) fn signature_mut(&mut self) -> Option<&mut bool> {
        match self {
            Self::Registered(Codec::Utf8 { signature }) | Self::Wide { signature, .. } => {
                Some(signature)
            }
            _ => None,
        }
    }

    pub(super) fn encode<'a>(
        self,
        text: &'a str,
        errors: &str,
        original: impl FnOnce() -> Result<StrOwner>,
    ) -> Result<Cow<'a, [u8]>> {
        match self {
            Self::Registered(codec) => codec.encode(text, Some(errors), original),
            Self::Wide { format, signature } => {
                let mut bytes = Vec::new();
                format.append(
                    &mut bytes,
                    signature
                        .then_some(0xfeff)
                        .into_iter()
                        .chain(text.chars().map(u32::from)),
                )?;
                Ok(Cow::Owned(bytes))
            }
        }
    }

    /// Boxed strings retain the original scalar input only when error construction needs it.
    pub(crate) fn encode_str(self, text: &str, errors: &str) -> Result<Vec<u8>> {
        match self.encode(text, errors, || super::text::try_owned_string(text))? {
            Cow::Owned(bytes) => Ok(bytes),
            Cow::Borrowed(bytes) => {
                let mut output = super::bytes::allocate(bytes.len())?;
                output.extend_from_slice(bytes);
                Ok(output)
            }
        }
    }
}

fn check_encode_names(encoding: &str, errors: &str) -> Result<()> {
    if encoding.contains('\0') || errors.contains('\0') {
        return Err(Error::new(Kind::ValueError, "embedded null character"));
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) enum Codec {
    Utf8 { signature: bool },
    Latin1,
    Ascii,
    CodePage(&'static CodePage),
    Wide(FixedUnicode),
    DoubleByte(DoubleByte),
    Idna,
}

impl Codec {
    fn index(self) -> usize {
        match self {
            Self::Ascii => 0,
            Self::Latin1 => 1,
            Self::Utf8 { signature: false } => 2,
            Self::Utf8 { signature: true } => 3,
            Self::CodePage(page) => page.index,
            Self::DoubleByte(DoubleByte::ShiftJis) => 7,
            Self::DoubleByte(DoubleByte::Big5) => 17,
            Self::DoubleByte(DoubleByte::EucJp) => 19,
            Self::Idna => 11,
            Self::Wide(FixedUnicode::Utf16(Utf16Order::Little)) => 13,
            Self::Wide(FixedUnicode::Utf16(Utf16Order::Big)) => 14,
            Self::Wide(FixedUnicode::Utf32Le) => 15,
            Self::Wide(FixedUnicode::Utf32Be) => 16,
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

    pub(super) fn for_encode(encoding: &str, errors: &str) -> Result<Self> {
        check_encode_names(encoding, errors)?;
        Self::lookup(encoding)
    }

    pub(crate) fn encode_str(text: &str, encoding: &str, errors: &str) -> Result<Vec<u8>> {
        StrEncoder::Registered(Self::for_encode(encoding, errors)?).encode_str(text, errors)
    }

    pub(crate) fn by_key(key: &str, value: &str) -> Result<Self> {
        if let Some(format) = FixedUnicode::by_key(key) {
            return Ok(Self::Wide(format));
        }
        match key {
            "utf8" | "utf_8" | "u8" | "utf" | "utf8_ucs2" | "utf8_ucs4" | "cp65001" => {
                Ok(Self::Utf8 { signature: false })
            }
            "utf_8_sig" => Ok(Self::Utf8 { signature: true }),
            "shift_jis" | "shiftjis" | "s_jis" | "sjis" | "csshiftjis" | "x_mac_japanese" => {
                Ok(Self::DoubleByte(DoubleByte::ShiftJis))
            }
            "euc_jp" | "eucjp" | "ujis" | "u_jis" => Ok(Self::DoubleByte(DoubleByte::EucJp)),
            "big5" | "big5_tw" | "csbig5" | "x_mac_trad_chinese" => {
                Ok(Self::DoubleByte(DoubleByte::Big5))
            }
            "idna" => Ok(Self::Idna),
            "cp1251" | "1251" | "windows_1251" => Ok(Self::CodePage(&CP1251)),
            "cp1252" | "1252" | "windows_1252" => Ok(Self::CodePage(&CP1252)),
            "cp424" | "424" | "csibm424" | "ebcdic_cp_he" | "ibm424" => Ok(Self::CodePage(&CP424)),
            "cp437" | "437" | "cspc8codepage437" | "ibm437" => Ok(Self::CodePage(&CP437)),
            "iso8859_15" | "iso_8859_15" | "l9" | "latin9" => Ok(Self::CodePage(&ISO8859_15)),
            "koi8_r" | "cskoi8r" => Ok(Self::CodePage(&KOI8_R)),
            "koi8_u" => Ok(Self::CodePage(&KOI8_U)),
            "latin1" | "latin" | "iso8859" | "latin_1" | "l1" | "iso8859_1" | "iso_8859_1"
            | "iso_8859_1_1987" | "8859" | "cp819" | "ibm819" | "iso_ir_100" | "csisolatin1" => {
                Ok(Self::Latin1)
            }
            "ascii" | "us" | "us_ascii" | "646" | "ansi_x3.4_1968" | "ansi_x3_4_1968"
            | "ansi_x3.4_1986" | "cp367" | "csascii" | "ibm367" | "iso646_us"
            | "iso_646.irv_1991" | "iso_ir_6" => Ok(Self::Ascii),
            key if key == "iso8859_2"
                || matches_alias(
                    key,
                    &[
                        "csisolatin2",
                        "iso_8859_2",
                        "iso_8859_2_1987",
                        "iso_ir_101",
                        "l2",
                        "latin2",
                    ],
                ) =>
            {
                Ok(Self::CodePage(&ISO8859_2))
            }
            _ => Err(option_error(
                Kind::LookupError,
                "unknown encoding: ",
                value,
                "",
            )),
        }
    }

    /// Text streams accept the locale sentinel in addition to registry names.
    pub(crate) fn for_stream(encoding: Option<&str>) -> Result<Self> {
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

    /// Decode a POSIX filename with the startup codec and Python's surrogateescape policy.
    pub(crate) fn decode_filesystem(self, data: &[u8]) -> Result<Vec<u32>> {
        if let Self::Wide(format) = self {
            return format.decode_filesystem(data);
        }
        if matches!(self, Self::Idna) {
            return self
                .decode(data, Some("surrogateescape"))
                .map(|s| s.chars().map(u32::from).collect());
        }
        let mut points = Vec::new();
        points
            .try_reserve_exact(data.len())
            .map_err(|_| Error::allocation())?;
        if matches!(self, Self::Utf8 { .. }) {
            points.extend(super::interpreter_stdlib::fsdecode(data));
            return Ok(points);
        }
        if let Self::DoubleByte(format) = self {
            format.decode_filesystem(data, &mut points);
            return Ok(points);
        }
        if let Self::CodePage(page) = self {
            if let Some(issue) = page.decode_prefix(&mut points, data, 0, true)? {
                return Err(issue.error(data));
            }
        } else {
            // The remaining codecs are ASCII and Latin-1.
            points.extend(data.iter().map(|&byte| {
                if matches!(self, Self::Ascii) && !byte.is_ascii() {
                    0xdc00 + u32::from(byte)
                } else {
                    u32::from(byte)
                }
            }));
        }
        Ok(points)
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
                        decoding_error(&mut text, errors, &[byte], || {
                            charmap_issue(index).error(data)
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
                        decoding_error(&mut text, errors, &[byte], || {
                            ascii_decode_error(data, index)
                        })?;
                    }
                }
                Ok(text)
            }
            Self::Wide(format) => {
                format.decode_text(&mut text, data, errors, true)?;
                Ok(text)
            }
            Self::Idna => idna::decode(data, errors),
            Self::DoubleByte(format) => {
                format.decode_chunk(&mut text, data, errors, true)?;
                Ok(text)
            }
            Self::Utf8 { signature } => {
                let data = if signature {
                    data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data)
                } else {
                    data
                };
                let mut offset = 0;
                while let Some(issue) = utf8_prefix(&mut text, data, offset)? {
                    decoding_error(&mut text, errors, &data[issue.start..issue.end], || {
                        issue.error(data)
                    })?;
                    offset = issue.end;
                }
                Ok(text)
            }
        }
    }

    /// A representable scalar in these single-byte and multibyte mappings uses up to three bytes.
    pub(crate) fn encode_unit(self, ch: char) -> Option<([u8; 3], usize)> {
        if let Self::DoubleByte(format) = self {
            return format.encode_char(ch);
        }
        self.encode_byte(ch).map(|byte| ([byte, 0, 0], 1))
    }

    pub(crate) fn encode_byte(self, ch: char) -> Option<u8> {
        match self {
            Self::Ascii => ch.is_ascii().then_some(ch as u8),
            Self::Latin1 => u8::try_from(u32::from(ch)).ok(),
            Self::CodePage(page) => page.encode.get(&ch).copied(),
            Self::Utf8 { .. } | Self::DoubleByte(_) | Self::Idna | Self::Wide(_) => {
                unreachable!("expected a single-byte encoding")
            }
        }
    }

    /// Text substituted for one unencodable Python code point by a built-in error handler.
    pub(crate) fn error_replacement(name: &str, code: u32) -> Str {
        match name {
            "replace" => Str::const_new("?"),
            "backslashreplace" => escaped(code),
            "xmlcharrefreplace" => compact_str::format_compact!("&#{code};"),
            "namereplace" => char::from_u32(code)
                .and_then(unicode_names2::name)
                .map_or_else(
                    || escaped(code),
                    |name| compact_str::format_compact!("\\N{{{name}}}"),
                ),
            _ => unreachable!("text replacement policy"),
        }
    }

    pub(crate) fn encoding_error_reason(self) -> (&'static str, &'static str) {
        match self {
            Self::Wide(format) => (format.name(), "surrogates not allowed"),
            Self::Ascii => ("ascii", "ordinal not in range(128)"),
            Self::Latin1 => ("latin-1", "ordinal not in range(256)"),
            Self::CodePage(_) => ("charmap", "character maps to <undefined>"),
            Self::DoubleByte(_) => (self.name(), "illegal multibyte sequence"),
            Self::Idna => unreachable!("IDNA encodes whole labels"),
            Self::Utf8 { .. } => unreachable!("UTF-8 encodes every Unicode scalar"),
        }
    }

    pub(crate) fn encode<'a>(
        self,
        text: &'a str,
        errors: Option<&str>,
        error_input: impl FnOnce() -> Result<StrOwner>,
    ) -> Result<Cow<'a, [u8]>> {
        let (codec, reason) = match self {
            Self::Wide(format) => {
                return StrEncoder::Wide {
                    format,
                    signature: false,
                }
                .encode(text, errors.unwrap_or("strict"), error_input);
            }
            Self::Idna => return idna::encode(text, errors, error_input).map(Cow::Owned),
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
            _ => self.encoding_error_reason(),
        };
        let mut out = Vec::new();
        out.try_reserve_exact(text.len())
            .map_err(|_| Error::allocation())?;
        let mut chars = text.chars().enumerate().peekable();
        while let Some((start, ch)) = chars.next() {
            if let Some((unit, size)) = self.encode_unit(ch) {
                out.try_reserve(size).map_err(|_| Error::allocation())?;
                out.extend_from_slice(&unit[..size]);
                continue;
            }
            let replacement = match errors.unwrap_or("strict") {
                "ignore" => continue,
                name @ ("replace" | "backslashreplace" | "xmlcharrefreplace" | "namereplace") => {
                    Self::error_replacement(name, u32::from(ch))
                }
                "strict" | "surrogatepass" | "surrogateescape" => {
                    let mut end = start;
                    if !matches!(self, Self::DoubleByte(_)) {
                        while let Some((i, _)) =
                            chars.next_if(|(_, c)| self.encode_byte(*c).is_none())
                        {
                            end = i;
                        }
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
                let Some((unit, size)) = self.encode_unit(c) else {
                    return Err(encoding_error(
                        codec,
                        reason,
                        error_input()?,
                        start,
                        start,
                        ch,
                    ));
                };
                out.extend_from_slice(&unit[..size]);
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
        format!("character '{}' in position {start}", escaped(u32::from(ch)))
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

pub(super) use imre_ast::source_encoding::universal_newlines;

fn escaped(code: u32) -> Str {
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

/// Append valid text until the next malformed sequence or the end of the bytes.
pub(crate) fn utf8_prefix(
    text: &mut String,
    data: &[u8],
    offset: usize,
) -> Result<Option<DecodeIssue>> {
    let (valid, issue) = match std::str::from_utf8(&data[offset..]) {
        Ok(valid) => (valid, None),
        Err(error) => {
            let issue = DecodeIssue::utf8(data, offset, error);
            // The validator has already checked this prefix.
            (
                std::str::from_utf8(&data[offset..issue.start]).unwrap(),
                Some(issue),
            )
        }
    };
    text.try_reserve(valid.len())
        .map_err(|_| Error::allocation())?;
    text.push_str(valid);
    Ok(issue)
}

pub(crate) fn unknown_handler(name: &str) -> Error {
    // CPython includes at most 400 UTF-8 bytes, ending before any partial character.
    let name = &name[..name.floor_char_boundary(400)];
    option_error(Kind::LookupError, "unknown error handler name '", name, "'")
}

fn charmap_issue(index: usize) -> DecodeIssue {
    DecodeIssue {
        encoding: "charmap",
        start: index,
        end: index + 1,
        reason: "character maps to <undefined>",
    }
}

pub(crate) fn decoding_error(
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
