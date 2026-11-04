//! IDNA 2003 (RFC 3490), following CPython 3.14.5's encodings.idna.
mod nameprep;
mod punycode;

use super::super::bytes::owned_input;
use super::{Error, Kind, Result, StrOwner};
use std::ops::Range;

/// Positions belong to a label until the outer codec translates them to domain offsets.
#[derive(Debug)]
pub(crate) enum LabelError {
    Invalid {
        bounds: Range<usize>,
        reason: String,
        text: Option<Vec<u32>>,
    },
    Overflow,
}

impl LabelError {
    fn new(start: usize, end: usize, reason: impl Into<String>) -> Self {
        Self::Invalid {
            bounds: start..end,
            reason: reason.into(),
            text: None,
        }
    }

    fn overflow() -> Self {
        Self::Overflow
    }

    fn with_text(mut self, points: Vec<u32>) -> Self {
        if let Self::Invalid { text, .. } = &mut self {
            *text = Some(points);
        }
        self
    }

    fn at_offset(self, offset: usize) -> Self {
        match self {
            Self::Invalid { bounds, reason, .. } if bounds.end <= isize::MAX as usize - offset => {
                Self::new(offset + bounds.start, offset + bounds.end, reason)
            }
            _ => Self::Overflow,
        }
    }

    fn into_encode(self, text: &str, original: impl FnOnce() -> Result<StrOwner>) -> Error {
        let Self::Invalid { bounds, reason, .. } = self else {
            return overflow();
        };
        let start = bounds.start;
        let location = match text.chars().nth(start).filter(|_| bounds.len() == 1) {
            Some(ch) => format!(
                "character '{}' in position {start}",
                super::escaped(ch as u32)
            ),
            None => format!("characters in position {start}-{}", bounds.end as isize - 1),
        };
        let message = format!("'idna' codec can't encode {location}: {reason}");
        match original() {
            Ok(object) => Error::unicode_encode("idna", object, bounds, &reason, message),
            Err(error) => error,
        }
    }

    pub(crate) fn into_decode(self, codec: &str, data: &[u8]) -> Error {
        let Self::Invalid { bounds, reason, .. } = self else {
            return overflow();
        };
        let start = bounds.start;
        let location = match data.get(start).filter(|_| bounds.len() == 1) {
            Some(byte) => format!("byte 0x{byte:02x} in position {start}"),
            None => format!("bytes in position {start}-{}", bounds.end as isize - 1),
        };
        let message = format!("'{codec}' codec can't decode {location}: {reason}");
        match owned_input(data) {
            Ok(object) => Error::unicode_decode(codec, object, bounds, &reason, message),
            Err(error) => error,
        }
    }
}

fn overflow() -> Error {
    Error::new(
        Kind::OverflowError,
        "Python int too large to convert to C ssize_t",
    )
}
type LabelResult<T> = std::result::Result<T, LabelError>;
const ACE: &[u8] = b"xn--";

pub(crate) fn check_errors(errors: Option<&str>) -> Result<()> {
    match errors {
        None | Some("strict") => Ok(()),
        Some(policy) => Err(Error::new(
            Kind::UnicodeError,
            format!("Unsupported error handling: {policy}"),
        )),
    }
}

pub(crate) fn prepare(label: &[u32]) -> LabelResult<String> {
    nameprep::prepare(label)
}

fn label_length(len: usize) -> LabelResult<()> {
    match len {
        0 => Err(LabelError::new(0, 1, "label empty")),
        1..64 => Ok(()),
        _ => Err(LabelError::new(0, len, "label too long")),
    }
}

pub(crate) fn to_ascii(label: &[u32]) -> LabelResult<Vec<u8>> {
    if label.iter().all(|&point| point < 128) {
        label_length(label.len())?;
        return Ok(label.iter().map(|&point| point as u8).collect());
    }
    let prepared = prepare(label)?;
    let points = || prepared.chars().map(u32::from).collect();
    let len = prepared.chars().count();
    if prepared.is_ascii() {
        label_length(len).map_err(|err| err.with_text(points()))?;
        return Ok(prepared.into_bytes());
    }
    if prepared
        .as_bytes()
        .get(..ACE.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(ACE))
    {
        return Err(
            LabelError::new(0, ACE.len(), "Label starts with ACE prefix").with_text(points()),
        );
    }
    // Each code point contributes at least one byte to the payload.
    if len < 64 - ACE.len() {
        let mut bytes = ACE.to_vec();
        bytes.extend(punycode::encode(&prepared));
        if bytes.len() < 64 {
            return Ok(bytes);
        }
    }
    Err(LabelError::new(0, len, "label too long").with_text(points()))
}
/// Split an IDNA domain at each RFC 3490 separator, keeping empty labels.
pub(crate) fn labels(points: &[u32]) -> impl Iterator<Item = &[u32]> {
    points.split(|c| matches!(c, 0x2e | 0x3002 | 0xff0e | 0xff61))
}

fn encode_domain(points: &[u32]) -> LabelResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut start = 0;
    for label in labels(points) {
        if start > 0 {
            bytes.push(b'.');
        }
        if start == points.len() {
            break;
        }
        bytes.extend(to_ascii(label).map_err(|err| err.at_offset(start))?);
        start += label.len() + 1;
    }
    Ok(bytes)
}

/// Encode boxed strings that may contain surrogate code points.
pub(crate) fn encode_points(
    points: &[u32],
) -> std::result::Result<Vec<u8>, (Range<usize>, String)> {
    encode_domain(points).map_err(|err| match err {
        LabelError::Invalid { bounds, reason, .. } => (bounds, reason),
        LabelError::Overflow => {
            unreachable!("encoding positions are bounded by the input allocation")
        }
    })
}

pub(super) fn encode(
    text: &str,
    errors: Option<&str>,
    original: impl FnOnce() -> Result<StrOwner>,
) -> Result<Vec<u8>> {
    check_errors(errors)?;
    if text.is_ascii() {
        // CPython's ASCII fast path rejects interior empty labels before checking any lengths.
        let mut offset = 0;
        for label in text.split_terminator('.') {
            if label.is_empty() {
                return Err(
                    LabelError::new(offset, offset + 1, "label empty").into_encode(text, original)
                );
            }
            offset += label.len() + 1;
        }
        offset = 0;
        for label in text.split('.') {
            if label.len() >= 64 {
                return Err(
                    LabelError::new(offset, offset + label.len(), "label too long")
                        .into_encode(text, original),
                );
            }
            offset += label.len() + 1;
        }
        return Ok(text.as_bytes().to_vec());
    }
    encode_domain(&text.chars().map(u32::from).collect::<Vec<_>>())
        .map_err(|err| err.into_encode(text, original))
}

pub(crate) fn to_unicode(label: &[u8]) -> LabelResult<String> {
    if label.len() > 1024 {
        return Err(LabelError::new(0, label.len(), "label way too long"));
    }
    if !label
        .get(..ACE.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(ACE))
    {
        if let Some(pos) = label.iter().position(|b| !b.is_ascii()) {
            return Err(LabelError::new(pos, pos + 1, "ordinal not in range(128)"));
        }
        return Ok(String::from_utf8(label.to_vec()).expect("ASCII"));
    }
    let points = punycode::decode(&label[ACE.len()..]).map_err(|err| err.at_offset(ACE.len()))?;
    let ascii = to_ascii(&points).map_err(|err| match err {
        LabelError::Invalid { text: None, .. } => err.with_text(points.clone()),
        _ => err,
    })?;
    if !label.eq_ignore_ascii_case(&ascii) {
        let mut reason = String::from("IDNA does not round-trip, '");
        super::super::write_bytes_repr(&mut reason, label);
        reason.push_str("' != '");
        super::super::write_bytes_repr(&mut reason, &ascii);
        reason.push('\'');
        return Err(LabelError::new(0, label.len(), reason));
    }
    Ok(points
        .into_iter()
        .map(|c| char::from_u32(c).expect("ToASCII prohibits surrogates"))
        .collect())
}

pub(super) fn decode(data: &[u8], errors: Option<&str>) -> Result<String> {
    check_errors(errors)?;
    if data.is_ascii() && !data.windows(ACE.len()).any(|s| s.eq_ignore_ascii_case(ACE)) {
        return Ok(String::from_utf8(data.to_vec()).expect("ASCII"));
    }
    let mut text = String::new();
    let mut offset = 0;
    for label in data.split(|&b| b == b'.') {
        if offset > 0 {
            text.push('.');
        }
        text.push_str(
            &to_unicode(label).map_err(|err| err.at_offset(offset).into_decode("idna", data))?,
        );
        offset += label.len() + 1;
    }
    Ok(text)
}
