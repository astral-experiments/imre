//! Native encoding for Python strings containing non-scalar code points.
use super::{Context, Error, Id, Result, Value};
use crate::objects::Codec;

impl Context {
    pub(in crate::dynamic) fn encode_filesystem_string(&mut self, id: Id) -> Result<Vec<u8>> {
        if let Ok(text) = self.str_value(id) {
            return Ok(crate::objects::Codec::encode_str(
                text,
                "utf-8",
                "surrogateescape",
            )?);
        }
        self.encode_utf8_codepoints(id, false, "surrogateescape")
    }

    /// Encode an OS argument using the codec selected at interpreter startup.
    pub(in crate::dynamic) fn encode_os_path(&mut self, id: Id) -> Result<Vec<u8>> {
        let codec = Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
        match codec {
            Codec::Utf8 { signature: false } => self.encode_filesystem_string(id),
            Codec::Utf8 { signature: true } => {
                self.encode_utf8_codepoints(id, true, "surrogateescape")
            }
            Codec::Idna | Codec::Wide(_) => {
                let encoding = self.text(codec.name());
                let errors = self.text("surrogateescape");
                super::encode_text(self, id, Some(encoding), Some(errors))
            }
            _ => self.encode_legacy_codepoints(id, codec, true),
        }
    }

    /// Encode code points with a legacy codec under strict or surrogateescape handling.
    pub(in crate::dynamic::stdlib) fn encode_legacy_codepoints(
        &mut self,
        id: Id,
        codec: Codec,
        surrogateescape: bool,
    ) -> Result<Vec<u8>> {
        let points = self.string_codepoints(id)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(points.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        let encode = |point| char::from_u32(point).and_then(|ch| codec.encode_unit(ch));
        for (start, &point) in points.iter().enumerate() {
            if let Some((unit, len)) = encode(point) {
                bytes
                    .try_reserve(len)
                    .map_err(|_| crate::Error::allocation())?;
                bytes.extend_from_slice(&unit[..len]);
            } else if surrogateescape && (0xdc80..=0xdcff).contains(&point) {
                bytes.push(point as u8);
            } else {
                let end = if matches!(codec, Codec::DoubleByte(_)) {
                    start + 1
                } else {
                    start
                        + points[start..]
                            .iter()
                            .take_while(|&&p| encode(p).is_none())
                            .count()
                };
                // A charmap passes the whole unmapped run to the error handler.
                // ASCII and Latin-1 first consume its leading escaped bytes.
                let start = if surrogateescape && matches!(codec, Codec::CodePage(_)) {
                    start
                        - points[..start]
                            .iter()
                            .rev()
                            .take_while(|&&p| (0xdc80..=0xdcff).contains(&p))
                            .count()
                } else {
                    start
                };
                let (name, reason) = codec.encoding_error_reason();
                let first = points[start];
                return Err(self.string_encoding_error(id, name, reason, first, start..end));
            }
        }
        Ok(bytes)
    }

    pub(in crate::dynamic) fn string_encoding_error(
        &mut self,
        id: Id,
        codec: &str,
        reason: &str,
        point: impl Into<Option<u32>>,
        range: std::ops::Range<usize>,
    ) -> Error {
        let start = range.start;
        let part = if let Some(point) = point.into().filter(|_| range.len() == 1) {
            let escape = match point {
                0..=0xff => format!("\\x{point:02x}"),
                0x100..=0xffff => format!("\\u{point:04x}"),
                _ => format!("\\U{point:08x}"),
            };
            format!("character '{escape}' in position {start}")
        } else {
            format!("characters in position {start}-{}", range.end - 1)
        };
        let mut error = Error::new(
            "UnicodeEncodeError",
            format!("'{codec}' codec can't encode {part}: {reason}"),
        );
        let encoding = self.text(codec);
        let reason = self.text(reason);
        let start = self.alloc(Value::Int(crate::Int::from_usize(start)));
        let end = self.alloc(Value::Int(crate::Int::from_usize(range.end)));
        error.args = Some(self.alloc(Value::Tuple(vec![encoding, id, start, end, reason])));
        error
    }

    pub(in crate::dynamic) fn encode_utf8_codepoints(
        &mut self,
        id: Id,
        signature: bool,
        errors: &str,
    ) -> Result<Vec<u8>> {
        let points = self.string_codepoints(id)?;
        let mut bytes = Vec::new();
        if signature {
            bytes.extend_from_slice(b"\xef\xbb\xbf");
        }
        for (index, &point) in points.iter().enumerate() {
            if let Some(ch) = char::from_u32(point) {
                bytes.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                continue;
            }
            match errors {
                "surrogateescape" if (0xdc80..=0xdcff).contains(&point) => bytes.push(point as u8),
                "surrogatepass" => {
                    bytes.extend_from_slice(&[
                        (0xe0 | (point >> 12)) as u8,
                        (0x80 | ((point >> 6) & 0x3f)) as u8,
                        (0x80 | (point & 0x3f)) as u8,
                    ]);
                }
                "ignore" => (),
                "replace" => bytes.push(b'?'),
                "backslashreplace" | "namereplace" => {
                    bytes.extend_from_slice(format!("\\u{point:04x}").as_bytes())
                }
                "xmlcharrefreplace" => bytes.extend_from_slice(format!("&#{point};").as_bytes()),
                "strict" | "surrogateescape" => {
                    let count = points[index..]
                        .iter()
                        .take_while(|&&p| (0xd800..=0xdfff).contains(&p))
                        .count();
                    return Err(self.string_encoding_error(
                        id,
                        "utf-8",
                        "surrogates not allowed",
                        point,
                        index..index + count,
                    ));
                }
                _ => return Err(crate::objects::unknown_handler(errors).into()),
            }
        }
        Ok(bytes)
    }
}
