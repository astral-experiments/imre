//! CPython marshal data records. Container children remain in the owning Python heap.

use std::collections::HashMap;

use super::{Context, Error, Id, MAX_DEPTH, Result, Value};
use crate::Int;

pub(in crate::dynamic::stdlib) fn dumps(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if args.len() + kwargs.len() > 3 {
        return Err(Error::new(
            "TypeError",
            format!(
                "dumps() takes at most 3 arguments ({} given)",
                args.len() + kwargs.len()
            ),
        ));
    }
    let Some((&object, options)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "dumps() takes at least 1 positional argument (0 given)",
        ));
    };
    if options.len() > 1 {
        return Err(Error::new(
            "TypeError",
            format!(
                "dumps() takes at most 2 positional arguments ({} given)",
                args.len()
            ),
        ));
    }
    let mut allow_code = None;
    for (key, value) in kwargs {
        if key != "allow_code" {
            return Err(Error::new(
                "TypeError",
                format!("dumps() got an unexpected keyword argument '{key}'"),
            ));
        }
        allow_code = Some(*value);
    }
    let version = match options.first() {
        Some(version) => super::super::os::c_int(ctx, *version)?,
        None => imre_ast::MARSHAL_VERSION,
    };
    let allow_code = match allow_code {
        Some(flag) => ctx.truth(flag)?,
        None => true,
    };
    let bytes = Writer {
        version,
        allow_code,
        refs: HashMap::new(),
        bytes: Vec::new(),
    }
    .encode(ctx, object)?;
    Ok(ctx.alloc(Value::Bytes(bytes)))
}

struct Writer {
    version: i32,
    allow_code: bool,
    refs: HashMap<Id, i32>,
    bytes: Vec<u8>,
}

fn too_large() -> Error {
    Error::new("ValueError", "object too large to marshal")
}

impl Writer {
    fn encode(mut self, ctx: &mut Context, root: Id) -> Result<Vec<u8>> {
        let mut parents = Vec::new();
        let mut object = root;
        loop {
            if parents.len() >= MAX_DEPTH {
                return Err(Error::new(
                    "ValueError",
                    "object too deeply nested to marshal",
                ));
            }
            if self.record(ctx, object, &parents)? {
                parents
                    .try_reserve(1)
                    .map_err(|_| crate::Error::allocation())?;
                parents.push((object, 0usize));
            }
            object = loop {
                let Some((parent, index)) = parents.last_mut() else {
                    return Ok(self.bytes);
                };
                let child = match ctx.heap.get(*parent) {
                    Value::List(items) | Value::Tuple(items) => items.get(*index).copied(),
                    Value::Slice(items) => items.get(*index).copied(),
                    Value::Dict(entries) => match entries.get(*index / 2) {
                        Some(&(key, value)) => Some(if *index % 2 == 0 { key } else { value }),
                        None => {
                            self.append(b"0")?;
                            None
                        }
                    },
                    _ => unreachable!("only containers have marshal children"),
                };
                if let Some(child) = child {
                    *index += 1;
                    break child;
                }
                parents.pop();
            }
        }
    }

    fn append(&mut self, data: &[u8]) -> Result<()> {
        self.bytes
            .try_reserve(data.len())
            .map_err(|_| crate::Error::allocation())?;
        self.bytes.extend_from_slice(data);
        Ok(())
    }

    fn count(&mut self, count: usize) -> Result<()> {
        self.append(&i32::try_from(count).map_err(|_| too_large())?.to_le_bytes())
    }

    /// Returns true when a newly written header must be followed by its children.
    fn record(&mut self, ctx: &mut Context, id: Id, parents: &[(Id, usize)]) -> Result<bool> {
        let value = ctx.heap.get(id);
        let singleton = match value {
            Value::None => Some(b'N'),
            Value::Bool(true) => Some(b'T'),
            Value::Bool(false) => Some(b'F'),
            Value::Ellipsis => Some(b'.'),
            Value::Native(name) if name == "StopIteration" => Some(b'S'),
            _ => None,
        };
        if let Some(tag) = singleton {
            self.append(&[tag])?;
            return Ok(false);
        }
        // Versions 3+ permit a reference slot for any non-singleton, including
        // an unfinished container. The flag is independent of the object's type.
        let flag = if self.version >= 3 {
            if let Some(index) = self.refs.get(&id).copied() {
                // A slice is constructed only after all three fields have been read.
                if matches!(value, Value::Slice(_))
                    && parents.iter().any(|(parent, _)| *parent == id)
                {
                    return Err(Error::new("ValueError", "unmarshallable object"));
                }
                self.append(b"r")?;
                self.append(&index.to_le_bytes())?;
                return Ok(false);
            }
            let index = i32::try_from(self.refs.len()).map_err(|_| too_large())?;
            self.refs
                .try_reserve(1)
                .map_err(|_| crate::Error::allocation())?;
            self.refs.insert(id, index);
            0x80
        } else {
            0
        };
        match value {
            Value::Int(value) => self.integer(value, flag)?,
            Value::Float(f) => {
                self.append(&[flag | if self.version >= 2 { b'g' } else { b'f' }])?;
                self.float(*f)?;
            }
            Value::Complex(z) => {
                self.append(&[flag | if self.version >= 2 { b'y' } else { b'x' }])?;
                self.float(z.real)?;
                self.float(z.imag)?;
            }
            Value::Str(text) => self.string(text.as_bytes(), ctx.heap.is_interned(id), flag)?,
            Value::SurrogateStr(_) => {
                let text = ctx.encode_utf8_codepoints(id, false, "surrogatepass")?;
                self.string(&text, false, flag)?;
            }
            Value::Bytes(data) | Value::ByteArray(data) => self.blob(data, flag)?,
            Value::MemoryView { .. } | Value::Array(_) => {
                let data = ctx.read_buffer_bytes(id).map_err(|err| {
                    if matches!(err.kind.as_ref(), "BufferError" | "ValueError") {
                        Error::new("ValueError", "unmarshallable object")
                    } else {
                        err
                    }
                })?;
                self.blob(&data, flag)?;
            }
            Value::Tuple(items) => {
                if self.version >= 4
                    && let Ok(n) = u8::try_from(items.len())
                {
                    self.append(&[b')' | flag, n])?;
                } else {
                    self.append(&[b'(' | flag])?;
                    self.count(items.len())?;
                }
                return Ok(true);
            }
            Value::List(items) => {
                self.append(&[b'[' | flag])?;
                self.count(items.len())?;
                return Ok(true);
            }
            Value::Dict(_) => {
                self.append(&[b'{' | flag])?;
                return Ok(true);
            }
            Value::Slice(_) if self.version >= 5 => {
                self.append(&[b':' | flag])?;
                return Ok(true);
            }
            Value::Code(_) if !self.allow_code => {
                return Err(Error::new(
                    "ValueError",
                    "marshalling code objects is disallowed",
                ));
            }
            Value::Code(_) => {
                return Err(Error::new(
                    "NotImplementedError",
                    "native marshal cannot dump CPython code objects",
                ));
            }
            Value::Set { .. } => {
                return Err(Error::new(
                    "NotImplementedError",
                    "native marshal cannot dump sets",
                ));
            }
            _ => return Err(Error::new("ValueError", "unmarshallable object")),
        }
        Ok(false)
    }

    fn blob(&mut self, data: &[u8], flag: u8) -> Result<()> {
        self.append(&[b's' | flag])?;
        self.count(data.len())?;
        self.append(data)
    }

    fn string(&mut self, data: &[u8], interned: bool, flag: u8) -> Result<()> {
        if self.version >= 4 && data.is_ascii() {
            if let Ok(n) = u8::try_from(data.len()) {
                self.append(&[flag | if interned { b'Z' } else { b'z' }, n])?;
            } else {
                self.append(&[flag | if interned { b'A' } else { b'a' }])?;
                self.count(data.len())?;
            }
        } else {
            let tag = if self.version >= 3 && interned {
                b't'
            } else {
                b'u'
            };
            self.append(&[flag | tag])?;
            self.count(data.len())?;
        }
        self.append(data)
    }

    fn integer(&mut self, value: &Int, flag: u8) -> Result<()> {
        if let Some(n) = value.to_i64().and_then(|n| i32::try_from(n).ok()) {
            self.append(&[b'i' | flag])?;
            return self.append(&n.to_le_bytes());
        }
        let big = value.big();
        let count = i32::try_from(big.bits().div_ceil(15)).map_err(|_| too_large())?;
        let signed = if big.sign() == num_bigint::Sign::Minus {
            -count
        } else {
            count
        };
        self.append(&[b'l' | flag])?;
        self.append(&signed.to_le_bytes())?;
        let mut limbs = big.magnitude().iter_u32_digits();
        let mut pending = 0u64;
        let mut bits = 0;
        for _ in 0..count {
            if bits < 15 {
                pending |= u64::from(limbs.next().unwrap_or(0)) << bits;
                bits += 32;
            }
            self.append(&((pending & 0x7fff) as u16).to_le_bytes())?;
            pending >>= 15;
            bits -= 15;
        }
        Ok(())
    }

    fn float(&mut self, value: f64) -> Result<()> {
        if self.version >= 2 {
            return self.append(&value.to_le_bytes());
        }
        // The original text format uses the C locale and 17 significant digits.
        let text = crate::objects::percent_float_field(".17g", std::iter::empty(), value)?;
        self.append(&[u8::try_from(text.len()).map_err(|_| too_large())?])?;
        self.append(text.as_bytes())
    }
}
