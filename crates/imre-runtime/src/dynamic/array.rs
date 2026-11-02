//! Compact machine arrays used by runtime-loaded Python modules.
use std::ffi::{c_long, c_ulong};
use std::rc::Rc;

use num_traits::{Signed, ToPrimitive};

use super::{Context, Error, Id, NONE, Result, Value, builtins};
use crate::Int;

#[derive(Clone, Copy, PartialEq)]
enum Format {
    SignedByte,
    UnsignedByte,
    Short,
    UnsignedShort,
    Int,
    UnsignedInt,
    Long,
    UnsignedLong,
    Unicode,
    Float,
    Double,
    Wide,
}

impl Format {
    fn code(self) -> &'static str {
        match self {
            Self::SignedByte => "b",
            Self::UnsignedByte => "B",
            Self::Short => "h",
            Self::UnsignedShort => "H",
            Self::Int => "i",
            Self::UnsignedInt => "I",
            Self::Long => "l",
            Self::UnsignedLong => "L",
            Self::Unicode => "u",
            Self::Float => "f",
            Self::Double => "d",
            Self::Wide => "w",
        }
    }

    fn width(self) -> usize {
        match self {
            Self::SignedByte | Self::UnsignedByte => 1,
            Self::Short | Self::UnsignedShort => 2,
            Self::Long | Self::UnsignedLong => size_of::<c_long>(),
            Self::Double => 8,
            _ => 4,
        }
    }

    fn unicode(self) -> bool {
        matches!(self, Self::Unicode | Self::Wide)
    }

    fn encode(self, ctx: &mut Context, item: Id) -> Result<[u8; 8]> {
        let mut out = [0; 8];
        match self {
            Self::SignedByte => {
                let long = ctx.index_c_long(item)?;
                // CPython's 'b' setter parses a C short before narrowing to a signed char.
                let short = i16::try_from(long)
                    .map_err(|_| integer_overflow(long, "signed short integer"))?;
                out[0] =
                    i8::try_from(short).map_err(|_| integer_overflow(long, "signed char"))? as u8;
            }
            Self::UnsignedByte => {
                let long = ctx.index_c_long(item)?;
                out[0] = u8::try_from(long)
                    .map_err(|_| integer_overflow(long, "unsigned byte integer"))?;
            }
            Self::Short => {
                let n = ctx.index_c_long(item)?;
                let n =
                    i16::try_from(n).map_err(|_| integer_overflow(n, "signed short integer"))?;
                out[..2].copy_from_slice(&n.to_ne_bytes());
            }
            Self::UnsignedShort => {
                let n = ctx.index_c_int(item)?;
                let n = u16::try_from(n)
                    .map_err(|_| integer_overflow(i64::from(n), "unsigned short"))?;
                out[..2].copy_from_slice(&n.to_ne_bytes());
            }
            Self::Long => {
                let n = c_long::try_from(ctx.index_c_long(item)?).map_err(|_| {
                    Error::new("OverflowError", "Python int too large to convert to C long")
                })?;
                out[..self.width()].copy_from_slice(&n.to_ne_bytes());
            }
            Self::UnsignedInt | Self::UnsignedLong => {
                let n = ctx.index_integer(item)?;
                if n.big().is_negative() {
                    return Err(Error::new(
                        "OverflowError",
                        "can't convert negative value to unsigned int",
                    ));
                }
                let n = n
                    .big()
                    .to_u64()
                    .and_then(|n| c_ulong::try_from(n).ok())
                    .ok_or_else(|| {
                        Error::new(
                            "OverflowError",
                            "Python int too large to convert to C unsigned long",
                        )
                    })?;
                if self == Self::UnsignedInt {
                    let n = u32::try_from(n).map_err(|_| {
                        Error::new("OverflowError", "unsigned int is greater than maximum")
                    })?;
                    out[..4].copy_from_slice(&n.to_ne_bytes());
                } else {
                    out[..self.width()].copy_from_slice(&n.to_ne_bytes());
                }
            }
            Self::Int => {
                let n = ctx.index_c_int(item)?;
                out[..4].copy_from_slice(&n.to_ne_bytes());
            }
            Self::Float | Self::Double => {
                let number = builtins::from_numeric(ctx, item)?.ok_or_else(|| {
                    Error::new(
                        "TypeError",
                        format!("must be real number, not {}", ctx.type_name(item)),
                    )
                })?;
                let Value::Float(x) = *ctx.heap.get(ctx.scalar_value(number)) else {
                    unreachable!("float conversion returned a non-float")
                };
                if self == Self::Float {
                    out[..4].copy_from_slice(&(x as f32).to_ne_bytes());
                } else {
                    out = x.to_ne_bytes();
                }
            }
            Self::Wide | Self::Unicode => {
                let (ch, len) = match ctx.heap.get(ctx.scalar_value(item)) {
                    Value::Str(s) => (s.chars().next().map(u32::from), s.chars().count()),
                    Value::SurrogateStr(s) => (s.first().copied(), s.len()),
                    _ => {
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "array item must be a unicode character, not {}",
                                ctx.type_name(item)
                            ),
                        ));
                    }
                };
                let ch = ch.filter(|_| len == 1).ok_or_else(|| {
                    Error::new(
                        "TypeError",
                        format!(
                            "array item must be a unicode character, not a string of length {len}"
                        ),
                    )
                })?;
                out[..4].copy_from_slice(&ch.to_ne_bytes());
            }
        }
        Ok(out)
    }
}

fn integer_overflow(value: i64, kind: &str) -> Error {
    let bound = if value < 0 {
        "less than minimum"
    } else {
        "greater than maximum"
    };
    Error::new("OverflowError", format!("{kind} is {bound}"))
}

#[derive(Clone)]
pub(super) struct Array {
    format: Format,
    bytes: Rc<Vec<u8>>,
}

impl Array {
    /// Borrow the contiguous native-order bytes used to store the elements.
    pub(super) fn as_bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    /// Detach a copied array before mutating its fixed-size native representation.
    pub(super) fn as_bytes_mut(&mut self) -> Result<&mut [u8]> {
        Ok(self.writable_bytes(0)?.as_mut_slice())
    }

    pub(super) fn view_layout(&self) -> (&'static str, usize) {
        (
            if self.format.unicode() {
                "w"
            } else {
                self.format.code()
            },
            self.format.width(),
        )
    }

    pub(super) fn len(&self) -> usize {
        self.bytes.len() / self.format.width()
    }

    pub(super) fn at(&self, index: usize) -> Result<Value> {
        let width = self.format.width();
        let element = &self.bytes[width * index..width * (index + 1)];
        Ok(match self.format {
            Format::SignedByte => Value::Int(Int::from(i64::from(element[0] as i8))),
            Format::UnsignedByte => Value::Int(Int::from(i64::from(element[0]))),
            Format::Short => Value::Int(Int::from(i64::from(i16::from_ne_bytes(
                element.try_into().unwrap(),
            )))),
            Format::UnsignedShort => Value::Int(Int::from(i64::from(u16::from_ne_bytes(
                element.try_into().unwrap(),
            )))),
            Format::UnsignedInt => Value::Int(Int::from(i64::from(u32::from_ne_bytes(
                element.try_into().unwrap(),
            )))),
            Format::Long => Value::Int(Int::from(
                c_long::from_ne_bytes(element.try_into().unwrap()) as i64,
            )),
            Format::UnsignedLong => Value::Int(Int::new(
                c_ulong::from_ne_bytes(element.try_into().unwrap()).into(),
            )),
            Format::Int => Value::Int(Int::from(i64::from(i32::from_ne_bytes(
                element.try_into().unwrap(),
            )))),
            Format::Float => {
                Value::Float(f64::from(f32::from_ne_bytes(element.try_into().unwrap())))
            }
            Format::Double => Value::Float(f64::from_ne_bytes(element.try_into().unwrap())),
            Format::Wide | Format::Unicode => {
                let ch = u32::from_ne_bytes(element.try_into().unwrap());
                if (0xd800..=0xdfff).contains(&ch) {
                    return Ok(Value::SurrogateStr(vec![ch]));
                }
                Value::Str(
                    char::from_u32(ch)
                        .ok_or_else(|| {
                            Error::new("ValueError", "chr() arg not in range(0x110000)")
                        })?
                        .to_string(),
                )
            }
        })
    }

    fn writable_bytes(&mut self, additional: usize) -> Result<&mut Vec<u8>> {
        if Rc::get_mut(&mut self.bytes).is_none() {
            let mut copy = Vec::new();
            copy.try_reserve_exact(self.bytes.len())
                .map_err(|_| Error::new("MemoryError", ""))?;
            copy.extend_from_slice(&self.bytes);
            self.bytes = Rc::new(copy);
        }
        let data = Rc::get_mut(&mut self.bytes).unwrap();
        data.try_reserve(additional)
            .map_err(|_| Error::new("MemoryError", ""))?;
        Ok(data)
    }

    fn append_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.writable_bytes(bytes.len())?.extend_from_slice(bytes);
        Ok(())
    }
}

fn array_mut(ctx: &mut Context, receiver: Id) -> &mut Array {
    let Value::Array(array) = ctx.heap.get_mut(receiver) else {
        unreachable!("expected array receiver");
    };
    array
}

/// CPython validates the item, resizes at the original append offset, and converts
/// again to store it. Both conversions can run Python that changes or copies the array.
fn append_item(ctx: &mut Context, receiver: Id, item: Id) -> Result<()> {
    let array = array_mut(ctx, receiver);
    let format = array.format;
    let offset = array.bytes.len();
    format.encode(ctx, item)?;
    let end = offset
        .checked_add(format.width())
        .ok_or_else(|| Error::new("MemoryError", ""))?;
    // __index__ may already have grown the array to the requested size.
    if array_mut(ctx, receiver).bytes.len() != end && ctx.buffer_is_exported(receiver)? {
        return Err(Error::new(
            "BufferError",
            "cannot resize an array that is exporting buffers",
        ));
    }
    let array = array_mut(ctx, receiver);
    let additional = end.saturating_sub(array.bytes.len());
    array.writable_bytes(additional)?.resize(end, 0);
    let value = format.encode(ctx, item)?;
    let slot = array_mut(ctx, receiver)
        .writable_bytes(0)?
        .get_mut(offset..end)
        .ok_or_else(|| Error::new("IndexError", "array assignment index out of range"))?;
    slot.copy_from_slice(&value[..format.width()]);
    Ok(())
}

pub(super) fn new(ctx: &mut Context, args: &[Id], kw: &[(String, Id)]) -> Result<Id> {
    if !kw.is_empty() {
        return Err(Error::new(
            "TypeError",
            "array.array() takes no keyword arguments",
        ));
    }
    let [code, rest @ ..] = args else {
        return Err(Error::new("TypeError", "array.array() requires a typecode"));
    };
    if rest.len() > 1 {
        return Err(Error::new(
            "TypeError",
            "array.array() takes at most 2 arguments",
        ));
    }
    let code = ctx.str_value(*code)?;
    let format = match code {
        "b" => Format::SignedByte,
        "B" => Format::UnsignedByte,
        "h" => Format::Short,
        "H" => Format::UnsignedShort,
        "i" => Format::Int,
        "I" => Format::UnsignedInt,
        "l" => Format::Long,
        "L" => Format::UnsignedLong,
        // Imre's supported Linux and macOS targets store wchar_t as a 32-bit code point.
        #[cfg(unix)]
        "u" if size_of::<libc::wchar_t>() == 4 => Format::Unicode,
        "f" => Format::Float,
        "d" => Format::Double,
        "w" => Format::Wide,
        "u" | "q" | "Q" => {
            return Err(Error::new(
                "NotImplementedError",
                format!("unsupported array typecode '{code}'"),
            ));
        }
        _ if code.chars().count() != 1 => {
            return Err(Error::new(
                "TypeError",
                "array() argument 1 must be a unicode character",
            ));
        }
        _ => {
            return Err(Error::new(
                "ValueError",
                "bad typecode (must be b, B, u, w, h, H, i, I, l, L, q, Q, f or d)",
            ));
        }
    };
    if format == Format::Unicode {
        ctx.emit_builtin_warning(
            crate::objects::exceptions::Kind::DeprecationWarning,
            "The 'u' type code is deprecated and will be removed in Python 3.16",
            1,
        )?;
    }
    let mut array = Array {
        format,
        bytes: Rc::new(Vec::new()),
    };
    if let [initial] = rest {
        match ctx.heap.get(ctx.scalar_value(*initial)) {
            Value::Bytes(data) | Value::ByteArray(data) => {
                if data.len() % format.width() != 0 {
                    return Err(Error::new(
                        "ValueError",
                        "bytes length not a multiple of item size",
                    ));
                }
                array.append_bytes(data)?;
            }
            Value::Array(other) if other.format == format => array = other.clone(),
            Value::Array(other) if format.unicode() && other.format.unicode() => {
                array.bytes = Rc::clone(&other.bytes);
            }
            Value::Array(other) if other.format.unicode() => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "cannot use a unicode array to initialize an array with typecode '{}'",
                        format.code()
                    ),
                ));
            }
            Value::Str(text) if format.unicode() => {
                for ch in text.chars() {
                    array.append_bytes(&(ch as u32).to_ne_bytes())?;
                }
            }
            Value::SurrogateStr(points) if format.unicode() => {
                for ch in points {
                    array.append_bytes(&ch.to_ne_bytes())?;
                }
            }
            Value::Str(_) | Value::SurrogateStr(_) => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "cannot use a str to initialize an array with typecode '{}'",
                        format.code()
                    ),
                ));
            }
            Value::List(elements) | Value::Tuple(elements) => {
                let count = elements.len();
                let mark = ctx.heap.temporary_mark();
                let mut limit = ctx.heap.streaming_collection_limit();
                for index in 0..count {
                    let index = ctx.int(index as i64);
                    let item = ctx.get_item(*initial, index)?;
                    let bytes = format.encode(ctx, item)?;
                    array.append_bytes(&bytes[..format.width()])?;
                    ctx.heap.release_temporaries_since(mark);
                    ctx.collect_streaming_garbage(&mut limit);
                }
            }
            _ => {
                let iterator = ctx.iterator(*initial)?;
                ctx.heap.keep(iterator);
                let receiver = ctx.alloc(Value::Array(array));
                let mark = ctx.heap.temporary_mark();
                let mut limit = ctx.heap.streaming_collection_limit();
                while let Some(item) = ctx.next_item(iterator)? {
                    append_item(ctx, receiver, item)?;
                    ctx.heap.release_temporaries_since(mark);
                    ctx.collect_streaming_garbage(&mut limit);
                }
                return Ok(receiver);
            }
        }
    }
    Ok(ctx.alloc(Value::Array(array)))
}

pub(super) fn attribute(ctx: &mut Context, id: Id, name: &str) -> Option<Id> {
    let Value::Array(array) = ctx.heap.get(id) else {
        return None;
    };
    match name {
        "typecode" => Some(ctx.text(array.format.code())),
        "itemsize" => Some(ctx.int(array.format.width() as i64)),
        "tobytes" | "tolist" | "append" => {
            let callable = ctx.builtin(&format!("array.array.{name}"));
            Some(ctx.alloc(Value::Method {
                callable,
                receiver: id,
                explicit: false,
            }))
        }
        _ => None,
    }
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, args)) = args.split_first() else {
        return Err(Error::new("TypeError", "expected array receiver"));
    };
    let Value::Array(array) = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected array receiver"));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "array method takes no keyword arguments",
        ));
    }
    match (name, args) {
        ("tobytes", []) => {
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(array.bytes.len())
                .map_err(|_| Error::new("MemoryError", ""))?;
            bytes.extend_from_slice(&array.bytes);
            Ok(ctx.alloc(Value::Bytes(bytes)))
        }
        ("tolist", []) => {
            let items = ctx.items(receiver)?;
            Ok(ctx.alloc(Value::List(items)))
        }
        ("append", &[item]) => {
            append_item(ctx, receiver, item)?;
            Ok(NONE)
        }
        _ => Err(Error::new("TypeError", "invalid arguments to array method")),
    }
}
