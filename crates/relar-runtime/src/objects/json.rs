//! Schema-directed JSON decoding into existing acyclic native storage.
//!
//! The compiler proves structural constructors and supplies immutable metadata.
//! Every partial record and array owns its initialized words, so an error never
//! publishes incomplete values or leaks earlier fields. Unknown fields are
//! validated and skipped without building an intermediate object tree.

use std::borrow::Cow;
use std::fmt;
use std::rc::Rc;

use serde::de::{
    Deserialize, DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor,
};
use serde_json::value::RawValue;

use super::records::{RecordDescriptor, relar_record_new};
use super::words::{OwnedWord, WordKind, Words};
use super::{Error, Int, Kind, Object, Storage, StorageCell, Str, finish};

const INTEGER: u32 = 1;
const STRING: u32 = 2;
const BOOLEAN: u32 = 3;
const FLOAT: u32 = 4;
const NONE: u32 = 5;
const LIST: u32 = 6;
const RECORD: u32 = 7;
const OPTIONAL: u32 = 8;
const MAX_DEPTH: usize = 128;

/// Compiler-owned metadata. `kind` selects JSON syntax; `word_kind` selects the
/// independently checked native ownership representation.
#[repr(C)]
pub struct Schema {
    pub kind: u32,
    pub word_kind: u32,
    pub element: *const Schema,
    pub record: *const RecordDescriptor,
    pub field_count: usize,
    pub fields: *const Field,
}

#[repr(C)]
pub struct Field {
    pub name: *const u8,
    pub name_len: usize,
    pub schema: *const Schema,
    /// Zero requires the field; one decodes a scalar JSON default; two makes [].
    pub default_kind: u32,
    pub default_json: *const u8,
    pub default_len: usize,
}

impl Schema {
    fn storage(&self) -> WordKind {
        WordKind::new(self.word_kind)
    }

    fn child(&self) -> &Self {
        // SAFETY: list/optional compiler metadata contains a live child schema.
        unsafe { &*self.element }
    }

    fn fields(&self) -> &[Field] {
        if self.field_count == 0 {
            return &[];
        }
        // SAFETY: record metadata declares this many immutable field entries.
        unsafe { std::slice::from_raw_parts(self.fields, self.field_count) }
    }
}

impl Field {
    fn name(&self) -> &str {
        // SAFETY: emitted names are valid UTF-8 with this exact byte length.
        unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.name, self.name_len))
        }
    }

    fn schema(&self) -> &Schema {
        // SAFETY: every emitted field has one immutable live type descriptor.
        unsafe { &*self.schema }
    }

    fn missing<E: serde::de::Error>(&self) -> Result<OwnedWord, E> {
        match self.default_kind {
            0 => Err(E::custom(format!("missing field {:?}", self.name()))),
            1 => {
                // SAFETY: compiler literals are complete UTF-8 JSON fragments.
                let text = unsafe {
                    std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                        self.default_json,
                        self.default_len,
                    ))
                };
                decode(text, self.schema()).map_err(E::custom)
            }
            2 => Ok(empty_list(self.schema())),
            _ => unreachable!("compiler emitted an invalid JSON default"),
        }
    }
}

/// Common small records use a stack bitset; unusually wide schemas remain valid.
struct Seen {
    inline: u128,
    extra: Vec<u64>,
}

impl Seen {
    fn new(count: usize) -> Self {
        Self {
            inline: 0,
            extra: vec![0; count.saturating_sub(128).div_ceil(64)],
        }
    }

    fn insert(&mut self, index: usize) -> bool {
        if index < 128 {
            let bit = 1_u128 << index;
            let fresh = self.inline & bit == 0;
            self.inline |= bit;
            fresh
        } else {
            let index = index - 128;
            let bit = 1_u64 << (index % 64);
            let word = &mut self.extra[index / 64];
            let fresh = *word & bit == 0;
            *word |= bit;
            fresh
        }
    }
}

#[derive(Clone, Copy)]
struct Seed<'a>(&'a Schema);

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = OwnedWord;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<OwnedWord, D::Error> {
        let schema = self.0;
        let word = match schema.kind {
            INTEGER => {
                // Borrowing the validated token preserves arbitrary precision and
                // distinguishes integer syntax from booleans, fractions and exponents.
                let raw = <&RawValue>::deserialize(deserializer)?.get();
                let digits = raw.strip_prefix('-').unwrap_or(raw);
                if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(D::Error::custom("expected an integer"));
                }
                let value = raw
                    .parse::<i64>()
                    .ok()
                    .and_then(Int::small)
                    .unwrap_or_else(|| {
                        Int::new(
                            num_bigint::BigInt::parse_bytes(raw.as_bytes(), 10)
                                .expect("validated integer"),
                        )
                    });
                value.into_raw()
            }
            STRING => Rc::into_raw(Rc::new(Str::from(String::deserialize(deserializer)?))) as usize,
            BOOLEAN => usize::from(bool::deserialize(deserializer)?),
            FLOAT => {
                let raw = <&RawValue>::deserialize(deserializer)?.get();
                // Match JSON integer decoding followed by float conversion: -0
                // is integer zero, whereas -0.0 preserves its floating-point sign.
                let value = if raw == "-0" {
                    0.0
                } else {
                    raw.parse::<f64>()
                        .map_err(|_| D::Error::custom("expected a number"))?
                };
                if !value.is_finite() {
                    return Err(D::Error::custom("expected a finite float"));
                }
                value.to_bits() as usize
            }
            NONE => {
                <()>::deserialize(deserializer)?;
                0
            }
            LIST => return deserializer.deserialize_seq(self),
            RECORD => return deserializer.deserialize_map(self),
            OPTIONAL => return deserializer.deserialize_option(self),
            _ => unreachable!("compiler emitted an invalid JSON schema"),
        };
        // SAFETY: each branch transfers the exact schema's newly created owner.
        Ok(unsafe { OwnedWord::from_owned(word, schema.storage()) })
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = OwnedWord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0.kind {
            LIST => "an array",
            RECORD => "an object",
            _ => "a nullable value",
        })
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<OwnedWord, E> {
        // SAFETY: nullable words represent absence with zero and own no handle.
        Ok(unsafe { OwnedWord::from_owned(0, self.0.storage()) })
    }

    fn visit_some<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<OwnedWord, D::Error> {
        let child = Seed(self.0.child()).deserialize(deserializer)?;
        let kind = child.kind.raw();
        let word = child.into_word();
        let word = match kind {
            1 => word + 1,
            7 => Rc::into_raw(Rc::new(f64::from_bits(word as u64))) as usize,
            _ => word,
        };
        // SAFETY: scalar nullable encodings were promoted, other owners transfer unchanged.
        Ok(unsafe { OwnedWord::from_owned(word, self.0.storage()) })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<OwnedWord, A::Error> {
        let child = self.0.child();
        if self.0.word_kind == 4 {
            let mut values = Vec::new();
            while let Some(value) = sequence.next_element_seed(Seed(child))? {
                values.push(Int(value.into_word()));
            }
            // SAFETY: the schema is list[int], and every element owns an exact Int.
            return Ok(unsafe {
                OwnedWord::from_owned(crate::new_list(values) as usize, self.0.storage())
            });
        }
        let mut values = Words::new(child.word_kind);
        while let Some(value) = sequence.next_element_seed(Seed(child))? {
            values.values.push(value.into_word());
        }
        let pointer =
            Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Sequence(values))))) as usize;
        // SAFETY: this new generic sequence owns every decoded child word.
        Ok(unsafe { OwnedWord::from_owned(pointer, self.0.storage()) })
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<OwnedWord, A::Error> {
        let fields = self.0.fields();
        // SAFETY: emitted record metadata outlives every decoded record.
        let pointer = unsafe { relar_record_new(self.0.record) };
        // SAFETY: zeroed fields are cleanup-safe while the record is incomplete.
        let owner = unsafe { OwnedWord::from_owned(pointer as usize, self.0.storage()) };
        let mut seen = Seen::new(fields.len());
        while let Some(key) = map.next_key_seed(Key)? {
            let Some(index) = fields.iter().position(|field| field.name() == key) else {
                map.next_value::<IgnoredAny>()?;
                continue;
            };
            if !seen.insert(index) {
                return Err(A::Error::custom(format!(
                    "duplicate field {:?}",
                    fields[index].name()
                )));
            }
            let value = map
                .next_value_seed(Seed(fields[index].schema()))
                .map_err(|error| {
                    A::Error::custom(format!("field {:?}: {error}", fields[index].name()))
                })?;
            // SAFETY: each declared slot is initialized once with its exact owned type.
            unsafe { store(pointer.add(index), value) };
        }
        for (index, field) in fields.iter().enumerate() {
            if seen.insert(index) {
                let value = field.missing()?;
                // SAFETY: the missing slot is still zero and the default has its exact type.
                unsafe { store(pointer.add(index), value) };
            }
        }
        Ok(owner)
    }
}

/// Borrows ordinary keys; only escaped keys need a temporary decoded allocation.
struct Key;
impl<'de> DeserializeSeed<'de> for Key {
    type Value = Cow<'de, str>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_str(self)
    }
}
impl<'de> Visitor<'de> for Key {
    type Value = Cow<'de, str>;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string key")
    }
    fn visit_borrowed_str<E: serde::de::Error>(self, value: &'de str) -> Result<Self::Value, E> {
        Ok(Cow::Borrowed(value))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Cow::Owned(value.to_owned()))
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(Cow::Owned(value))
    }
}

/// Writes pointer fields with pointer provenance and float fields as native doubles.
unsafe fn store(destination: *mut usize, value: OwnedWord) {
    let kind = value.kind.raw();
    let word = value.into_word();
    // SAFETY: the caller supplies one aligned field with this exact native layout.
    unsafe {
        if matches!(kind, 3..=6 | 8) {
            destination.cast::<*const ()>().write(word as *const ());
        } else if kind == 7 {
            destination.cast::<f64>().write(f64::from_bits(word as u64));
        } else {
            destination.write(word);
        }
    }
}

fn empty_list(schema: &Schema) -> OwnedWord {
    let word = if schema.word_kind == 4 {
        crate::new_list(Vec::new()) as usize
    } else {
        super::relar_seq_new(schema.child().word_kind) as usize
    };
    // SAFETY: both branches create the schema's exact, independently owned list.
    unsafe { OwnedWord::from_owned(word, schema.storage()) }
}

/// Counts containers even in ignored values, where serde's skipping path does
/// not consume its recursion budget. Syntax validation remains the parser's job.
fn check_depth(text: &str) -> Result<(), &'static str> {
    let mut depth = 0_usize;
    let mut remaining = text.as_bytes();
    while let Some((&byte, rest)) = remaining.split_first() {
        remaining = rest;
        match byte {
            b'"' => loop {
                // Long names dominate report JSON. Skip ordinary string bytes
                // in vectorized spans, retaining the early limit on containers.
                let Some(index) = memchr::memchr2(b'"', b'\\', remaining) else {
                    // The parser reports unterminated strings and escapes.
                    return Ok(());
                };
                let closed = remaining[index] == b'"';
                remaining = &remaining[index + 1..];
                if closed {
                    break;
                }
                remaining = remaining.get(1..).unwrap_or(&[]);
            },
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err("JSON nesting exceeds 128 containers");
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

fn decode(text: &str, schema: &Schema) -> Result<OwnedWord, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    // The public entry point checks every container, including ignored ones.
    deserializer.disable_recursion_limit();
    let value = Seed(schema).deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

/// Decodes one complete document with an exact compiler-proved record schema.
///
/// # Safety
/// `text` is a rooted string. `schema` and all transitive metadata are immutable,
/// valid for this ABI, acyclic, and live for the lifetime of every returned value.
/// Success transfers one record owner; failure returns null with ValueError set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_decode(
    text: *const Str,
    schema: *const Schema,
) -> *const usize {
    // SAFETY: the caller roots the input and supplies valid static metadata.
    let (text, schema) = unsafe { (&*text, &*schema) };
    let result = check_depth(text)
        .map_err(|message| Error::new(Kind::ValueError, message))
        .and_then(|()| {
            decode(text, schema).map_err(|error| {
                Error::new(Kind::ValueError, format!("invalid typed JSON: {error}"))
            })
        })
        .map(|value| value.into_word() as *const usize);
    finish(result, std::ptr::null())
}
