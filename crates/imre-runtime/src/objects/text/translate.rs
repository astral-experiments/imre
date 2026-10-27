//! Ordinal lookup for exact built-in dictionaries: a miss copies and None deletes.

use crate::StrOwner;

use super::super::{mappings::Mapping, object, unions, words::NULLABLE};
use super::{Error, Int, Kind, Object, Result, Str, finish, owned_string};

#[derive(Clone, Copy)]
pub(crate) enum Piece<'a> {
    Character(char),
    Text(&'a str),
}

impl Piece<'_> {
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

fn replacement(table: &Mapping, ch: char) -> Result<Piece<'_>> {
    let Some((mut kind, mut word)) = table.translation_entry(ch) else {
        return Ok(Piece::Character(ch));
    };
    let nullable = kind.raw() & NULLABLE != 0;
    if kind.base() == 0 || (nullable && word == 0) {
        return Ok(Piece::Text(""));
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
            Ok(Piece::Text(unsafe { &*(word as *const Str) }))
        }
        _ => Err(Error::new(Kind::TypeError, INVALID_VALUE)),
    }
}

/// Looks up each input code point and appends its replacement to the result.
pub(crate) fn text<'a>(source: &str, lookup: impl Fn(char) -> Result<Piece<'a>>) -> Result<Str> {
    let mut result = Str::try_with_capacity(source.len()).map_err(|_| Error::memory_error())?;
    let mut ascii = [None; 128];
    for ch in source.chars() {
        let piece = if ch.is_ascii() {
            let slot = &mut ascii[ch as usize];
            match *slot {
                Some(piece) => piece,
                None => *slot.insert(lookup(ch)?),
            }
        } else {
            lookup(ch)?
        };
        let mut utf8 = [0; 4];
        let text = match piece {
            Piece::Character(value) => value.encode_utf8(&mut utf8),
            Piece::Text(text) => text,
        };
        result
            .try_reserve(text.len())
            .map_err(|_| Error::memory_error())?;
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
            .with(|storage| text(&*source, |ch| replacement(storage.dictionary(), ch)))
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
