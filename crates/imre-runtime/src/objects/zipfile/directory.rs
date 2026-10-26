//! Locate and decode the central directory. It records every name in archive order;
//! a map-based ZIP index would erase duplicate members.

use crate::StrOwner;
use std::io::{BufReader, Read, Seek, SeekFrom};

use super::super::{
    Str, bytes,
    exceptions::{Error, Kind, Result},
    files,
};
use super::Info;

const END: &[u8; 4] = b"PK\x05\x06";
const END64: &[u8; 4] = b"PK\x06\x06";
const LOCATOR64: &[u8; 4] = b"PK\x06\x07";
const CENTRAL: &[u8; 4] = b"PK\x01\x02";

pub(super) fn bad(message: impl Into<Str>) -> Error {
    Error::new(Kind::BadZipFile, message)
}

fn not_zip() -> Error {
    bad("File is not a zip file")
}

pub(super) fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

pub(super) fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn read_at(reader: &mut (impl Read + Seek), offset: u64, buf: &mut [u8]) -> Result<()> {
    reader
        .seek(SeekFrom::Start(offset))
        .and_then(|_| reader.read_exact(buf))
        .map_err(|_| not_zip())
}

pub(super) fn grow(buf: &mut Vec<u8>, size: usize) -> Result<()> {
    buf.clear();
    buf.try_reserve_exact(size)
        .map_err(|_| Error::allocation())?;
    buf.resize(size, 0);
    Ok(())
}

/// A ZIP64 locator, when present, supplies the actual 64-bit directory size and location.
fn directory_bounds(
    reader: &mut (impl Read + Seek),
    end: u64,
    size: u32,
    offset: u32,
) -> Result<(u64, u64, u64)> {
    let Some(locator_at) = end.checked_sub(20) else {
        return Ok((end, size.into(), offset.into()));
    };
    let mut locator = [0; 20];
    read_at(reader, locator_at, &mut locator)?;
    if !locator.starts_with(LOCATOR64) {
        return Ok((end, size.into(), offset.into()));
    }
    if u32_at(&locator, 4) != 0 || u32_at(&locator, 16) > 1 {
        return Err(bad("zipfiles that span multiple disks are not supported"));
    }
    let recorded = u64_at(&locator, 8);
    let fixed = locator_at
        .checked_sub(56)
        .filter(|&at| recorded <= at)
        .ok_or_else(|| bad("Corrupt zip64 end of central directory locator"))?;
    let mut end64 = [0; 56];
    let mut position = recorded;
    read_at(reader, position, &mut end64)?;
    if !end64.starts_with(END64) && recorded != fixed {
        // An executable stub may have been prepended to the archive.
        position = fixed;
        read_at(reader, position, &mut end64)?;
    }
    if !end64.starts_with(END64) {
        return Err(bad("Zip64 end of central directory record not found"));
    }
    let size = u64_at(&end64, 40);
    if u64_at(&end64, 48).checked_add(size) != Some(recorded)
        || u64_at(&end64, 4).checked_add(12) != Some(locator_at - position)
    {
        return Err(bad("Corrupt zip64 end of central directory record"));
    }
    Ok((position, size, u64_at(&end64, 48)))
}

/// Read the usual fixed trailer first. With a comment, search at most its 64 KiB limit.
fn find_directory(reader: &mut (impl Read + Seek)) -> Result<(u64, u64, i128)> {
    let length = reader.seek(SeekFrom::End(0)).map_err(|_| not_zip())?;
    let position = length.checked_sub(22).ok_or_else(not_zip)?;
    let mut end = [0; 22];
    read_at(reader, position, &mut end)?;
    let position = if end.starts_with(END) && u16_at(&end, 20) == 0 {
        position
    } else {
        let tail_size = length.min(22 + u64::from(u16::MAX)) as usize;
        let mut tail = Vec::new();
        grow(&mut tail, tail_size)?;
        let start = length - tail_size as u64;
        read_at(reader, start, &mut tail)?;
        let at = memchr::memmem::rfind(&tail, END).ok_or_else(not_zip)?;
        end.copy_from_slice(tail.get(at..at + 22).ok_or_else(not_zip)?);
        start + at as u64
    };
    let (end, size, offset) =
        directory_bounds(reader, position, u32_at(&end, 12), u32_at(&end, 16))?;
    let start = end
        .checked_sub(size)
        .ok_or_else(|| bad("Bad offset for central directory"))?;
    Ok((start, size, i128::from(start) - i128::from(offset)))
}

// The upper half of IBM code page 437, the default ZIP filename encoding.
const CP437: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ',
    'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ',
    'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕',
    '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦',
    '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐',
    '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', '≡', '±',
    '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

pub(super) fn name(raw: &[u8], utf8: bool) -> Result<Str> {
    if utf8 || raw.is_ascii() {
        let text = std::str::from_utf8(raw).map_err(|error| bytes::decode_error(raw, error))?;
        return Str::try_new(text).map_err(|_| Error::allocation());
    }
    let mut text = Str::try_with_capacity(raw.len() * 3).map_err(|_| Error::allocation())?;
    for &ch in raw {
        text.push(if ch < 128 {
            char::from(ch)
        } else {
            CP437[usize::from(ch - 128)]
        });
    }
    Ok(text)
}

/// ZIP64 may replace sentinel sizes/offsets, and Info-ZIP may override the display name.
fn parse_extra(raw: &[u8], mut extra: &[u8], info: &mut Info) -> Result<()> {
    while extra.len() >= 4 {
        let tag = u16_at(extra, 0);
        let len = usize::from(u16_at(extra, 2));
        let data = extra
            .get(4..4 + len)
            .ok_or_else(|| bad(format!("Corrupt extra field {tag:04x} (size={len})")))?;
        if tag == 1 {
            let mut words = data.as_chunks::<8>().0.iter();
            for (slot, field) in [
                (&mut info.file_size, "File size"),
                (&mut info.compress_size, "Compress size"),
                (&mut info.local_offset, "Header offset"),
            ] {
                if *slot == u64::from(u32::MAX) || (field == "File size" && *slot == u64::MAX) {
                    let word = words.next().ok_or_else(|| {
                        bad(format!("Corrupt zip64 extra field. {field} not found."))
                    })?;
                    *slot = u64::from_le_bytes(*word);
                }
            }
        } else if tag == 0x7075 {
            if data.len() < 5 {
                return Err(bad("Corrupt unicode path extra field (0x7075)"));
            }
            let mut checksum = flate2::Crc::new();
            checksum.update(raw);
            if data[0] == 1 && u32_at(data, 1) == checksum.sum() && data.len() > 5 {
                let text = std::str::from_utf8(&data[5..]).map_err(|_| {
                    bad("Corrupt unicode path extra field (0x7075): invalid utf-8 bytes")
                })?;
                info.filename = StrOwner::try_new(
                    Str::try_new(text.split('\0').next().unwrap())
                        .map_err(|_| Error::allocation())?,
                )
                .map_err(|_| Error::allocation())?;
            }
        }
        extra = &extra[4 + len..];
    }
    Ok(())
}

pub(super) fn entries(reader: &mut (impl Read + Seek)) -> Result<Vec<Info>> {
    let (start, mut remaining, prefix) = find_directory(reader)?;
    reader
        .seek(SeekFrom::Start(start))
        .map_err(files::errno_error)?;
    let mut source = BufReader::new(reader.take(remaining));
    let mut entries = Vec::new();
    let mut buf = Vec::new();
    let mut header = [0; 46];
    while remaining > 0 {
        source
            .read_exact(&mut header)
            .map_err(|_| bad("Truncated central directory"))?;
        if !header.starts_with(CENTRAL) {
            return Err(bad("Bad magic number for central directory"));
        }
        let name_len = usize::from(u16_at(&header, 28));
        let extra_len = usize::from(u16_at(&header, 30));
        let comment_len = usize::from(u16_at(&header, 32));
        remaining -= 46;
        let n = remaining.min((name_len + extra_len + comment_len) as u64) as usize;
        grow(&mut buf, n)?;
        source
            .read_exact(&mut buf)
            .map_err(|_| bad("Truncated central directory"))?;
        remaining -= n as u64;
        let raw = &buf[..name_len.min(n)];
        let flags = u16_at(&header, 8);
        let original = StrOwner::try_new(name(raw, flags & (1 << 11) != 0)?)
            .map_err(|_| Error::allocation())?;
        let version = header[6];
        if version > 63 {
            return Err(Error::new(
                Kind::NotImplementedError,
                format!("zip file version {:.1}", f32::from(version) / 10.0),
            ));
        }
        let filename = if let Some((prefix, _)) = original.split_once('\0') {
            StrOwner::try_new(Str::try_new(prefix).map_err(|_| Error::allocation())?)
                .map_err(|_| Error::allocation())?
        } else {
            original.clone()
        };
        let mut info = Info {
            filename,
            original,
            flags,
            method: u16_at(&header, 10),
            crc: u32_at(&header, 16),
            compress_size: u64::from(u32_at(&header, 20)),
            file_size: u64::from(u32_at(&header, 24)),
            external_attr: u32_at(&header, 38),
            local_offset: u64::from(u32_at(&header, 42)),
            offset: 0,
            end: 0,
        };
        parse_extra(
            raw,
            &buf[raw.len()..(name_len + extra_len).min(n)],
            &mut info,
        )?;
        info.offset = i128::from(info.local_offset) + prefix;
        entries.try_reserve(1).map_err(|_| Error::allocation())?;
        entries.push(info);
    }
    // Each entry must finish before the next local header. Stable order for
    // equal offsets distinguishes shared headers from overlapping payloads.
    let mut order = Vec::new();
    order
        .try_reserve_exact(entries.len())
        .map_err(|_| Error::allocation())?;
    order.extend(0..entries.len());
    order.sort_unstable_by_key(|&i| (entries[i].offset, i));
    let mut end = i128::from(start);
    for i in order.into_iter().rev() {
        entries[i].end = end;
        end = entries[i].offset;
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // No payload or local file header is present. A directory listing must
    // neither try to open a member nor merge entries that have the same name.
    fn index(records: &[(&[u8], u16, &[u8])], comment: &[u8]) -> Vec<u8> {
        let mut bytes = b"executable stub".to_vec();
        let start = bytes.len();
        for &(name, flags, extra) in records {
            let mut header = [0; 46];
            header[..4].copy_from_slice(CENTRAL);
            header[8..10].copy_from_slice(&flags.to_le_bytes());
            header[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
            header[30..32].copy_from_slice(&(extra.len() as u16).to_le_bytes());
            bytes.extend(header);
            bytes.extend(name);
            bytes.extend(extra);
        }
        let mut trailer = [0; 22];
        trailer[..4].copy_from_slice(END);
        trailer[12..16].copy_from_slice(&((bytes.len() - start) as u32).to_le_bytes());
        trailer[20..22].copy_from_slice(&(comment.len() as u16).to_le_bytes());
        bytes.extend(trailer);
        bytes.extend(comment);
        bytes
    }

    #[test]
    fn listing_preserves_order_duplicates_and_filename_encodings() {
        let utf8 = 1 << 11;
        let fixture = index(
            &[
                (b"\x82", 0, &[]),
                (b"nul\0ignored", 0, &[]),
                ("ß".as_bytes(), utf8, &[]),
                (b"\x82", 0, &[]),
            ],
            b"archive comment",
        );
        let found = entries(&mut Cursor::new(fixture)).ok().unwrap();
        assert_eq!(
            found
                .iter()
                .map(|info| info.filename.as_str())
                .collect::<Vec<_>>(),
            ["é", "nul", "ß", "é"]
        );
    }

    #[test]
    fn unicode_extra_requires_matching_crc_and_strict_utf8() {
        let mut extra = vec![0x75, 0x70, 7, 0, 1];
        let mut checksum = flate2::Crc::new();
        checksum.update(b"a");
        extra.extend(checksum.sum().to_le_bytes());
        extra.extend("é".as_bytes());
        let fixture = index(&[(b"a", 0, &extra), (b"b", 0, &extra)], b"");
        let found = entries(&mut Cursor::new(fixture)).ok().unwrap();
        assert_eq!(
            found
                .iter()
                .map(|info| info.filename.as_str())
                .collect::<Vec<_>>(),
            ["é", "b"]
        );
        assert!(!StrOwner::ptr_eq(&found[0].filename, &found[0].original));
        assert!(StrOwner::ptr_eq(&found[1].filename, &found[1].original));
        extra[9] = 0xff;
        assert!(entries(&mut Cursor::new(index(&[(b"a", 0, &extra)], b""))).is_err());
    }
}
