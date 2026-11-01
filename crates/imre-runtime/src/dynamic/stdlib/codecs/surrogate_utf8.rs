//! UTF-8 encoding for Python strings containing non-scalar code points.
use super::{Context, Error, Id, Result, Value};

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
                    let end = index + count;
                    let part = if count == 1 {
                        format!("character '\\u{point:04x}' in position {index}")
                    } else {
                        format!("characters in position {index}-{}", end - 1)
                    };
                    let mut error = Error::new(
                        "UnicodeEncodeError",
                        format!("'utf-8' codec can't encode {part}: surrogates not allowed"),
                    );
                    let encoding = self.text("utf-8");
                    let reason = self.text("surrogates not allowed");
                    let start = self.alloc(Value::Int(crate::Int::from_usize(index)));
                    let end = self.alloc(Value::Int(crate::Int::from_usize(end)));
                    error.args =
                        Some(self.alloc(Value::Tuple(vec![encoding, id, start, end, reason])));
                    return Err(error);
                }
                _ => return Err(crate::objects::unknown_handler(errors).into()),
            }
        }
        Ok(bytes)
    }
}
