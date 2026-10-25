//! TOML parsing retains scalar categories before applying concrete record schemas.
//!
//! The maintained parser validates the entire document, including ignored fields.
//! Only its temporary syntax tree is dynamic; published values use the same exact
//! native storage and cleanup rules as typed JSON.

use crate::StrOwner;
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

fn copy_string(value: &str, allocation: &mut Option<Error>) -> Result<crate::StrOwner, String> {
    Str::try_new(value)
        .ok()
        .and_then(|text| StrOwner::try_new(text).ok())
        .ok_or_else(|| {
            *allocation = Some(Error::allocation());
            String::new()
        })
}

fn convert(
    value: &DeValue<'_>,
    schema: &Schema,
    allocation: &mut Option<Error>,
) -> Result<OwnedWord, String> {
    if schema.kind == OPTIONAL {
        return convert(value, schema.child(), allocation).map(|value| optional(value, schema));
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
        (STRING, DeValue::String(value)) => {
            StrOwner::into_raw(copy_string(value, allocation)?) as usize
        }
        (BOOLEAN, DeValue::Boolean(value)) => usize::from(*value),
        (FLOAT, DeValue::Float(value)) => value
            .as_str()
            .parse::<f64>()
            .expect("parser validated and normalized the TOML float")
            .to_bits() as usize,
        (LIST, DeValue::Array(values)) => {
            let child = schema.child();
            let mut allocation_error = || {
                *allocation = Some(Error::allocation());
                String::new()
            };
            if schema.word_kind == 4 {
                let mut result = Vec::new();
                result
                    .try_reserve_exact(values.len())
                    .map_err(|_| allocation_error())?;
                for value in values {
                    result.push(Int(convert(value.get_ref(), child, allocation)?.into_word()));
                }
                crate::new_list(result) as usize
            } else {
                let mut result = Words::new(child.word_kind);
                result
                    .values
                    .try_reserve_exact(values.len())
                    .map_err(|_| allocation_error())?;
                for value in values {
                    result
                        .values
                        .push(convert(value.get_ref(), child, allocation)?.into_word());
                }
                crate::ObjectOwner::into_raw(result.into_object()) as usize
            }
        }
        (DICTIONARY, DeValue::Table(values)) => {
            let child = schema.child();
            let mut result = super::super::mappings::Mapping::new(3, child.word_kind);
            for (name, value) in values {
                let value = convert(value.get_ref(), child, allocation)?;
                let word = StrOwner::into_raw(copy_string(name.get_ref(), allocation)?) as usize;
                // SAFETY: the key transfers one newly allocated string owner.
                let key = unsafe { OwnedWord::from_owned(word, WordKind::new(3)) };
                assert!(
                    result.insert_unique(key, value).map_err(|error| {
                        *allocation = Some(error);
                        String::new()
                    })?,
                    "parser rejected duplicate keys"
                );
            }
            crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
                Storage::Dictionary(crate::objects::mappings::TableOwner::new(result)),
            )))) as usize
        }
        (RECORD, DeValue::Table(values)) => {
            let fields = schema.fields();
            // SAFETY: compiler-proved record metadata outlives each decoded value.
            let pointer = unsafe { imre_record_new(schema.record) };
            // SAFETY: zeroed fields are cleanup-safe until initialized below.
            let owner = unsafe { OwnedWord::from_owned(pointer as usize, schema.storage()) };
            for (index, field) in fields.iter().enumerate() {
                let value = if let Some(value) = values.get(field.name()) {
                    convert(value.get_ref(), field.schema(), allocation).map_err(|error| {
                        if allocation.is_some() {
                            error
                        } else {
                            format!("field {:?}: {error}", field.name())
                        }
                    })?
                } else {
                    field
                        .missing::<serde_json::Error>(allocation)
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

fn decode_toml(
    text: &str,
    schema: &Schema,
    allocation: &mut Option<Error>,
) -> Result<OwnedWord, String> {
    let table = DeTable::parse(text).map_err(|error| tomllib_parse_error(text, error))?;
    let value = DeValue::Table(table.into_inner());
    validate(&value, 1, text)?;
    convert(&value, schema, allocation)
}

/// CPython 3.14 reads TOML 1.0. The native parser also accepts four 1.1
/// extensions; inspect its tokens so punctuation inside a string or comment
/// cannot alter which grammar is accepted.
fn tomllib_syntax(text: &str) -> Result<(), &'static str> {
    use toml_parser::lexer::TokenKind::*;
    if text.starts_with('\u{feff}') {
        return Err("unexpected byte order mark");
    }
    let source = toml_parser::Source::new(text);
    let mut brackets = Vec::new();
    let mut previous = None;
    for token in source.lex() {
        let kind = token.kind();
        let raw = &text[token.span().start()..token.span().end()];
        match kind {
            LeftCurlyBracket | LeftSquareBracket => brackets.push(kind),
            RightCurlyBracket | RightSquareBracket => {
                if kind == RightCurlyBracket && previous == Some(Comma) {
                    return Err("trailing comma in inline table");
                }
                brackets.pop();
            }
            Newline if brackets.last() == Some(&LeftCurlyBracket) => {
                return Err("newline in inline table");
            }
            BasicString | MlBasicString => {
                let mut bytes = raw.bytes();
                while let Some(byte) = bytes.next() {
                    if byte == b'\\' && matches!(bytes.next(), Some(b'e' | b'x')) {
                        return Err("invalid string escape");
                    }
                }
            }
            Atom => {
                let time = raw.rsplit(['T', 't']).next().unwrap().as_bytes();
                if time.get(2) == Some(&b':') && time.get(5) != Some(&b':') {
                    return Err("time requires seconds");
                }
            }
            Whitespace | Comment => continue,
            _ => {}
        }
        previous = Some(kind);
    }
    Ok(())
}

/// Report the parser diagnosis and its position without rendering the input line.
fn tomllib_parse_error(text: &str, error: ::toml::de::Error) -> String {
    let title = error.message();
    let end = title.floor_char_boundary(256);
    let message = &title[..end];
    let omitted = if end < title.len() { "…" } else { "" };
    let Some(span) = error.span() else {
        return format!("{message}{omitted}");
    };
    let (message, mut at) = match message {
        "unclosed table, expected `]`" => {
            ("Expected ']' at the end of a table declaration", span.start)
        }
        "unclosed array table, expected `]]`" => (
            "Expected ']]' at the end of an array declaration",
            span.start,
        ),
        "unclosed array table, expected `]`" => (
            "Expected ']]' at the end of an array declaration",
            span.start.saturating_sub(1),
        ),
        _ => (message, span.start),
    };
    while matches!(text.as_bytes().get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    if at == text.len() {
        return format!("{message}{omitted} (at end of document)");
    }
    let prefix = &text[..at];
    let line = 1 + memchr::memchr_iter(b'\n', prefix.as_bytes()).count();
    let column = 1 + prefix
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count();
    format!("{message}{omitted} (at line {line}, column {column})")
}

/// Parse all values, including those under keys that callers never inspect.
/// The root dictionary cannot escape this projection, so its immutable view
/// needs only an insertion-ordered native key table.
///
/// # Safety
/// The caller roots the exact UTF-8 string for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_toml_keys(text: *const Str) -> *const Object {
    // SAFETY: the compiler checked and rooted the input.
    let text = unsafe { &*text };
    let table = (|| {
        let table = DeTable::parse(text).map_err(|err| tomllib_parse_error(text, err))?;
        tomllib_syntax(text)?;
        let value = DeValue::Table(table.into_inner());
        validate(&value, 1, text)?;
        let DeValue::Table(table) = value else {
            unreachable!()
        };
        Ok(table)
    })()
    .map_err(|error: String| Error::new(Kind::TomlDecodeError, error));
    let result = table.and_then(|table| {
        // SAFETY: this is one fresh set owner.
        let keys = unsafe { crate::ObjectOwner::from_raw(super::super::mappings::imre_set_new(3)) };
        for (name, _) in table {
            let text = Str::try_new(name.get_ref()).map_err(|_| Error::allocation())?;
            let string = StrOwner::try_new(text).map_err(|_| Error::allocation())?;
            // SAFETY: the set has string key storage and insertion retains the rooted key.
            unsafe {
                super::super::mappings::set_add(
                    crate::ObjectOwner::as_ptr(&keys),
                    StrOwner::as_ptr(&string) as usize,
                )
            }?;
        }
        Ok(crate::ObjectOwner::into_raw(keys))
    });
    finish(result, std::ptr::null())
}

/// Decode TOML 1.1 into an exact compiler-proved record or dictionary schema.
///
/// # Safety
/// `text` is rooted, and `schema` plus all transitive metadata is immutable,
/// acyclic, ABI-valid, and live for every returned owner. Root schemas use a
/// reference representation. Failure returns null with ValueError set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_toml_decode(text: *const Str, schema: *const Schema) -> *const usize {
    // SAFETY: caller roots the string and supplies live, compiler-checked metadata.
    let (text, schema) = unsafe { (&*text, &*schema) };
    let mut allocation = None;
    let result = decode_toml(text, schema, &mut allocation)
        .map(|value| value.into_word() as *const usize)
        .map_err(|error| {
            allocation.take().unwrap_or_else(|| {
                Error::new(Kind::ValueError, format!("invalid typed TOML: {error}"))
            })
        });
    finish(result, std::ptr::null())
}

pub(super) fn datetimes_equal(left: &str, right: &str) -> bool {
    left == right || utc_instant(left).is_some_and(|instant| Some(instant) == utc_instant(right))
}

fn utc_instant(text: &str) -> Option<(i64, u8, u32)> {
    use toml_datetime::{Datetime, Offset};

    let value: Datetime = text.parse().ok()?;
    let (date, time) = (value.date?, value.time?);
    let offset = match value.offset? {
        Offset::Z => 0,
        Offset::Custom { minutes } => i64::from(minutes),
    };

    // Count days from March so leap days fall at the end of the year.
    let year = i64::from(date.year) - i64::from(date.month <= 2);
    let month = (i64::from(date.month) + 9) % 12;
    let days = 365 * year + year / 4 - year / 100
        + year / 400
        + (153 * month + 2) / 5
        + i64::from(date.day);
    let minutes = (days * 24 + i64::from(time.hour)) * 60 + i64::from(time.minute) - offset;
    Some((
        minutes,
        time.second.unwrap_or(0),
        time.nanosecond.unwrap_or(0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_errors_omit_long_source_lines() {
        let text = format!("a ={}?", " ".repeat(8192));
        let error = DeTable::parse(&text).unwrap_err();
        let message = tomllib_parse_error(&text, error);
        assert!(message.len() < 128, "{message}");
        assert!(message.ends_with("(at line 1, column 8196)"), "{message}");
    }

    #[test]
    fn offset_datetimes_compare_by_instant() {
        for (left, right, expected) in [
            (
                "2000-01-01 00:00:00+00:00",
                "1999-12-31 23:30:00-00:30",
                true,
            ),
            (
                "2000-03-01 00:01:00+00:15",
                "2000-02-29 23:46:00+00:00",
                true,
            ),
            (
                "1900-03-01 00:01:00+00:15",
                "1900-02-28 23:46:00+00:00",
                true,
            ),
            (
                "2000-01-01 00:00:00.000001+00:00",
                "1999-12-31 23:30:00-00:30",
                false,
            ),
            ("2000-01-01 00:00:00", "2000-01-01 00:00:00+00:00", false),
            ("2000-01-01", "2000-01-01", true),
            ("12:00:00", "12:00:01", false),
        ] {
            assert_eq!(datetimes_equal(left, right), expected, "{left} / {right}");
            assert_eq!(datetimes_equal(right, left), expected, "{right} / {left}");
        }
    }
}
