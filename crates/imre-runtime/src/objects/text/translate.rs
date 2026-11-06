//! String translation by ordinal subscription. A miss copies the character; None deletes it.

use crate::StrOwner;

use super::super::{mappings::Mapping, object, unions, words::NULLABLE};
use super::{Error, Int, Kind, Object, Result, Str, finish, owned_string};

#[derive(Clone, Copy)]
pub(crate) enum Piece<T> {
    Delete,
    Character(char),
    Text(T),
}

impl<T> Piece<T> {
    /// Accepts an ordinal in Imre's native string domain.
    pub(crate) fn ordinal(value: &Int) -> Result<Self> {
        match value.to_i64() {
            Some(0xd800..=0xdfff) => Err(Error::new(
                Kind::ValueError,
                "surrogates are outside the native string domain",
            )),
            Some(value @ 0..=0x10ffff) => Ok(Self::Character(
                char::from_u32(value as u32).expect("validated Unicode scalar"),
            )),
            _ => Err(Error::new(
                Kind::ValueError,
                "character mapping must be in range(0x110000)",
            )),
        }
    }
}

pub(crate) const INVALID_VALUE: &str = "character mapping must return integer, None or str";

fn replacement(table: &Mapping, ch: char) -> Result<Piece<&str>> {
    let Some((mut kind, mut word)) = table.translation_entry(ch) else {
        return Ok(Piece::Character(ch));
    };
    let nullable = kind.raw() & NULLABLE != 0;
    if kind.base() == 0 || (nullable && word == 0) {
        return Ok(Piece::Delete);
    }
    if kind.base() == 9 {
        (word, kind) = unions::member(word);
    }
    match kind.base() {
        1 => {
            // Unwrapping a tagged union returns a bare bool even in a nullable table.
            let false_word = usize::from(kind.raw() & NULLABLE != 0);
            Ok(Piece::Character(if word == false_word {
                '\0'
            } else {
                '\x01'
            }))
        }
        2 => {
            // SAFETY: the table keeps this integer live for the whole lookup.
            let value = unsafe { Int::borrow(word) };
            Piece::ordinal(&value)
        }
        3 => {
            // SAFETY: the exact string is borrowed only while its table is rooted.
            Ok(Piece::Text(unsafe { &*(word as *const Str) }.as_str()))
        }
        _ => Err(Error::new(Kind::TypeError, INVALID_VALUE)),
    }
}

/// Write the one-byte ASCII replacements directly into the reserved output.
/// Return the remaining input when a replacement needs the general UTF-8 writer.
fn ascii_prefix<'s, 'a, E: From<Error>>(
    source: &'s str,
    output: &mut Str,
    lookup: &impl Fn(char) -> std::result::Result<Piece<&'a str>, E>,
) -> std::result::Result<&'s str, E> {
    const UNSEEN: u8 = u8::MAX;
    const DELETE: u8 = 128;
    let mut table = [UNSEEN; 256];
    let mut written = 0;
    let mut consumed = 0;
    // SAFETY: the caller reserves the whole ASCII source in an empty heap string.
    // Every included output byte is initialized below with a single ASCII replacement.
    let bytes = unsafe { output.spare_capacity_mut() };
    for byte in source.bytes() {
        let slot = &mut table[usize::from(byte)];
        if *slot == UNSEEN {
            *slot = match lookup(char::from(byte))? {
                Piece::Delete | Piece::Text("") => DELETE,
                Piece::Character(ch) if ch.is_ascii() => ch as u8,
                Piece::Text(text) if text.len() == 1 => text.as_bytes()[0],
                _ => break,
            };
        }
        if *slot != DELETE {
            bytes[written].write(*slot);
            written += 1;
        }
        consumed += 1;
    }
    // SAFETY: the initialized prefix consists entirely of ASCII bytes.
    unsafe { output.set_len(written) };
    Ok(&source[consumed..])
}

/// Borrowed, callback-free lookups may reuse ASCII replacements even in mixed Unicode text.
pub(crate) fn borrowed_text<'a, E: From<Error>>(
    source: &str,
    lookup: impl Fn(char) -> std::result::Result<Piece<&'a str>, E>,
) -> std::result::Result<Str, E> {
    let mut output = Str::try_with_capacity(source.len()).map_err(|_| Error::memory_error())?;
    let source = if source.len() >= 256 && source.is_ascii() {
        ascii_prefix(source, &mut output, &lookup)?
    } else {
        source
    };
    if source.is_empty() {
        return Ok(output);
    }
    let mut ascii = [None; 128];
    write_translation(source, output, |ch| {
        if !ch.is_ascii() {
            return lookup(ch);
        }
        let slot = &mut ascii[ch as usize];
        Ok(match *slot {
            Some(piece) => piece,
            None => *slot.insert(lookup(ch)?),
        })
    })
}

/// Preserve the observable order of subscriptions to a Python translation table.
pub(crate) fn text<T: AsRef<str>, E: From<Error>>(
    source: &str,
    mut lookup: impl FnMut(char) -> std::result::Result<Piece<T>, E>,
) -> std::result::Result<Str, E> {
    let mut fast = source.is_ascii();
    let mut ascii = [None; 128];
    let mut deleting = false;
    let output = Str::try_with_capacity(source.len()).map_err(|_| Error::memory_error())?;
    write_translation(source, output, |ch| {
        if fast {
            // CPython caches one-byte ASCII replacements and deletions. A longer
            // replacement switches paths and queries this ordinal again.
            const DELETE: u8 = 128;
            let slot = &mut ascii[ch as usize];
            if slot.is_none() {
                *slot = match lookup(ch)? {
                    Piece::Delete => Some(DELETE),
                    Piece::Character(c) if c.is_ascii() => Some(c as u8),
                    Piece::Text(s) if s.as_ref().len() == 1 => Some(s.as_ref().as_bytes()[0]),
                    _ => None,
                };
            }
            if let Some(byte) = *slot {
                return Ok(if byte == DELETE {
                    Piece::Delete
                } else {
                    Piece::Character(char::from(byte))
                });
            }
            fast = false;
        }

        let mut piece = lookup(ch)?;
        if deleting && !matches!(piece, Piece::Delete) {
            // The lookup that ends a run of deletions only probes the next
            // character. Subscribing again is observable on a Python table.
            piece = lookup(ch)?;
        }
        deleting = matches!(piece, Piece::Delete);
        Ok(piece)
    })
}

/// Append each replacement while keeping output growth fallible.
fn write_translation<T: AsRef<str>, E: From<Error>>(
    source: &str,
    mut result: Str,
    mut lookup: impl FnMut(char) -> std::result::Result<Piece<T>, E>,
) -> std::result::Result<Str, E> {
    for ch in source.chars() {
        let piece = lookup(ch)?;
        let mut utf8 = [0; 4];
        let text = match &piece {
            Piece::Delete => continue,
            Piece::Character(value) => value.encode_utf8(&mut utf8),
            Piece::Text(text) => text.as_ref(),
        };
        if text.len() > result.capacity() - result.len() {
            result
                .try_reserve(text.len())
                .map_err(|_| Error::memory_error())?;
        }
        result.push_str(text);
    }
    Ok(result)
}

/// # Safety
/// Source and an exact plain dictionary remain rooted; no user callback runs while borrowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_translate(
    source: *const Str,
    table: *const Object,
) -> *const Str {
    // SAFETY: both owners stay live and the dictionary cannot mutate during translation.
    let result = unsafe {
        object(table)
            .0
            .with(|storage| borrowed_text(&*source, |ch| replacement(storage.dictionary(), ch)))
    };
    finish(
        result.map(|text| {
            let owner = if text.len() <= 2 {
                owned_string(&text)
            } else {
                StrOwner::new(text)
            };
            StrOwner::into_raw(owner)
        }),
        std::ptr::null(),
    )
}
