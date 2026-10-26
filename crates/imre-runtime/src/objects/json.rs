//! Schema-directed JSON decoding into existing acyclic native storage.
//!
//! The compiler proves structural constructors and supplies immutable metadata.
//! Every partial record and array owns its initialized words, so an error never
//! publishes incomplete values or leaks earlier fields. Unknown fields are
//! validated and skipped without building an intermediate object tree.

use crate::StrOwner;
use std::borrow::Cow;
use std::fmt;
use std::rc::Rc;

use serde::de::{
    Deserialize, DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Unexpected, Visitor,
};
use serde_json::value::RawValue;

use super::records::{RecordBatch, RecordDescriptor, imre_record_new};
use super::words::{OwnedWord, WordKind, Words};
use super::{Error, Int, Kind, Object, Storage, StorageCell, Str, finish};

mod binary;
mod decoder;
mod encode;
#[cfg(test)]
pub(super) use encode::imre_json_dumps;
pub(super) mod readonly;
mod syntax;
mod toml;

const INTEGER: u32 = 1;
const STRING: u32 = 2;
const BOOLEAN: u32 = 3;
const FLOAT: u32 = 4;
const NONE: u32 = 5;
const LIST: u32 = 6;
const RECORD: u32 = 7;
const OPTIONAL: u32 = 8;
const DICTIONARY: u32 = 9;
const MAX_DEPTH: usize = 128;

/// Compiler-owned metadata. `kind` selects the scalar/container schema; `word_kind` selects the
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

    fn missing<E: serde::de::Error>(&self, allocation: &mut Option<Error>) -> Result<OwnedWord, E> {
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
                decode(text, self.schema(), allocation).map_err(E::custom)
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

type StringSlots = [Option<StrOwner>; 64];

/// Raw numeric tokens and ignored containers can bypass serde's recursion
/// budget. Check the full document once before admitting such values.
struct DecodeState<'a> {
    allocation: &'a mut Option<Error>,
    text: &'a str,
    checked: bool,
    strings: Option<&'a mut StringSlots>,
    records: Option<RecordBatch<'a>>,
}

impl DecodeState<'_> {
    fn array_allocation<E: serde::de::Error>(&mut self) -> E {
        *self.allocation = Some(Error::allocation());
        E::custom("array allocation failed")
    }

    fn check<E: serde::de::Error>(&mut self) -> Result<(), E> {
        if !self.checked {
            check_depth(self.text).map_err(E::custom)?;
            self.checked = true;
        }
        Ok(())
    }
}

struct Seed<'schema, 'state, 'de>(&'schema Schema, &'state mut DecodeState<'de>);

impl<'de> DeserializeSeed<'de> for Seed<'_, '_, 'de> {
    type Value = OwnedWord;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<OwnedWord, D::Error> {
        let schema = self.0;
        if matches!(schema.kind, INTEGER | FLOAT) {
            self.1.check()?;
        }
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
            STRING => return deserializer.deserialize_str(self),
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
            // Validate these kinds in the visitor. The type-specific deserializers
            // include the entire rejected string in their mismatch diagnostics.
            BOOLEAN | NONE | LIST | RECORD | DICTIONARY => {
                return deserializer.deserialize_any(self);
            }
            OPTIONAL => return deserializer.deserialize_option(self),
            _ => unreachable!("compiler emitted an invalid JSON schema"),
        };
        // SAFETY: each branch transfers the exact schema's newly created owner.
        Ok(unsafe { OwnedWord::from_owned(word, schema.storage()) })
    }
}

impl<'de> Visitor<'de> for Seed<'_, '_, 'de> {
    type Value = OwnedWord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0.kind {
            BOOLEAN => "a boolean",
            NONE => "null",
            STRING => "a string",
            LIST => "an array",
            RECORD | DICTIONARY => "an object",
            _ => "a nullable value",
        })
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<OwnedWord, E> {
        if self.0.kind != BOOLEAN {
            return Err(E::invalid_type(Unexpected::Bool(value), &self));
        }
        // SAFETY: booleans are stored as immediate zero/one words.
        Ok(unsafe { OwnedWord::from_owned(usize::from(value), self.0.storage()) })
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<OwnedWord, E> {
        if self.0.kind != NONE {
            return Err(E::invalid_type(Unexpected::Unit, &self));
        }
        self.visit_none()
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        if self.0.kind != STRING {
            return Err(E::invalid_type(Unexpected::Other("a string"), &self));
        }
        // A collision evicts one immutable owner from the document-local table.
        let owner = if !value.is_empty()
            && value.len() <= 32
            && let Some(strings) = self.1.strings.as_deref_mut()
        {
            let bytes = value.as_bytes();
            let index = (value.len() ^ usize::from(bytes[0]) ^ usize::from(bytes[bytes.len() - 1]))
                % strings.len();
            let slot = &mut strings[index];
            if let Some(owner) = slot
                && owner.as_str() == value
            {
                StrOwner::clone(owner)
            } else {
                let owner = StrOwner::new(Str::new(value));
                *slot = Some(StrOwner::clone(&owner));
                owner
            }
        } else if value.len() > 32 && self.1.records.is_some() {
            super::records::strings::copy(value)
        } else {
            StrOwner::new(Str::new(value))
        };
        // SAFETY: the checked string schema owns this new string handle.
        Ok(unsafe { OwnedWord::from_owned(StrOwner::into_raw(owner) as usize, self.0.storage()) })
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<OwnedWord, E> {
        // SAFETY: nullable words represent absence with zero and own no handle.
        Ok(unsafe { OwnedWord::from_owned(0, self.0.storage()) })
    }

    fn visit_some<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<OwnedWord, D::Error> {
        let child = Seed(self.0.child(), self.1).deserialize(deserializer)?;
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
        if self.0.kind != LIST {
            return Err(A::Error::invalid_type(Unexpected::Seq, &self));
        }
        let child = self.0.child();
        if self.0.word_kind == 4 {
            let mut values = Vec::new();
            while let Some(value) = sequence.next_element_seed(Seed(child, self.1))? {
                values
                    .try_reserve(1)
                    .map_err(|_| self.1.array_allocation::<A::Error>())?;
                values.push(Int(value.into_word()));
            }
            // SAFETY: the schema is list[int], and every element owns an exact Int.
            return Ok(unsafe {
                OwnedWord::from_owned(crate::new_list(values) as usize, self.0.storage())
            });
        }
        let mut values = Words::new(child.word_kind);
        if child.kind == RECORD {
            // Nested arrays fill independent blocks. An error aborts the decode
            // and drops both the suspended cursor and this array's partial owners.
            let outer = self.1.records.take();
            while let Some(value) = sequence.next_element_seed(Seed(child, self.1))? {
                values
                    .values
                    .try_reserve(1)
                    .map_err(|_| self.1.array_allocation::<A::Error>())?;
                values.values.push(value.into_word());
                if values.values.len() == values.values.inline_size() + 1 {
                    // Once the array spills its inline slots, pack subsequent records.
                    // SAFETY: the element descriptor outlives this decode and its results.
                    self.1.records = RecordBatch::new(unsafe { &*child.record });
                }
            }
            self.1.records = outer;
        } else {
            while let Some(value) = sequence.next_element_seed(Seed(child, self.1))? {
                values
                    .values
                    .try_reserve(1)
                    .map_err(|_| self.1.array_allocation::<A::Error>())?;
                values.values.push(value.into_word());
            }
        }
        let pointer = crate::ObjectOwner::into_raw(values.into_object()) as usize;
        // SAFETY: this new generic sequence owns every decoded child word.
        Ok(unsafe { OwnedWord::from_owned(pointer, self.0.storage()) })
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<OwnedWord, A::Error> {
        if self.0.kind == DICTIONARY {
            let child = self.0.child();
            let mut mapping = super::mappings::Mapping::new(3, child.word_kind);
            while let Some(key) = map.next_key_seed(Key)? {
                let value = map.next_value_seed(Seed(child, self.1))?;
                let pointer = StrOwner::into_raw(StrOwner::new(Str::from(key.as_ref()))) as usize;
                // SAFETY: this temporary transfers one newly allocated string owner.
                let key = unsafe { OwnedWord::from_owned(pointer, WordKind::new(3)) };
                let inserted = match mapping.insert_unique(key, value) {
                    Ok(inserted) => inserted,
                    Err(error) => {
                        drop(mapping);
                        *self.1.allocation = Some(error);
                        return Err(serde::de::Error::custom("mapping allocation failed"));
                    }
                };
                if !inserted {
                    return Err(A::Error::custom("duplicate JSON dictionary key"));
                }
            }
            let pointer =
                crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
                    Storage::Dictionary(crate::objects::mappings::TableOwner::new(mapping)),
                )))) as usize;
            // SAFETY: the new dictionary owns every successfully decoded entry.
            return Ok(unsafe { OwnedWord::from_owned(pointer, self.0.storage()) });
        }
        if self.0.kind != RECORD {
            return Err(A::Error::invalid_type(Unexpected::Map, &self));
        }
        let fields = self.0.fields();
        // SAFETY: emitted record metadata outlives every decoded record.
        let pointer = unsafe {
            match &mut self.1.records {
                Some(records) if records.accepts(self.0.record) => records.allocate(),
                _ => imre_record_new(self.0.record),
            }
        };
        // SAFETY: zeroed fields are cleanup-safe while the record is incomplete.
        let owner = unsafe { OwnedWord::from_owned(pointer as usize, self.0.storage()) };
        let mut seen = Seen::new(fields.len());
        while let Some(key) = map.next_key_seed(Key)? {
            let Some(index) = fields.iter().position(|field| field.name() == key) else {
                if self.1.checked {
                    map.next_value::<IgnoredAny>()?;
                } else {
                    map.next_value_seed(Ignore(self.1))?;
                }
                continue;
            };
            if !seen.insert(index) {
                return Err(A::Error::custom(format!(
                    "duplicate field {:?}",
                    fields[index].name()
                )));
            }
            let value = map
                .next_value_seed(Seed(fields[index].schema(), self.1))
                .map_err(|error| {
                    A::Error::custom(format!("field {:?}: {error}", fields[index].name()))
                })?;
            // SAFETY: each declared slot is initialized once with its exact owned type.
            unsafe { store(pointer.add(index), value) };
        }
        for (index, field) in fields.iter().enumerate() {
            if seen.insert(index) {
                let value = field.missing(self.1.allocation)?;
                // SAFETY: the missing slot is still zero and the default has its exact type.
                unsafe { store(pointer.add(index), value) };
            }
        }
        Ok(owner)
    }
}

/// Skip scalars without a depth scan. Before serde can skip an arbitrary
/// container, check the nesting of the complete document once.
struct Ignore<'state, 'de>(&'state mut DecodeState<'de>);

impl<'de> DeserializeSeed<'de> for Ignore<'_, 'de> {
    type Value = IgnoredAny;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Ignore<'_, 'de> {
    type Value = IgnoredAny;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }
    fn visit_unit<E>(self) -> Result<IgnoredAny, E> {
        Ok(IgnoredAny)
    }
    fn visit_bool<E>(self, _: bool) -> Result<IgnoredAny, E> {
        Ok(IgnoredAny)
    }
    fn visit_str<E>(self, _: &str) -> Result<IgnoredAny, E> {
        Ok(IgnoredAny)
    }
    fn visit_i64<E>(self, _: i64) -> Result<IgnoredAny, E> {
        Ok(IgnoredAny)
    }
    fn visit_u64<E>(self, _: u64) -> Result<IgnoredAny, E> {
        Ok(IgnoredAny)
    }
    fn visit_f64<E>(self, _: f64) -> Result<IgnoredAny, E> {
        Ok(IgnoredAny)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<IgnoredAny, A::Error> {
        self.0.check()?;
        IgnoredAny.visit_seq(seq)
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<IgnoredAny, A::Error> {
        self.0.check()?;
        IgnoredAny.visit_map(map)
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
    let list = if schema.kind == OPTIONAL {
        schema.child()
    } else {
        schema
    };
    let word = if list.word_kind == 4 {
        crate::new_list(Vec::new()) as usize
    } else {
        super::imre_seq_new(list.child().word_kind) as usize
    };
    // SAFETY: both branches create the exact list payload. Nullable list storage
    // uses that same pointer, so the original schema retains the correct owner.
    unsafe { OwnedWord::from_owned(word, schema.storage()) }
}

/// Counts containers even in ignored values, where serde's skipping path does
/// not consume its recursion budget. Syntax validation remains the parser's job.
fn check_depth(text: &str) -> Result<(), &'static str> {
    // Even brackets inside strings are a conservative upper bound. Most flat
    // numeric documents can be certified without tokenizing quotes and escapes.
    if memchr::memchr2_iter(b'[', b'{', text.as_bytes())
        .nth(MAX_DEPTH)
        .is_none()
    {
        return Ok(());
    }
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

fn decode(
    text: &str,
    schema: &Schema,
    allocation: &mut Option<Error>,
) -> Result<OwnedWord, serde_json::Error> {
    decode_with_depth(text, schema, true, allocation)
}

/// Starts with serde's recursion budget unless the caller already checked all
/// containers. An error on the speculative path is never exposed to the caller.
fn decode_with_depth(
    text: &str,
    schema: &Schema,
    checked: bool,
    allocation: &mut Option<Error>,
) -> Result<OwnedWord, serde_json::Error> {
    // Even malformed input this short cannot open more than MAX_DEPTH containers.
    let checked = checked || text.len() <= MAX_DEPTH;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    if checked {
        deserializer.disable_recursion_limit();
    }
    // Keep the table on the stack for large documents. Inputs smaller than
    // the table bypass its initialization and destruction entirely.
    let mut slots;
    let strings = if text.len() >= size_of::<StringSlots>() {
        slots = [const { None }; 64];
        Some(&mut slots)
    } else {
        None
    };
    let value = Seed(
        schema,
        &mut DecodeState {
            allocation,
            text,
            checked,
            strings,
            records: None,
        },
    )
    .deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

/// Decodes one complete document with an exact compiler-proved record schema.
///
/// # Safety
/// `text` is a rooted string. `schema` and all transitive metadata are immutable,
/// valid for this ABI, acyclic, and live for the lifetime of every returned value.
/// Success transfers one record, list, or dictionary owner; failure returns null
/// with ValueError set. Root schemas always use a reference representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_decode(text: *const Str, schema: *const Schema) -> *const usize {
    // SAFETY: the caller roots the input and supplies valid static metadata.
    let (text, schema) = unsafe { (&*text, &*schema) };
    let mut allocation = None;
    let result = decode_with_depth(text, schema, false, &mut allocation)
        .or_else(|_| {
            if let Some(error) = allocation.take() {
                return Err(error);
            }
            check_depth(text).map_err(|message| Error::new(Kind::ValueError, message))?;
            decode(text, schema, &mut allocation).map_err(|error| {
                allocation.take().unwrap_or_else(|| {
                    Error::new(Kind::ValueError, format!("invalid typed JSON: {error}"))
                })
            })
        })
        .map(|value| value.into_word() as *const usize);
    finish(result, std::ptr::null())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_integer_list_factories_preserve_payload_layout_and_fresh_identity() {
        let integer = Schema {
            kind: INTEGER,
            word_kind: 2,
            element: std::ptr::null(),
            record: std::ptr::null(),
            field_count: 0,
            fields: std::ptr::null(),
        };
        let list = Schema {
            kind: LIST,
            word_kind: 4,
            element: &integer,
            ..integer
        };
        let optional = Schema {
            kind: OPTIONAL,
            word_kind: 256 | 4,
            element: &list,
            ..integer
        };
        let first = empty_list(&optional);
        let second = empty_list(&optional);
        assert_eq!(first.kind.raw(), 256 | 4);
        assert_ne!(first.word, second.word);
        // SAFETY: each factory result owns an exact native integer list. Mutating
        // one verifies its payload layout and independence from the other result.
        unsafe {
            crate::imre_list_append(first.word as *const crate::List, Int::small(7).unwrap().0);
            assert_eq!(
                Int::borrow(crate::imre_list_len(first.word as *const crate::List)).as_small(),
                Some(1)
            );
            assert_eq!(
                Int::borrow(crate::imre_list_len(second.word as *const crate::List)).as_small(),
                Some(0)
            );
        }
    }
}
