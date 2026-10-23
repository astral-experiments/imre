//! TOML parsing retains scalar categories before applying concrete record schemas.
//!
//! The maintained parser validates the entire document, including ignored fields.
//! Only its temporary syntax tree is dynamic; published values use the same exact
//! native storage and cleanup rules as typed JSON.

use ::toml::de::{DeTable, DeValue};

use super::*;

const MAX_TOML_DEPTH: usize = 64;

/// Validate the entire document, including fields that the schema will ignore.
fn validate(value: &DeValue<'_>, depth: usize, text: &str) -> Result<(), String> {
    match value {
        DeValue::Array(values) => {
            if depth > MAX_TOML_DEPTH {
                return Err("TOML nesting exceeds 64 containers".to_owned());
            }
            for value in values {
                validate(value.get_ref(), depth + 1, text)?;
            }
        }
        DeValue::Table(values) => {
            if depth > MAX_TOML_DEPTH {
                return Err("TOML nesting exceeds 64 containers".to_owned());
            }
            for (key, value) in values {
                // Only assigned values start after the key. Table headers and
                // implicit dotted tables have spans that overlap the key.
                let gap = key.span().end..value.span().start;
                if text.get(gap).is_some_and(|gap| gap.contains('\n')) {
                    return Err("TOML key and value must start on the same line".to_owned());
                }
                validate(value.get_ref(), depth + 1, text)?;
            }
        }
        DeValue::Integer(value) => {
            // The parser can return empty digits or non-decimal digits after an underscore.
            let number = value.as_str();
            let digits = number.strip_prefix(['+', '-']).unwrap_or(number);
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(value.radix())) {
                return Err("invalid TOML integer".to_owned());
            }
        }
        DeValue::Datetime(value)
            if value.date.is_some_and(|date| date.year == 0)
                || value
                    .time
                    .is_some_and(|time| time.second.is_some_and(|second| second > 59)) =>
        {
            return Err("TOML datetime is outside the supported calendar range".to_owned());
        }
        _ => {}
    }
    Ok(())
}

/// Promote an initialized child to the optional ABI without creating a second owner.
fn optional(value: OwnedWord, schema: &Schema) -> OwnedWord {
    let kind = value.kind.raw();
    let word = value.into_word();
    let word = match kind {
        1 => word + 1,
        7 => Rc::into_raw(Rc::new(f64::from_bits(word as u64))) as usize,
        _ => word,
    };
    // SAFETY: optional scalar encodings are promoted; pointer owners transfer unchanged.
    unsafe { OwnedWord::from_owned(word, schema.storage()) }
}

fn convert(value: &DeValue<'_>, schema: &Schema) -> Result<OwnedWord, String> {
    if schema.kind == OPTIONAL {
        return convert(value, schema.child()).map(|value| optional(value, schema));
    }
    let word = match (schema.kind, value) {
        (INTEGER, DeValue::Integer(value)) => {
            let digits = value.as_str();
            let radix = value.radix();
            let integer = i64::from_str_radix(digits, radix)
                .ok()
                .and_then(Int::small)
                .or_else(|| num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix).map(Int::new))
                .ok_or("invalid TOML integer")?;
            integer.into_raw()
        }
        (STRING, DeValue::String(value)) => Rc::into_raw(Rc::new(Str::new(value))) as usize,
        (BOOLEAN, DeValue::Boolean(value)) => usize::from(*value),
        (FLOAT, DeValue::Float(value)) => value
            .as_str()
            .parse::<f64>()
            .expect("parser validated and normalized the TOML float")
            .to_bits() as usize,
        (LIST, DeValue::Array(values)) => {
            let child = schema.child();
            if schema.word_kind == 4 {
                let values = values
                    .iter()
                    .map(|value| {
                        convert(value.get_ref(), child).map(|value| Int(value.into_word()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                crate::new_list(values) as usize
            } else {
                let mut result = Words::new(child.word_kind);
                for value in values {
                    result
                        .values
                        .push(convert(value.get_ref(), child)?.into_word());
                }
                Rc::into_raw(result.into_object()) as usize
            }
        }
        (DICTIONARY, DeValue::Table(values)) => {
            let child = schema.child();
            let mut result = super::super::mappings::Mapping::new(3, child.word_kind);
            for (name, value) in values {
                let value = convert(value.get_ref(), child)?;
                let word = Rc::into_raw(Rc::new(Str::new(name.get_ref()))) as usize;
                // SAFETY: the key transfers one newly allocated string owner.
                let key = unsafe { OwnedWord::from_owned(word, WordKind::new(3)) };
                assert!(
                    result.insert_unique(key, value),
                    "parser rejected duplicate keys"
                );
            }
            Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Dictionary(
                Box::new(result),
            ))))) as usize
        }
        (RECORD, DeValue::Table(values)) => {
            let fields = schema.fields();
            // SAFETY: compiler-proved record metadata outlives each decoded value.
            let pointer = unsafe { relar_record_new(schema.record) };
            // SAFETY: zeroed fields are cleanup-safe until initialized below.
            let owner = unsafe { OwnedWord::from_owned(pointer as usize, schema.storage()) };
            for (index, field) in fields.iter().enumerate() {
                let value = if let Some(value) = values.get(field.name()) {
                    convert(value.get_ref(), field.schema())
                        .map_err(|error| format!("field {:?}: {error}", field.name()))?
                } else {
                    field
                        .missing::<serde_json::Error>()
                        .map_err(|error| error.to_string())?
                };
                // SAFETY: each field is written once with its exact owned representation.
                unsafe { store(pointer.add(index), value) };
            }
            return Ok(owner);
        }
        (_, DeValue::Datetime(_)) => {
            return Err("TOML date and time values cannot be exposed by a schema".to_owned());
        }
        _ => return Err("TOML value does not match schema".to_owned()),
    };
    // SAFETY: each successful branch constructs exactly the selected schema storage.
    Ok(unsafe { OwnedWord::from_owned(word, schema.storage()) })
}

fn decode_toml(text: &str, schema: &Schema) -> Result<OwnedWord, String> {
    let table = DeTable::parse(text).map_err(|error| error.to_string())?;
    let value = DeValue::Table(table.into_inner());
    validate(&value, 1, text)?;
    convert(&value, schema)
}

/// Decode TOML 1.1 into an exact compiler-proved record or dictionary schema.
///
/// # Safety
/// `text` is rooted, and `schema` plus all transitive metadata is immutable,
/// acyclic, ABI-valid, and live for every returned owner. Root schemas use a
/// reference representation. Failure returns null with ValueError set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_toml_decode(
    text: *const Str,
    schema: *const Schema,
) -> *const usize {
    // SAFETY: caller roots the string and supplies live, compiler-checked metadata.
    let (text, schema) = unsafe { (&*text, &*schema) };
    let result = decode_toml(text, schema)
        .map(|value| value.into_word() as *const usize)
        .map_err(|error| Error::new(Kind::ValueError, format!("invalid typed TOML: {error}")));
    finish(result, std::ptr::null())
}
