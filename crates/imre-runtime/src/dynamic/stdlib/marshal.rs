use num_bigint::{BigInt, Sign};

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};
use crate::Int;
use crate::dynamic::value::ELLIPSIS;

fn read<const N: usize>(data: &[u8]) -> Result<[u8; N]> {
    data.first_chunk().copied().ok_or_else(too_short)
}

fn too_short() -> Error {
    Error::new("EOFError", "marshal data too short")
}

/// Decode a signed count of little-endian 15-bit digits.
fn read_long(data: &[u8]) -> Result<Int> {
    let size = i32::from_le_bytes(read(data)?);
    let count = size
        .checked_abs()
        .ok_or_else(|| Error::new("ValueError", "bad marshal data (long size out of range)"))?;
    let (words, _) = data[4..].as_chunks::<2>();
    let mut input = words.iter();
    let mut digits = Vec::new();
    for index in 0..count {
        let digit = u16::from_le_bytes(*input.next().ok_or_else(too_short)?);
        if digit >= 1 << 15 {
            return Err(Error::new(
                "ValueError",
                "bad marshal data (digit out of range in long)",
            ));
        }
        if digit == 0 && index + 1 == count {
            return Err(Error::new(
                "ValueError",
                "bad marshal data (unnormalized long data)",
            ));
        }
        // Each wire digit is exactly three base-32 digits.
        digits
            .try_reserve(3)
            .map_err(|_| Error::new("MemoryError", ""))?;
        digits.extend([
            (digit & 31) as u8,
            ((digit >> 5) & 31) as u8,
            (digit >> 10) as u8,
        ]);
    }
    let sign = if size < 0 { Sign::Minus } else { Sign::Plus };
    Int::try_new(BigInt::from_radix_le(sign, &digits, 32).expect("five-bit digits"))
        .map_err(|_| Error::new("MemoryError", ""))
}

/// Read one scalar from CPython's marshal format and ignore trailing bytes.
pub(super) fn loads(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [input] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "loads() takes exactly 1 positional argument ({} given)",
                args.len()
            ),
        ));
    };
    let mut allow_code = true;
    for (key, value) in kwargs {
        if key != "allow_code" {
            return Err(Error::new(
                "TypeError",
                format!("loads() got an unexpected keyword argument '{key}'"),
            ));
        }
        ctx.bytes_like(*input)?;
        allow_code = ctx.truth_with_buffer_export(*input, *value)?;
    }
    let data = ctx.bytes_like(*input)?;
    let (&tag, data) = data
        .split_first()
        .ok_or_else(|| Error::new("EOFError", "EOF read where object expected"))?;
    // Bit 7 registers the object for later references, which a standalone scalar cannot contain.
    let tag = tag & 0x7f;
    match tag {
        b'N' => Ok(NONE),
        b'F' => Ok(FALSE),
        b'T' => Ok(TRUE),
        b'.' => Ok(ELLIPSIS),
        b'i' => Ok(ctx.int(i64::from(i32::from_le_bytes(read(data)?)))),
        b'I' => Ok(ctx.int(i64::from_le_bytes(read(data)?))),
        b'l' => Ok(ctx.alloc(Value::Int(read_long(data)?))),
        b's' => {
            let size = usize::try_from(i32::from_le_bytes(read(data)?)).map_err(|_| {
                Error::new(
                    "ValueError",
                    "bad marshal data (bytes object size out of range)",
                )
            })?;
            let payload = data[4..].get(..size).ok_or_else(too_short)?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(size)
                .map_err(|_| crate::Error::allocation())?;
            bytes.extend_from_slice(payload);
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        b'0' => Err(Error::new(
            "TypeError",
            "NULL object in marshal data for object",
        )),
        b'r' => {
            read::<4>(data)?;
            Err(Error::new(
                "ValueError",
                "bad marshal data (invalid reference)",
            ))
        }
        b'c' if !allow_code => Err(Error::new(
            "ValueError",
            "unmarshalling code objects is disallowed",
        )),
        b'c' => Err(Error::new(
            "NotImplementedError",
            "native marshal cannot load CPython code objects",
        )),
        // These are valid wire types. Distinguish them from a corrupt tag.
        b'S' | b'f' | b'g' | b'x' | b'y' | b'u' | b't' | b'a' | b'A' | b'z' | b'Z' | b'('
        | b')' | b'[' | b'{' | b'<' | b'>' | b':' => Err(Error::new(
            "NotImplementedError",
            format!(
                "native marshal does not support type code '{}'",
                char::from(tag)
            ),
        )),
        _ => Err(Error::new(
            "ValueError",
            "bad marshal data (unknown type code)",
        )),
    }
}
