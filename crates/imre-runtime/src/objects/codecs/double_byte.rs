//! Packed character maps for the stateless Shift JIS, Big5 and EUC-JP codecs.
use super::{DecodeIssue, Error, Result};

#[derive(Clone, Copy)]
pub(crate) enum DoubleByte {
    ShiftJis,
    Big5,
    EucJp,
}

const UNMAPPED: u16 = 0xfffe;
type Page = [[u8; 2]; 256];

// Rows and columns omit byte-range gaps. The packed tables are reproducible
// with scripts/generate-shift-jis.py on the pinned CPython.
const SHIFT_JIS: &[u8; 42 * 188 * 2] = include_bytes!("shift_jis.bin");
const JIS_X_0212: &[u8; 77 * 94 * 2] = include_bytes!("jis_x_0212.bin");
const BIG5: &[u8; 89 * 157 * 2] = include_bytes!("big5.bin");

const fn page_index(table: &[u8]) -> ([u8; 256], usize) {
    let mut pages = [u8::MAX; 256];
    let mut count = 0;
    let mut i = 0;
    while i < table.len() {
        let scalar = u16::from_le_bytes([table[i], table[i + 1]]);
        let page = table[i + 1] as usize;
        if scalar != UNMAPPED && pages[page] == u8::MAX {
            pages[page] = count as u8;
            count += 1;
        }
        i += 2;
    }
    (pages, count)
}

const SHIFT_JIS_PAGES: ([u8; 256], usize) = page_index(SHIFT_JIS);
const JIS_X_0212_PAGES: ([u8; 256], usize) = page_index(JIS_X_0212);
const BIG5_PAGES: ([u8; 256], usize) = page_index(BIG5);

const fn invert<const N: usize>(format: DoubleByte, index: &[u8; 256]) -> [Page; N] {
    let table = format.table();
    let mut pages = [[[0; 2]; 256]; N];
    let mut i = 0;
    while i < table.len() / 2 {
        let low = table[2 * i];
        let high = table[2 * i + 1];
        if u16::from_le_bytes([low, high]) != UNMAPPED {
            let row = (i / format.columns()) as u8;
            let col = (i % format.columns()) as u8;
            let lead = match format {
                DoubleByte::ShiftJis if row < 31 => 0x81 + row,
                DoubleByte::ShiftJis => 0xc1 + row,
                DoubleByte::Big5 | DoubleByte::EucJp => 0xa1 + row,
            };
            let trail = match format {
                DoubleByte::EucJp => 0xa1 + col,
                _ if col < 63 => 0x40 + col,
                DoubleByte::ShiftJis => 0x41 + col,
                DoubleByte::Big5 => 0xa1 + (col - 63),
            };
            // Later pairs are canonical for the four duplicate Big5 mappings.
            pages[index[high as usize] as usize][low as usize] = [lead, trail];
        }
        i += 1;
    }
    pages
}

static SHIFT_JIS_ENCODE: [Page; SHIFT_JIS_PAGES.1] =
    invert(DoubleByte::ShiftJis, &SHIFT_JIS_PAGES.0);
static JIS_X_0212_ENCODE: [Page; JIS_X_0212_PAGES.1] =
    invert(DoubleByte::EucJp, &JIS_X_0212_PAGES.0);
static BIG5_ENCODE: [Page; BIG5_PAGES.1] = invert(DoubleByte::Big5, &BIG5_PAGES.0);

impl DoubleByte {
    const fn table(self) -> &'static [u8] {
        match self {
            Self::ShiftJis => SHIFT_JIS,
            Self::Big5 => BIG5,
            Self::EucJp => JIS_X_0212,
        }
    }

    const fn columns(self) -> usize {
        match self {
            Self::ShiftJis => 188,
            Self::Big5 => 157,
            Self::EucJp => 94,
        }
    }

    pub(crate) fn encode_char(self, ch: char) -> Option<([u8; 3], usize)> {
        let byte = match (self, ch) {
            (_, '\0'..='\u{7f}') => ch as u8,
            (Self::ShiftJis | Self::EucJp, '\u{a5}') => 0x5c,
            (Self::ShiftJis | Self::EucJp, '\u{203e}') => 0x7e,
            (Self::ShiftJis | Self::EucJp, '\u{ff61}'..='\u{ff9f}') => {
                let kana = 0xa1 + (u32::from(ch) - 0xff61) as u8;
                if matches!(self, Self::EucJp) {
                    return Some(([0x8e, kana, 0], 2));
                }
                kana
            }
            _ => {
                let [low, high] = u16::try_from(u32::from(ch)).ok()?.to_le_bytes();
                let lookup = |index: &[u8; 256], pages: &[Page]| {
                    let page = pages.get(usize::from(index[usize::from(high)]))?;
                    let pair = page[usize::from(low)];
                    (pair[0] != 0).then_some(pair)
                };
                let [lead, trail] = match self {
                    Self::ShiftJis => lookup(&SHIFT_JIS_PAGES.0, &SHIFT_JIS_ENCODE)?,
                    Self::Big5 => lookup(&BIG5_PAGES.0, &BIG5_ENCODE)?,
                    Self::EucJp => {
                        // Shift JIS packs two JIS X 0208 rows into one lead byte.
                        if let Some([lead, trail]) = lookup(&SHIFT_JIS_PAGES.0, &SHIFT_JIS_ENCODE) {
                            let row = lead - if lead < 0xa0 { 0x81 } else { 0xc1 };
                            let col = trail - if trail < 0x7f { 0x40 } else { 0x41 };
                            [0xa1 + row * 2 + col / 94, 0xa1 + col % 94]
                        } else {
                            let [lead, trail] = lookup(&JIS_X_0212_PAGES.0, &JIS_X_0212_ENCODE)?;
                            return Some(([0x8f, lead, trail], 3));
                        }
                    }
                };
                return Some(([lead, trail, 0], 2));
            }
        };
        Some(([byte, 0, 0], 1))
    }

    #[inline(always)]
    fn character(
        self,
        data: &[u8],
        start: usize,
    ) -> std::result::Result<(char, usize), DecodeIssue> {
        let lead = data[start];
        if lead.is_ascii() {
            return Ok((char::from(lead), 1));
        }
        let error = |reason| DecodeIssue {
            encoding: super::Codec::DoubleByte(self).name(),
            start,
            end: start + 1,
            reason,
        };
        if matches!(self, Self::EucJp) {
            let width = if lead == 0x8f { 3 } else { 2 };
            let Some(unit) = data[start..].get(..width) else {
                return Err(DecodeIssue {
                    end: data.len(),
                    ..error("incomplete multibyte sequence")
                });
            };
            let (table, row, col): (&[u8], _, _) = match unit {
                [0x8e, kana @ 0xa1..=0xdf] => {
                    return Ok((char::from_u32(0xfec0 + u32::from(*kana)).unwrap(), 2));
                }
                [row @ 0xa1..=0xf4, col @ 0xa1..=0xfe] => (SHIFT_JIS, row, col),
                [0x8f, row @ 0xa1..=0xed, col @ 0xa1..=0xfe] => (JIS_X_0212, row, col),
                _ => return Err(error("illegal multibyte sequence")),
            };
            let index = usize::from(row - 0xa1) * 94 + usize::from(col - 0xa1);
            return mapped_character(table, index)
                .map(|ch| (ch, width))
                .ok_or_else(|| error("illegal multibyte sequence"));
        }
        let row = match (self, lead) {
            (Self::ShiftJis, 0xa1..=0xdf) => {
                return Ok((char::from_u32(0xff61 + u32::from(lead - 0xa1)).unwrap(), 1));
            }
            (Self::ShiftJis, 0x81..=0x9f) => lead - 0x81,
            (Self::ShiftJis, 0xe0..=0xea) => lead - 0xc1,
            // Big5 waits for a trail before diagnosing *any* non-ASCII lead.
            (Self::Big5, _) if start + 1 == data.len() => {
                return Err(error("incomplete multibyte sequence"));
            }
            (Self::Big5, 0xa1..=0xf9) => lead - 0xa1,
            _ => return Err(error("illegal multibyte sequence")),
        };
        let trail = *data
            .get(start + 1)
            .ok_or_else(|| error("incomplete multibyte sequence"))?;
        let col = match (self, trail) {
            (_, 0x40..=0x7e) => trail - 0x40,
            (Self::ShiftJis, 0x80..=0xfc) => trail - 0x41,
            (Self::Big5, 0xa1..=0xfe) => trail - 0xa1 + 63,
            _ => return Err(error("illegal multibyte sequence")),
        };
        let index = usize::from(row) * self.columns() + usize::from(col);
        mapped_character(self.table(), index)
            .map(|ch| (ch, 2))
            .ok_or_else(|| error("illegal multibyte sequence"))
    }

    /// Append complete characters until an error or an incomplete, non-final lead.
    pub(crate) fn decode_prefix(
        self,
        text: &mut String,
        data: &[u8],
        offset: usize,
        final_: bool,
    ) -> Result<(usize, Option<DecodeIssue>)> {
        // Dispatch once per input buffer to specialize byte classification in the hot loop.
        match self {
            Self::ShiftJis => Self::ShiftJis.append_prefix(text, data, offset, final_),
            Self::Big5 => Self::Big5.append_prefix(text, data, offset, final_),
            Self::EucJp => Self::EucJp.append_prefix(text, data, offset, final_),
        }
    }

    #[inline(always)]
    fn append_prefix(
        self,
        text: &mut String,
        data: &[u8],
        mut offset: usize,
        final_: bool,
    ) -> Result<(usize, Option<DecodeIssue>)> {
        while offset < data.len() {
            match self.character(data, offset) {
                Ok((ch, size)) => {
                    text.try_reserve(ch.len_utf8())
                        .map_err(|_| Error::allocation())?;
                    text.push(ch);
                    offset += size;
                }
                Err(issue) if !final_ && issue.reason == "incomplete multibyte sequence" => break,
                Err(issue) => return Ok((offset, Some(issue))),
            }
        }
        Ok((offset, None))
    }

    /// Decode a whole caller buffer, preserving it in error objects even when its final lead is pending.
    pub(crate) fn decode_chunk(
        self,
        text: &mut String,
        data: &[u8],
        errors: Option<&str>,
        final_: bool,
    ) -> Result<usize> {
        let mut offset = 0;
        loop {
            let (consumed, issue) = self.decode_prefix(text, data, offset, final_)?;
            let Some(issue) = issue else {
                return Ok(consumed);
            };
            super::decoding_error(text, errors, &data[issue.start..issue.end], || {
                issue.error(data)
            })?;
            offset = issue.end;
        }
    }

    /// POSIX filesystem decoding preserves every unmapped byte as a low surrogate.
    pub(super) fn decode_filesystem(self, data: &[u8], points: &mut Vec<u32>) {
        let mut offset = 0;
        while offset < data.len() {
            match self.character(data, offset) {
                Ok((ch, size)) => {
                    points.push(u32::from(ch));
                    offset += size;
                }
                Err(_) => {
                    points.push(0xdc00 + u32::from(data[offset]));
                    offset += 1;
                }
            }
        }
    }
}

#[inline]
fn mapped_character(table: &[u8], index: usize) -> Option<char> {
    let at = index * 2;
    let value = u16::from_le_bytes([table[at], table[at + 1]]);
    (value != UNMAPPED).then(|| char::from_u32(u32::from(value)).unwrap())
}
