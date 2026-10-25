//! Detect the encoding of binary JSON before handing text to the standard reader.

use crate::StrOwner;
use std::borrow::Cow;

use crate::objects::{
    Object,
    bytes::{decode_error, decode_owned_error},
};
use crate::{Bytes, Error, Kind, finish};

#[derive(Clone, Copy)]
enum Encoding {
    Utf8,
    Utf16(bool),
    Utf32(bool),
}

impl Encoding {
    /// A BOM takes precedence over the pattern of NUL bytes in the first unit.
    fn detect(bytes: &[u8]) -> (Self, usize) {
        match bytes {
            [0, 0, 0xfe, 0xff, ..] => (Self::Utf32(false), 4),
            [0xff, 0xfe, 0, 0, ..] => (Self::Utf32(true), 4),
            [0xfe, 0xff, ..] => (Self::Utf16(false), 2),
            [0xff, 0xfe, ..] => (Self::Utf16(true), 2),
            [0xef, 0xbb, 0xbf, ..] => (Self::Utf8, 3),
            [0, 0, _, _, ..] => (Self::Utf32(false), 0),
            [0, _, _, _, ..] | [0, _] => (Self::Utf16(false), 0),
            [_, 0, 0, 0, ..] => (Self::Utf32(true), 0),
            [_, 0, _, _, ..] | [_, 0] => (Self::Utf16(true), 0),
            _ => (Self::Utf8, 0),
        }
    }
}

fn decode(bytes: &crate::BytesOwner) -> Result<Cow<'_, str>, Error> {
    let (encoding, mut pos) = Encoding::detect(bytes);
    let (width, little, codec) = match encoding {
        Encoding::Utf8 => {
            let data = &bytes[pos..];
            return std::str::from_utf8(data).map(Cow::Borrowed).map_err(|err| {
                if pos == 0 {
                    decode_owned_error(crate::BytesOwner::clone(bytes), err)
                } else {
                    decode_error(data, err)
                }
            });
        }
        Encoding::Utf16(little) => (2, little, if little { "utf-16-le" } else { "utf-16-be" }),
        Encoding::Utf32(little) => (4, little, if little { "utf-32-le" } else { "utf-32-be" }),
    };
    let invalid = |start, end, reason| {
        let location = if end == start + 1 {
            format!("byte 0x{:02x} in position {start}", bytes[start])
        } else {
            format!("bytes in position {start}-{}", end - 1)
        };
        Error::unicode_decode(
            codec,
            bytes.clone(),
            start..end,
            reason,
            format!("'{codec}' codec can't decode {location}: {reason}"),
        )
    };
    let unit = |at| {
        let pair = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        })
    };
    let mut text = String::new();
    while pos < bytes.len() {
        let start = pos;
        let end = pos + width;
        let chunk = bytes
            .get(pos..end)
            .ok_or_else(|| invalid(start, bytes.len(), "truncated data"))?;
        pos = end;
        let code = if width == 4 {
            let word = chunk.try_into().expect("four bytes");
            if little {
                u32::from_le_bytes(word)
            } else {
                u32::from_be_bytes(word)
            }
        } else {
            let lead = unit(start).expect("two bytes");
            if (0xd800..=0xdbff).contains(&lead)
                && let Some(trail @ 0xdc00..=0xdfff) = unit(pos)
            {
                pos += 2;
                0x10000 + (u32::from(lead - 0xd800) << 10) + u32::from(trail - 0xdc00)
            } else {
                u32::from(lead)
            }
        };
        let c = char::from_u32(code).ok_or_else(|| {
            invalid(
                start,
                end,
                if code > 0x10ffff {
                    "code point not in range(0x110000)"
                } else {
                    "unpaired surrogate is outside the native string domain"
                },
            )
        })?;
        text.try_reserve(c.len_utf8())
            .map_err(|_| Error::new(Kind::MemoryError, "JSON text allocation failed"))?;
        text.push(c);
    }
    Ok(Cow::Owned(text))
}

/// # Safety
/// The caller roots the input bytes for the entire decode. A successful parse
/// transfers its own document owner, independent of the input buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_loads_bytes(bytes: *const Bytes) -> *const Object {
    // SAFETY: the caller supplies live immutable bytes until the parse completes.
    let bytes = unsafe { crate::acquire_bytes(bytes as usize) };
    let result = decode(&bytes).and_then(|text| {
        super::readonly::loads(&text, || {
            crate::Str::try_new(text.as_ref()).ok().map(StrOwner::new)
        })
    });
    finish(result, std::ptr::null())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::exceptions::{imre_error_take, imre_exception_repr};

    #[test]
    fn invalid_utf8_retains_the_original_bytes() {
        let bytes = crate::BytesOwner::new(vec![0xff]);
        let error = decode(&bytes).unwrap_err();
        assert_eq!(crate::BytesOwner::strong_count(&bytes), 2);
        drop(error);
        assert_eq!(crate::BytesOwner::strong_count(&bytes), 1);
    }

    #[test]
    fn wide_decode_error_retains_the_original_bytes() {
        let bytes = crate::BytesOwner::new(vec![0xff, 0xfe, b'"', 0, 3]);
        let observer = bytes.clone();
        // SAFETY: bytes roots its exact runtime handle throughout the decode.
        assert!(unsafe { imre_json_loads_bytes(crate::BytesOwner::as_ptr(&bytes)) }.is_null());
        drop(bytes);
        // SAFETY: take and repr each transfer one owner, and the exception stays rooted.
        let (error, repr) = unsafe {
            let error = crate::ObjectOwner::from_raw(imre_error_take());
            let repr =
                crate::StrOwner::from_raw(imre_exception_repr(crate::ObjectOwner::as_ptr(&error)));
            (error, repr)
        };
        assert_eq!(
            repr.as_str(),
            r#"UnicodeDecodeError('utf-16-le', b'\xff\xfe"\x00\x03', 4, 5, 'truncated data')"#
        );
        assert_eq!(crate::BytesOwner::strong_count(&observer), 2);
        drop(error);
        assert_eq!(crate::BytesOwner::strong_count(&observer), 1);
    }
}
