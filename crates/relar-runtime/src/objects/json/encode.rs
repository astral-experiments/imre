//! Writes typed storage directly, without constructing a dynamic JSON tree.

use num_traits::ToPrimitive;

use super::*;
use crate::objects::{List, object};

struct Encoder {
    output: String,
    indent: Option<usize>,
    ensure_ascii: bool,
}

/// Recognizes strings that need no escaping in either JSON output mode. Checking
/// eight bytes at a time lets ordinary names and URLs use one bulk copy.
fn plain_ascii(bytes: &[u8]) -> bool {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let has_zero = |word: u64| word.wrapping_sub(ONES) & !word & HIGH != 0;
    let (chunks, remainder) = bytes.as_chunks::<8>();
    for chunk in chunks {
        let word = u64::from_ne_bytes(*chunk);
        if word & HIGH != 0
            || word.wrapping_sub(0x2020_2020_2020_2020) & !word & HIGH != 0
            || has_zero(word ^ 0x2222_2222_2222_2222)
            || has_zero(word ^ 0x5c5c_5c5c_5c5c_5c5c)
            || has_zero(word ^ 0x7f7f_7f7f_7f7f_7f7f)
        {
            return false;
        }
    }
    remainder
        .iter()
        .all(|&byte| (b' '..=b'~').contains(&byte) && byte != b'"' && byte != b'\\')
}

impl Encoder {
    #[inline]
    fn reserve(&mut self, additional: usize) -> Result<(), Error> {
        if additional <= self.output.capacity() - self.output.len() {
            return Ok(());
        }
        self.grow(additional)
    }

    #[cold]
    fn grow(&mut self, additional: usize) -> Result<(), Error> {
        // A geometrically larger buffer can exceed the process limit while the
        // result still fits. Grow in smaller chunks under memory pressure.
        self.output
            .try_reserve(additional)
            .or_else(|_| self.output.try_reserve_exact(additional.max(1 << 20)))
            .or_else(|_| self.output.try_reserve_exact(additional))
            .map_err(|_| Error::new(Kind::MemoryError, "JSON output allocation failed"))
    }

    #[inline]
    fn append(&mut self, text: &str) -> Result<(), Error> {
        self.reserve(text.len())?;
        self.output.push_str(text);
        Ok(())
    }

    /// JSON's escapes differ from Python repr, including UTF-16 surrogate pairs
    /// for non-BMP characters when ensure_ascii is requested.
    fn string(&mut self, value: &str) -> Result<(), Error> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        if plain_ascii(value.as_bytes()) {
            self.reserve(value.len().saturating_add(2))?;
            self.output.push('"');
            self.output.push_str(value);
            self.output.push('"');
            return Ok(());
        }
        self.reserve(1)?;
        self.output.push('"');
        let mut rest = value;
        while !rest.is_empty() {
            // At most six output bytes per input byte. Checking one UTF-8 chunk
            // at a time bounds spare capacity outside the per-character loop.
            let (chunk, tail) = rest.split_at(rest.floor_char_boundary(rest.len().min(4096)));
            self.reserve(6 * chunk.len())?;
            for character in chunk.chars() {
                match character {
                    '"' => self.output.push_str("\\\""),
                    '\\' => self.output.push_str("\\\\"),
                    '\u{8}' => self.output.push_str("\\b"),
                    '\u{c}' => self.output.push_str("\\f"),
                    '\n' => self.output.push_str("\\n"),
                    '\r' => self.output.push_str("\\r"),
                    '\t' => self.output.push_str("\\t"),
                    '\0'..='\u{1f}' => {
                        let value = character as usize;
                        self.output.push_str("\\u00");
                        self.output.push(char::from(HEX[value >> 4]));
                        self.output.push(char::from(HEX[value & 15]));
                    }
                    _ if self.ensure_ascii && !(' '..='~').contains(&character) => {
                        let mut buffer = [0; 2];
                        for &unit in character.encode_utf16(&mut buffer).iter() {
                            self.output.push_str("\\u");
                            for shift in [12, 8, 4, 0] {
                                self.output
                                    .push(char::from(HEX[usize::from((unit >> shift) & 15)]));
                            }
                        }
                    }
                    _ => self.output.push(character),
                }
            }
            rest = tail;
        }
        self.reserve(1)?;
        self.output.push('"');
        Ok(())
    }

    fn newline(&mut self, depth: usize) -> Result<(), Error> {
        let count = self
            .indent
            .unwrap()
            .checked_mul(depth)
            .and_then(|count| count.checked_add(1))
            .filter(|&count| count <= isize::MAX as usize)
            .ok_or_else(|| Error::new(Kind::MemoryError, "JSON indentation is too large"))?;
        self.reserve(count)?;
        self.output.push('\n');
        self.output.extend(std::iter::repeat_n(' ', count - 1));
        Ok(())
    }

    fn begin(&mut self, delimiter: char, depth: usize) -> Result<(), Error> {
        if depth >= MAX_DEPTH {
            return Err(Error::new(
                Kind::ValueError,
                "JSON nesting exceeds 128 containers",
            ));
        }
        self.reserve(1)?;
        self.output.push(delimiter);
        Ok(())
    }

    fn separator(&mut self, index: usize, depth: usize) -> Result<(), Error> {
        if self.indent.is_some() {
            if index != 0 {
                self.append(",")?;
            }
            self.newline(depth + 1)
        } else if index != 0 {
            self.append(", ")
        } else {
            Ok(())
        }
    }

    fn end(&mut self, delimiter: char, nonempty: bool, depth: usize) -> Result<(), Error> {
        if nonempty && self.indent.is_some() {
            self.newline(depth)?;
        }
        self.reserve(1)?;
        self.output.push(delimiter);
        Ok(())
    }

    /// # Safety
    /// The iterator borrows initialized words of `schema` for the whole call.
    unsafe fn array(
        &mut self,
        values: impl IntoIterator<Item = usize>,
        schema: &Schema,
        depth: usize,
    ) -> Result<(), Error> {
        self.begin('[', depth)?;
        let mut nonempty = false;
        for (index, value) in values.into_iter().enumerate() {
            nonempty = true;
            self.separator(index, depth)?;
            // SAFETY: the enclosing storage borrow roots this exact child word.
            unsafe { self.value(value, schema, depth + 1) }?;
        }
        self.end(']', nonempty, depth)
    }

    /// # Safety
    /// `word` and every descendant are rooted with the exact immutable schema.
    /// The checked type graph is acyclic and no callback or mutation can occur.
    unsafe fn value(&mut self, word: usize, schema: &Schema, depth: usize) -> Result<(), Error> {
        // SAFETY: each branch reads only the schema's exact rooted representation.
        unsafe {
            match schema.kind {
                NONE => self.append("null")?,
                BOOLEAN => self.append(if word == 0 { "false" } else { "true" })?,
                INTEGER => {
                    let value = Int::borrow(word);
                    if let Some(value) = value.as_small() {
                        self.append(itoa::Buffer::new().format(value))?;
                    } else {
                        self.append(&value.big().to_string())?;
                    }
                }
                FLOAT => {
                    let value = f64::from_bits(word as u64);
                    if !value.is_finite() {
                        return Err(Error::new(
                            Kind::ValueError,
                            "non-finite floats are not JSON",
                        ));
                    }
                    self.append(&crate::floats::render(value))?;
                }
                STRING => self.string(&*(word as *const Str))?,
                OPTIONAL if word == 0 => self.append("null")?,
                OPTIONAL => {
                    let child = schema.child();
                    let word = match child.word_kind {
                        1 => word - 1,
                        7 => (*(word as *const f64)).to_bits() as usize,
                        _ => word,
                    };
                    self.value(word, child, depth)?;
                }
                LIST if schema.word_kind == 4 => {
                    (&*(word as *const List)).0.with(|values| {
                        self.array(values.iter().map(|value| value.0), schema.child(), depth)
                    })?;
                }
                LIST => {
                    object(word as *const Object).0.with(|storage| {
                        self.array(
                            storage.sequence().values.iter().copied(),
                            schema.child(),
                            depth,
                        )
                    })?;
                }
                RECORD => {
                    self.begin('{', depth)?;
                    for (index, field) in schema.fields().iter().enumerate() {
                        self.separator(index, depth)?;
                        self.string(field.name())?;
                        self.append(": ")?;
                        let child = field.schema();
                        let word = child
                            .storage()
                            .read_record_field((word as *const usize).add(index));
                        self.value(word, child, depth + 1)?;
                    }
                    self.end('}', schema.field_count != 0, depth)?;
                }
                DICTIONARY => {
                    self.begin('{', depth)?;
                    let nonempty = object(word as *const Object).0.with(|storage| {
                        let mut nonempty = false;
                        for (index, (key, value)) in
                            storage.dictionary().string_entries().enumerate()
                        {
                            nonempty = true;
                            self.separator(index, depth)?;
                            self.string(key)?;
                            self.append(": ")?;
                            self.value(value, schema.child(), depth + 1)?;
                        }
                        Ok::<_, Error>(nonempty)
                    })?;
                    self.end('}', nonempty, depth)?;
                }
                _ => unreachable!("the compiler validates JSON encoding schemas"),
            }
        }
        Ok(())
    }
}

/// Encodes one rooted value in declaration/insertion order. The output uses
/// Python's default separators, optional integer indentation, Unicode escaping,
/// shortest float spelling, and arbitrary-precision decimal integers.
///
/// # Safety
/// `word` uses the supplied static schema and roots an acyclic value graph.
/// `indent` is an optional Int word; every input remains live until return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_json_encode(
    word: usize,
    schema: *const Schema,
    indent: usize,
    ensure_ascii: u32,
) -> *const Str {
    let result = (|| {
        let indent = if indent == 0 {
            None
        } else {
            // SAFETY: a present optional Int has the ordinary Int representation.
            let value = unsafe { Int::borrow(indent) };
            let value = value
                .as_small()
                .and_then(|value| isize::try_from(value).ok())
                .or_else(|| value.big().to_isize())
                .ok_or_else(|| {
                    Error::new(
                        Kind::OverflowError,
                        "JSON indent does not fit in a platform index",
                    )
                })?;
            Some(value.max(0) as usize)
        };
        let mut encoder = Encoder {
            output: String::new(),
            indent,
            ensure_ascii: ensure_ascii != 0,
        };
        // SAFETY: the caller roots the complete graph and immutable schema.
        unsafe { encoder.value(word, &*schema, 0) }?;
        Ok(Rc::into_raw(Rc::new(Str::from(encoder.output))))
    })();
    finish(result, std::ptr::null())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::exceptions::{relar_error_take, relar_exception_matches};

    #[test]
    fn failed_reservation_preserves_the_encoder_for_a_shorter_value() {
        let mut encoder = Encoder {
            output: String::from("["),
            indent: None,
            ensure_ascii: true,
        };
        assert!(encoder.reserve(usize::MAX).is_err());
        assert!(encoder.string("\0𐐀").is_ok());
        assert!(encoder.end(']', true, 0).is_ok());
        assert_eq!(encoder.output, r#"["\u0000\ud801\udc00"]"#);
    }

    #[test]
    fn plain_ascii_checks_every_byte_and_chunk_boundary() {
        for length in 0..=25 {
            let mut bytes = vec![b'x'; length];
            assert!(plain_ascii(&bytes));
            for index in 0..length {
                for byte in 0..=u8::MAX {
                    bytes[index] = byte;
                    let expected = (b' '..=b'~').contains(&byte) && byte != b'"' && byte != b'\\';
                    assert_eq!(plain_ascii(&bytes), expected, "{bytes:?}");
                }
                bytes[index] = b'x';
            }
        }
        assert!(!plain_ascii(b"\x1f       "));
        assert!(!plain_ascii(b"!\"!!!!!!"));
        assert!(!plain_ascii(b"]\\]]]]]]"));
    }

    #[test]
    fn nesting_limit_rejects_without_consuming_the_input() {
        let mut schemas = vec![Box::new(Schema {
            kind: INTEGER,
            word_kind: 2,
            element: std::ptr::null(),
            record: std::ptr::null(),
            field_count: 0,
            fields: std::ptr::null(),
        })];
        let encoder = || Encoder {
            output: String::new(),
            indent: None,
            ensure_ascii: true,
        };
        for depth in 1..=MAX_DEPTH + 1 {
            // Boxes keep child metadata stable while the schema directory grows.
            let child = &**schemas.last().unwrap() as *const Schema;
            schemas.push(Box::new(Schema {
                kind: LIST,
                word_kind: if depth == 1 { 4 } else { 5 },
                element: child,
                record: std::ptr::null(),
                field_count: 0,
                fields: std::ptr::null(),
            }));
            if depth < MAX_DEPTH - 1 {
                continue;
            }
            let schema = schemas.last().unwrap();
            let text = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
            // The internal decoder builds this valid acyclic graph without the
            // public depth check, so encoding must enforce its own boundary.
            let value = decode(&text, schema).unwrap();
            let mut output = encoder();
            // SAFETY: the decoder owns every word with the live exact schema.
            let result = unsafe { output.value(value.word, schema, 0) };
            if depth <= MAX_DEPTH {
                assert!(result.is_ok());
                assert_eq!(output.output, text);
            } else {
                result.err().unwrap().raise();
                // SAFETY: raising publishes one owned native exception.
                unsafe {
                    let error = Rc::from_raw(relar_error_take());
                    assert_eq!(
                        relar_exception_matches(Rc::as_ptr(&error), Kind::ValueError as u32),
                        1
                    );
                }
                // A failed borrowed encoding must leave the complete child
                // graph usable. Its 128-container suffix is valid on its own.
                // SAFETY: this nonempty outer list owns its one generic child.
                let child = unsafe {
                    object(value.word as *const Object)
                        .0
                        .with(|storage| storage.sequence().values[0])
                };
                let mut retry = encoder();
                // SAFETY: `value` still roots the child and all its descendants.
                assert!(unsafe { retry.value(child, schema.child(), 0) }.is_ok());
                assert_eq!(retry.output, text[1..text.len() - 1]);
            }
        }
    }
}
