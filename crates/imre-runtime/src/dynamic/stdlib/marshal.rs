mod writer;

pub(super) use writer::dumps;

use std::collections::{HashMap, HashSet};

use num_bigint::{BigInt, Sign};

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};
use crate::Int;
use crate::dynamic::{complex::Complex, value::ELLIPSIS};
use crate::objects::Codec;

const MAX_DEPTH: usize = 2000;

fn read<const N: usize>(data: &[u8]) -> Result<[u8; N]> {
    data.first_chunk().copied().ok_or_else(too_short)
}

fn too_short() -> Error {
    Error::new("EOFError", "marshal data too short")
}

/// Decode a signed count of little-endian 15-bit digits.
fn read_long(data: &[u8]) -> Result<(Int, usize)> {
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
    let number = Int::try_new(BigInt::from_radix_le(sign, &digits, 32).expect("five-bit digits"))
        .map_err(|_| crate::Error::allocation())?;
    Ok((number, 4 + 2 * count as usize))
}

/// Read one object and leave trailing bytes untouched.
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
    ctx.bytes_like(*input)?;
    ctx.with_buffer_export(*input, |ctx| {
        Reader {
            input: *input,
            offset: 0,
            references: Vec::new(),
            open_tuples: HashMap::new(),
            allow_code,
        }
        .object(ctx, 0)?
        .ok_or_else(|| null_object("object"))
    })
}

fn null_object(kind: &str) -> Error {
    Error::new(
        "TypeError",
        format!("NULL object in marshal data for {kind}"),
    )
}

fn invalid_reference() -> Error {
    Error::new("ValueError", "bad marshal data (invalid reference)")
}

fn copy_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(data.len())
        .map_err(|_| crate::Error::allocation())?;
    bytes.extend_from_slice(data);
    Ok(bytes)
}

struct Reader {
    input: Id,
    offset: usize,
    references: Vec<Option<Id>>,
    // A mutable container can refer to a parent tuple before its fields are complete.
    open_tuples: HashMap<Id, bool>,
    allow_code: bool,
}

impl Reader {
    fn take<'a>(&mut self, ctx: &'a Context, size: usize) -> Result<&'a [u8]> {
        let end = self.offset.checked_add(size).ok_or_else(too_short)?;
        let bytes = ctx
            .bytes_like(self.input)?
            .get(self.offset..end)
            .ok_or_else(too_short)?;
        self.offset = end;
        Ok(bytes)
    }

    fn byte(&mut self, ctx: &Context) -> Result<u8> {
        Ok(self.take(ctx, 1)?[0])
    }

    fn size(&mut self, ctx: &Context, kind: &str) -> Result<usize> {
        let value = i32::from_le_bytes(read(self.take(ctx, 4)?)?);
        usize::try_from(value).map_err(|_| {
            Error::new(
                "ValueError",
                format!("bad marshal data ({kind} size out of range)"),
            )
        })
    }

    fn reference(&mut self, value: Option<Id>) -> Result<usize> {
        self.references
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        let slot = self.references.len();
        self.references.push(value);
        Ok(slot)
    }

    fn float(&mut self, ctx: &Context, binary: bool) -> Result<f64> {
        if binary {
            return Ok(f64::from_le_bytes(read(self.take(ctx, 8)?)?));
        }
        let len = usize::from(self.byte(ctx)?);
        let bytes = self.take(ctx, len)?;
        let text = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
        std::str::from_utf8(text)
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .ok_or_else(|| {
                Error::new(
                    "ValueError",
                    format!(
                        "could not convert string to float: '{}'",
                        String::from_utf8_lossy(text)
                    ),
                )
            })
    }

    fn string(&mut self, ctx: &mut Context, tag: u8) -> Result<Id> {
        let size = if matches!(tag, b'z' | b'Z') {
            usize::from(self.byte(ctx)?)
        } else {
            self.size(ctx, "string")?
        };
        let bytes = self.take(ctx, size)?;
        let text = if matches!(tag, b'a' | b'A' | b'z' | b'Z') {
            let value = Codec::Latin1.decode(bytes, None)?;
            ctx.text(value)
        } else if let Ok(value) = std::str::from_utf8(bytes) {
            let text = crate::Str::try_new(value).map_err(|_| crate::Error::allocation())?;
            ctx.text(text)
        } else {
            let data = copy_bytes(bytes)?;
            let data = ctx.alloc(Value::Bytes(data));
            let policy = ctx.text("surrogatepass");
            super::codecs::decode_bytes(ctx, Codec::Utf8 { signature: false }, data, Some(policy))?
        };
        if matches!(tag, b't' | b'A' | b'Z') && matches!(ctx.heap.get(text), Value::Str(_)) {
            let (heap, roots) = ctx.heap_and_roots();
            return Ok(heap.intern(text, roots));
        }
        Ok(text)
    }

    fn object(&mut self, ctx: &mut Context, depth: usize) -> Result<Option<Id>> {
        if depth >= MAX_DEPTH {
            return Err(Error::new("ValueError", "recursion limit exceeded"));
        }
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || self.read_object(ctx, depth))
    }

    fn read_object(&mut self, ctx: &mut Context, depth: usize) -> Result<Option<Id>> {
        let tag = self
            .byte(ctx)
            .map_err(|_| Error::new("EOFError", "EOF read where object expected"))?;
        let flagged = tag & 0x80 != 0;
        let tag = tag & 0x7f;
        let value = match tag {
            b'0' => return Ok(None),
            b'N' => return Ok(Some(NONE)),
            b'F' => return Ok(Some(FALSE)),
            b'T' => return Ok(Some(TRUE)),
            b'.' => return Ok(Some(ELLIPSIS)),
            b'S' => return Ok(Some(ctx.builtin("StopIteration"))),
            b'r' => {
                let index = i32::from_le_bytes(read(self.take(ctx, 4)?)?);
                let id = usize::try_from(index)
                    .ok()
                    .and_then(|i| self.references.get(i))
                    .copied()
                    .flatten()
                    .ok_or_else(invalid_reference)?;
                if let Some(used) = self.open_tuples.get_mut(&id) {
                    *used = true;
                }
                return Ok(Some(id));
            }
            b'i' => ctx.int(i64::from(i32::from_le_bytes(read(self.take(ctx, 4)?)?))),
            b'I' => ctx.int(i64::from_le_bytes(read(self.take(ctx, 8)?)?)),
            b'l' => {
                let data = ctx
                    .bytes_like(self.input)?
                    .get(self.offset..)
                    .ok_or_else(too_short)?;
                let (integer, used) = read_long(data)?;
                self.offset += used;
                ctx.alloc(Value::Int(integer))
            }
            b'f' | b'g' => {
                let number = self.float(ctx, tag == b'g')?;
                ctx.alloc(Value::Float(number))
            }
            b'x' | b'y' => {
                let real = self.float(ctx, tag == b'y')?;
                let imag = self.float(ctx, tag == b'y')?;
                ctx.alloc(Value::Complex(Complex { real, imag }))
            }
            b's' => {
                let size = self.size(ctx, "bytes object")?;
                let bytes = copy_bytes(self.take(ctx, size)?)?;
                ctx.alloc(Value::Bytes(bytes))
            }
            b'u' | b't' | b'a' | b'A' | b'z' | b'Z' => self.string(ctx, tag)?,
            b'(' | b')' | b'[' => return self.sequence(ctx, tag, flagged, depth).map(Some),
            b'{' => return self.mapping(ctx, flagged, depth).map(Some),
            b':' => {
                let slot = flagged.then(|| self.reference(None)).transpose()?;
                let mut fields = [NONE; 3];
                for field in &mut fields {
                    *field = self
                        .object(ctx, depth + 1)?
                        .ok_or_else(|| null_object("object"))?;
                }
                let id = ctx.alloc(Value::Slice(fields));
                if let Some(slot) = slot {
                    self.references[slot] = Some(id);
                }
                return Ok(Some(id));
            }
            b'c' if !self.allow_code => {
                return Err(Error::new(
                    "ValueError",
                    "unmarshalling code objects is disallowed",
                ));
            }
            b'c' => {
                return Err(Error::new(
                    "NotImplementedError",
                    "native marshal cannot load CPython code objects",
                ));
            }
            b'<' | b'>' => {
                return Err(Error::new(
                    "NotImplementedError",
                    format!(
                        "native marshal does not support type code '{}'",
                        char::from(tag)
                    ),
                ));
            }
            _ => {
                return Err(Error::new(
                    "ValueError",
                    "bad marshal data (unknown type code)",
                ));
            }
        };
        if flagged {
            self.reference(Some(value))?;
        }
        Ok(Some(value))
    }

    fn sequence(&mut self, ctx: &mut Context, tag: u8, flagged: bool, depth: usize) -> Result<Id> {
        let tuple = tag != b'[';
        let kind = if tuple { "tuple" } else { "list" };
        let size = if tag == b')' {
            usize::from(self.byte(ctx)?)
        } else {
            self.size(ctx, kind)?
        };
        // Heap::alloc interns the empty tuple. Use a unique shell for nonempty tuples.
        let id = ctx.alloc(if tuple && size != 0 {
            Value::Tuple(vec![NONE])
        } else if tuple {
            Value::Tuple(Vec::new())
        } else {
            Value::List(Vec::new())
        });
        if flagged {
            self.reference(Some(id))?;
        }
        if size == 0 {
            return Ok(id);
        }
        if tuple && flagged {
            self.open_tuples
                .try_reserve(1)
                .map_err(|_| crate::Error::allocation())?;
            self.open_tuples.insert(id, false);
        }
        for index in 0..size {
            let item = self
                .object(ctx, depth + 1)?
                .ok_or_else(|| null_object(kind))?;
            let (Value::Tuple(items) | Value::List(items)) = ctx.heap.get_mut(id) else {
                unreachable!("sequence shell")
            };
            if tuple && index == 0 {
                items[0] = item;
            } else {
                items
                    .try_reserve(1)
                    .map_err(|_| crate::Error::allocation())?;
                items.push(item);
            }
        }
        if tuple && flagged && self.open_tuples.remove(&id) == Some(true) {
            let Value::Tuple(items) = ctx.heap.get(id) else {
                unreachable!()
            };
            // Ordinary tuple cycles pass through a mutable container. An immutable-only
            // cycle in a fabricated stream would never terminate in the tuple hash.
            if immutable_reaches(ctx, items, |other| other == id)? {
                return Err(invalid_reference());
            }
        }
        Ok(id)
    }

    fn mapping(&mut self, ctx: &mut Context, flagged: bool, depth: usize) -> Result<Id> {
        let id = ctx.alloc(Value::Dict(Default::default()));
        if flagged {
            self.reference(Some(id))?;
        }
        while let Some(key) = self.object(ctx, depth + 1)? {
            let Some(value) = self.object(ctx, depth + 1)? else {
                break;
            };
            if matches!(ctx.heap.get(key), Value::Tuple(_) | Value::Slice(_))
                && self.open_tuples.values().any(|&used| used)
                && immutable_reaches(ctx, &[key], |other| self.open_tuples.contains_key(&other))?
            {
                return Err(invalid_reference());
            }
            ctx.set_dictionary_key(id, key, value, None, true)?;
        }
        Ok(id)
    }
}

/// Follow the immutable edges that Python would hash for a mapping key.
fn immutable_reaches(ctx: &Context, roots: &[Id], target: impl Fn(Id) -> bool) -> Result<bool> {
    let mut todo = Vec::new();
    todo.try_reserve(roots.len())
        .map_err(|_| crate::Error::allocation())?;
    todo.extend_from_slice(roots);
    let mut visited = HashSet::new();
    while let Some(id) = todo.pop() {
        if target(id) {
            return Ok(true);
        }
        let children: &[Id] = match ctx.heap.get(id) {
            Value::Tuple(items) => items,
            Value::Slice(fields) => fields,
            _ => continue,
        };
        visited
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        if visited.insert(id) {
            todo.try_reserve(children.len())
                .map_err(|_| crate::Error::allocation())?;
            todo.extend_from_slice(children);
        }
    }
    Ok(false)
}
