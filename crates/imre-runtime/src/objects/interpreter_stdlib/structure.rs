//! The unsigned integer and native pointer formats of Python's struct module.

#[derive(Clone, Copy)]
struct Field {
    code: u8,
    offset: usize,
    count: usize,
    width: usize,
}

pub(crate) struct StructFormat {
    little_endian: bool,
    pub(crate) size: usize,
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
            little_endian: match prefix {
                b'<' => true,
                b'>' | b'!' => false,
                _ => cfg!(target_endian = "little"),
            },
            size: 0,
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
                        .ok_or(overflow)?,
                );
                continue;
            }
            if c.is_ascii_whitespace() && repeat.is_none() {
                continue;
            }
            let (width, alignment) = match c {
                b'B' => (1, 1),
                b'H' => (2, align_of::<u16>()),
                b'I' => (4, align_of::<u32>()),
                b'Q' => (8, align_of::<u64>()),
                b'P' if prefix == b'@' => (size_of::<usize>(), align_of::<usize>()),
                _ => return Err("bad char in struct format".into()),
            };
            let count = repeat.take().unwrap_or(1);
            let offset = if prefix == b'@' {
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

    pub(crate) fn item_count(&self) -> usize {
        self.fields.iter().map(|field| field.count).sum()
    }

    /// Fill the checked, zero-padded output in Python argument order.
    pub(crate) fn write_words<E>(
        &self,
        output: &mut [u8],
        mut word: impl FnMut(u8, usize) -> Result<u64, E>,
    ) -> Result<(), E> {
        for field in &self.fields {
            let end = field.offset + field.count * field.width;
            for target in output[field.offset..end].chunks_exact_mut(field.width) {
                let value = word(field.code, field.width)?;
                if self.little_endian {
                    target.copy_from_slice(&value.to_le_bytes()[..field.width]);
                } else {
                    target.copy_from_slice(&value.to_be_bytes()[8 - field.width..]);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn unpack(&self, buffer: &[u8]) -> Result<Vec<u64>, String> {
        if buffer.len() != self.size {
            return Err(format!("unpack requires a buffer of {} bytes", self.size));
        }
        let mut result = Vec::new();
        for &Field {
            offset,
            count,
            width,
            ..
        } in &self.fields
        {
            for field in buffer[offset..offset + count * width].chunks_exact(width) {
                let mut word = [0u8; 8];
                let value = if self.little_endian {
                    word[..width].copy_from_slice(field);
                    u64::from_le_bytes(word)
                } else {
                    word[8 - width..].copy_from_slice(field);
                    u64::from_be_bytes(word)
                };
                result.push(value);
            }
        }
        Ok(result)
    }
}

pub(crate) fn struct_calcsize(format: &str) -> Result<usize, String> {
    Ok(StructFormat::parse(format)?.size)
}

pub(crate) fn struct_unpack(format: &str, buffer: &[u8]) -> Result<Vec<u64>, String> {
    StructFormat::parse(format)?.unpack(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elf_integer_fields_respect_byte_order() {
        let header = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        assert_eq!(struct_unpack(">HI", &header).unwrap(), [0x0102, 0x03040506]);
        assert_eq!(struct_unpack("<HI", &header).unwrap(), [0x0201, 0x06050403]);
        assert_eq!(struct_unpack(">Q", &[255; 8]).unwrap(), [u64::MAX]);
        assert_eq!(struct_calcsize("16B").unwrap(), 16);
        assert_eq!(struct_calcsize("<HHIIIIIHHH").unwrap(), 30);
        assert_eq!(struct_calcsize(">HHIQQQIHHH").unwrap(), 42);
        assert_eq!(struct_calcsize("<IIQQQQQQ").unwrap(), 56);
    }

    #[test]
    fn native_alignment_and_invalid_buffers() {
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
