//! Layout and decoding of native and standard struct records.

#[derive(Clone, Copy)]
struct Field {
    code: u8,
    offset: usize,
    count: usize,
    width: usize,
}

/// A value decoded from a struct record. Padding contributes no value.
#[derive(Debug, PartialEq)]
pub(crate) enum UnpackedField {
    Bool(bool),
    Integer(u64),
    Signed(i64),
    Float(f64),
    Bytes(Vec<u8>),
}

pub(crate) struct StructFormat {
    native: bool,
    pub(crate) little_endian: bool,
    pub(crate) size: usize,
    pub(crate) item_count: usize,
    fields: Vec<Field>,
}

impl StructFormat {
    pub(crate) fn parse(format: &str) -> Result<Self, String> {
        let mut chars = format.bytes().peekable();
        let prefix = if matches!(chars.peek(), Some(b'@' | b'=' | b'<' | b'>' | b'!')) {
            chars.next().unwrap()
        } else {
            b'@'
        };
        let mut result = Self {
            native: prefix == b'@',
            little_endian: match prefix {
                b'<' => true,
                b'>' | b'!' => false,
                _ => cfg!(target_endian = "little"),
            },
            size: 0,
            item_count: 0,
            fields: Vec::new(),
        };
        let mut repeat = None;
        let overflow = "total struct size too long";
        for c in chars {
            if c.is_ascii_digit() {
                repeat = Some(
                    repeat
                        .unwrap_or(0usize)
                        .checked_mul(10)
                        .and_then(|n| n.checked_add(usize::from(c - b'0')))
                        .filter(|&n| n <= isize::MAX as usize)
                        .ok_or(overflow)?,
                );
                continue;
            }
            if c.is_ascii_whitespace() && repeat.is_none() {
                continue;
            }
            let repeat_count = repeat.take().unwrap_or(1);
            let count = if matches!(c, b's' | b'p') {
                1
            } else {
                repeat_count
            };
            let (width, alignment) = match c {
                b's' | b'p' => (repeat_count, 1),
                b'L' if result.native => (
                    size_of::<std::ffi::c_ulong>(),
                    align_of::<std::ffi::c_ulong>(),
                ),
                b'l' if result.native => (
                    size_of::<std::ffi::c_long>(),
                    align_of::<std::ffi::c_long>(),
                ),
                b'L' | b'l' => (4, 1),
                b'c' | b'b' | b'B' | b'?' => (1, 1),
                b'h' | b'H' => (2, align_of::<u16>()),
                b'i' | b'I' => (4, align_of::<u32>()),
                b'f' => (4, align_of::<f32>()),
                b'd' => (8, align_of::<f64>()),
                b'q' | b'Q' => (8, align_of::<u64>()),
                b'x' => (1, 1),
                b'P' if result.native => (size_of::<usize>(), align_of::<usize>()),
                _ => return Err("bad char in struct format".into()),
            };
            let offset = if result.native {
                result
                    .size
                    .checked_next_multiple_of(alignment)
                    .ok_or(overflow)?
            } else {
                result.size
            };
            result.size = count
                .checked_mul(width)
                .and_then(|n| n.checked_add(offset))
                .filter(|&size| size <= isize::MAX as usize)
                .ok_or(overflow)?;
            if c == b'x' {
                continue; // Padding occupies bytes without consuming a value.
            }
            result.item_count = result
                .item_count
                .checked_add(count)
                .filter(|&n| n <= isize::MAX as usize)
                .ok_or(overflow)?;
            result.fields.push(Field {
                code: c,
                offset,
                count,
                width,
            });
        }
        if repeat.is_some() {
            return Err("repeat count given without format specifier".into());
        }
        Ok(result)
    }

    /// Yield one argument and byte range per value, including a single empty range for 0s or 0p.
    pub(crate) fn items(&self) -> impl Iterator<Item = (u8, std::ops::Range<usize>)> + '_ {
        self.fields.iter().flat_map(|field| {
            (0..field.count).map(move |index| {
                let start = field.offset + index * field.width;
                (field.code, start..start + field.width)
            })
        })
    }

    pub(crate) fn write_integer(&self, target: &mut [u8], value: u64) {
        if self.little_endian {
            target.copy_from_slice(&value.to_le_bytes()[..target.len()]);
        } else {
            target.copy_from_slice(&value.to_be_bytes()[8 - target.len()..]);
        }
    }

    /// Decode a record at a byte offset, with negative offsets measured from the end.
    pub(crate) fn unpack_from(
        &self,
        buffer: &[u8],
        offset: isize,
    ) -> Result<Vec<UnpackedField>, String> {
        let range = self.record_range(buffer.len(), offset, false)?;
        self.unpack(&buffer[range])
    }

    /// Locate one record for pack_into or unpack_from, preserving their distinct diagnostics.
    pub(crate) fn record_range(
        &self,
        len: usize,
        offset: isize,
        packing: bool,
    ) -> Result<std::ops::Range<usize>, String> {
        let size = self.size;
        let start = if offset < 0 {
            let distance = offset.unsigned_abs();
            if size > distance {
                return Err(if packing {
                    format!("no space to pack {size} bytes at offset {offset}")
                } else {
                    format!("not enough data to unpack {size} bytes at offset {offset}")
                });
            }
            len.checked_sub(distance)
                .ok_or_else(|| format!("offset {offset} out of range for {len}-byte buffer"))?
        } else {
            offset as usize
        };
        // Both the parsed size and the nonnegative offset fit in isize.
        let end = start + size;
        if end > len {
            let (method, action) = if packing {
                ("pack_into", "packing")
            } else {
                ("unpack_from", "unpacking")
            };
            return Err(format!(
                "{method} requires a buffer of at least {end} bytes for {action} {size} bytes at offset {start} (actual buffer size is {len})"
            ));
        }
        Ok(start..end)
    }

    fn unpack_single(&self, bits: u32) -> f64 {
        let single = f32::from_bits(bits);
        if !self.native && single.is_nan() {
            // Standard formats preserve signaling NaNs. Native unpacking uses a float cast.
            let sign = u64::from(bits >> 31) << 63;
            let fraction = u64::from(bits & 0x007f_ffff) << 29;
            return f64::from_bits(sign | f64::INFINITY.to_bits() | fraction);
        }
        f64::from(single)
    }

    pub(crate) fn unpack(&self, buffer: &[u8]) -> Result<Vec<UnpackedField>, String> {
        if buffer.len() != self.size {
            return Err(format!("unpack requires a buffer of {} bytes", self.size));
        }
        let mut result = Vec::new();
        for (code, range) in self.items() {
            let field = &buffer[range];
            if matches!(code, b'c' | b's' | b'p') {
                let data = match (code, field) {
                    (b'p', [length, data @ ..]) => &data[..usize::from(*length).min(data.len())],
                    _ => field,
                };
                result.push(UnpackedField::Bytes(data.to_vec()));
                continue;
            }
            let mut word = [0u8; 8];
            let value = if self.little_endian {
                word[..field.len()].copy_from_slice(field);
                u64::from_le_bytes(word)
            } else {
                word[8 - field.len()..].copy_from_slice(field);
                u64::from_be_bytes(word)
            };
            result.push(match code {
                b'?' => UnpackedField::Bool(value != 0),
                b'b' | b'h' | b'i' | b'l' | b'q' => {
                    // Move the field's sign bit into bit 63 before the arithmetic shift.
                    let shift = (8 - field.len()) * 8;
                    UnpackedField::Signed(((value << shift) as i64) >> shift)
                }
                b'f' => UnpackedField::Float(self.unpack_single(value as u32)),
                b'd' => UnpackedField::Float(f64::from_bits(value)),
                _ => UnpackedField::Integer(value),
            });
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn struct_calcsize(format: &str) -> Result<usize, String> {
        Ok(StructFormat::parse(format)?.size)
    }

    fn struct_unpack(format: &str, buffer: &[u8]) -> Result<Vec<UnpackedField>, String> {
        StructFormat::parse(format)?.unpack(buffer)
    }

    #[test]
    fn elf_integer_fields_respect_byte_order() {
        use UnpackedField::Integer;
        let header = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        assert_eq!(
            struct_unpack(">HI", &header).unwrap(),
            [Integer(0x0102), Integer(0x03040506)]
        );
        assert_eq!(
            struct_unpack("<HI", &header).unwrap(),
            [Integer(0x0201), Integer(0x06050403)]
        );
        assert_eq!(struct_unpack(">Q", &[255; 8]).unwrap(), [Integer(u64::MAX)]);
        assert_eq!(struct_calcsize("16B").unwrap(), 16);
        assert_eq!(struct_calcsize("<HHIIIIIHHH").unwrap(), 30);
        assert_eq!(struct_calcsize(">HHIQQQIHHH").unwrap(), 42);
        assert_eq!(struct_calcsize("<IIQQQQQQ").unwrap(), 56);
    }

    #[test]
    fn signed_long_long_transitions() {
        use UnpackedField::Signed;
        let values = [i64::MIN, -2_717_650_800, 0, i64::MAX];
        let big: Vec<_> = values.into_iter().flat_map(i64::to_be_bytes).collect();
        let little: Vec<_> = values.into_iter().flat_map(i64::to_le_bytes).collect();
        assert_eq!(struct_unpack(">4q", &big).unwrap(), values.map(Signed));
        assert_eq!(struct_unpack("<4q", &little).unwrap(), values.map(Signed));
        assert_eq!(struct_calcsize("@B0q"), struct_calcsize("@B0Q"));
        assert_eq!(struct_calcsize("!Bq"), Ok(9));
    }

    #[test]
    fn native_alignment_and_invalid_buffers() {
        let max = isize::MAX as usize;
        assert_eq!(struct_calcsize(&format!("{max}x0s")), Ok(max));
        assert_eq!(
            struct_calcsize(&format!("{max}B0s")).unwrap_err(),
            "total struct size too long"
        );
        assert_eq!(struct_calcsize("P").unwrap(), size_of::<usize>());
        assert_eq!(struct_calcsize("@BI").unwrap(), 8);
        assert_eq!(struct_calcsize("=BI").unwrap(), 5);
        assert_eq!(struct_calcsize("B0I").unwrap(), 4);
        assert!(struct_calcsize("<P").is_err());
        assert!(struct_calcsize("3").is_err());
        for size in [3, 5] {
            assert_eq!(
                struct_unpack("<I", &vec![0; size]).unwrap_err(),
                "unpack requires a buffer of 4 bytes"
            );
        }
    }
}
