//! Fixed-order decoding shared by checked text and Python codec factories.
use super::{DecodeIssue, Error, FixedUnicode, Result, decoding_error};

impl FixedUnicode {
    pub(crate) fn surrogate(self, data: &[u8]) -> Option<(u32, usize)> {
        let (point, width) = match self {
            Self::Utf16(order) => (u32::from(order.unit(*data.first_chunk()?)), 2),
            Self::Utf32Le | Self::Utf32Be => (self.utf32_unit(data)?, 4),
        };
        (0xd800..=0xdfff).contains(&point).then_some((point, width))
    }

    fn utf32_unit(self, data: &[u8]) -> Option<u32> {
        let unit = *data.first_chunk()?;
        Some(if matches!(self, Self::Utf32Le) {
            u32::from_le_bytes(unit)
        } else {
            u32::from_be_bytes(unit)
        })
    }

    pub(crate) fn decode_prefix(
        self,
        points: &mut Vec<u32>,
        data: &[u8],
        mut start: usize,
        final_chunk: bool,
    ) -> Result<(usize, Option<DecodeIssue>)> {
        if let Self::Utf16(order) = self {
            return order.decode_prefix(points, data, start, final_chunk);
        }
        points
            .try_reserve((data.len() - start) / 4)
            .map_err(|_| Error::allocation())?;
        while start < data.len() {
            let (reason, end) = match self.utf32_unit(&data[start..]) {
                Some(0xd800..=0xdfff) => (
                    "code point in surrogate code point range(0xd800, 0xe000)",
                    start + 4,
                ),
                Some(0x110000..) => ("code point not in range(0x110000)", start + 4),
                None if !final_chunk => break,
                None => ("truncated data", data.len()),
                Some(scalar) => {
                    points.push(scalar);
                    start += 4;
                    continue;
                }
            };
            return Ok((
                start,
                Some(DecodeIssue {
                    encoding: self.name(),
                    start,
                    end,
                    reason,
                }),
            ));
        }
        Ok((start, None))
    }

    /// Append scalar text for a checked caller, keeping an incomplete unit for the next read.
    pub(crate) fn decode_text(
        self,
        text: &mut String,
        data: &[u8],
        errors: Option<&str>,
        final_chunk: bool,
    ) -> Result<usize> {
        let mut points = Vec::new();
        if let Some(policy @ ("surrogateescape" | "surrogatepass")) = errors {
            let consumed = self.decode_surrogates(&mut points, data, policy, final_chunk)?;
            // A later error still wins when the successfully decoded prefix contains a surrogate.
            if points.iter().any(|p| (0xd800..=0xdfff).contains(p)) {
                let message = if policy == "surrogatepass" {
                    "native strings cannot represent surrogate code points"
                } else {
                    "native strings cannot represent surrogate-escaped input bytes"
                };
                return Err(Error::new(super::Kind::NotImplementedError, message));
            }
            append_scalars(text, &points)?;
            return Ok(consumed);
        }
        let mut start = 0;
        loop {
            let (end, issue) = self.decode_prefix(&mut points, data, start, final_chunk)?;
            append_scalars(text, &points)?;
            points.clear();
            let Some(issue) = issue else {
                return Ok(end);
            };
            decoding_error(text, errors, &data[issue.start..issue.end], || {
                issue.error(data)
            })?;
            start = issue.end;
        }
    }

    pub(super) fn decode_filesystem(self, data: &[u8]) -> Result<Vec<u32>> {
        let mut points = Vec::new();
        self.decode_surrogates(&mut points, data, "surrogateescape", true)?;
        Ok(points)
    }

    fn decode_surrogates(
        self,
        points: &mut Vec<u32>,
        data: &[u8],
        policy: &str,
        final_chunk: bool,
    ) -> Result<usize> {
        let mut start = 0;
        loop {
            let (end, issue) = self.decode_prefix(points, data, start, final_chunk)?;
            let Some(issue) = issue else {
                return Ok(end);
            };
            if policy == "surrogatepass" {
                let Some((point, width)) = self.surrogate(&data[issue.start..]) else {
                    return Err(issue.error(data));
                };
                points.try_reserve(1).map_err(|_| Error::allocation())?;
                points.push(point);
                start = issue.start + width;
            } else {
                let escaped = &data[issue.start..issue.end];
                let length = escaped.iter().take_while(|&&b| b >= 0x80).count();
                if length == 0 {
                    return Err(issue.error(data));
                }
                points
                    .try_reserve(length)
                    .map_err(|_| Error::allocation())?;
                points.extend(escaped[..length].iter().map(|&b| 0xdc00 + u32::from(b)));
                start = issue.start + length;
            }
        }
    }
}

fn append_scalars(text: &mut String, points: &[u32]) -> Result<()> {
    for &point in points {
        let ch = char::from_u32(point).expect("decoded Unicode scalar");
        text.try_reserve(ch.len_utf8())
            .map_err(|_| Error::allocation())?;
        text.push(ch);
    }
    Ok(())
}
