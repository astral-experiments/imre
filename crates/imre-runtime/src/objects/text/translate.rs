//! Ordinal lookup for exact built-in dictionaries: a miss copies and None deletes.

use crate::StrOwner;
use num_traits::ToPrimitive;

use super::super::{mappings::Mapping, object, unions, words::NULLABLE};
use super::{Error, Int, Kind, Object, Result, Str, finish, owned_string};

#[derive(Clone, Copy)]
enum Piece<'a> {
    Character(char),
    Text(&'a str),
}

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
            // A closed str-or-int result boxes even small integers for its tag bits.
            match value.as_small().or_else(|| value.big().to_i64()) {
                Some(0xd800..=0xdfff) => Err(Error::new(
                    Kind::ValueError,
                    "surrogates are outside the native string domain",
                )),
                Some(value @ 0..=0x10ffff) => Ok(Piece::Character(
                    char::from_u32(value as u32).expect("validated Unicode scalar"),
                )),
                _ => Err(Error::new(
                    Kind::ValueError,
                    "character mapping must be in range(0x110000)",
                )),
            }
        }
        3 => {
            // SAFETY: the exact string is borrowed only while its table is rooted.
            Ok(Piece::Text(unsafe { &*(word as *const Str) }))
        }
        _ => Err(Error::new(
            Kind::TypeError,
            "character mapping must return integer, None or str",
        )),
    }
}

fn translate(source: &str, table: &Mapping) -> Result<*const Str> {
    let mut result = Str::try_with_capacity(source.len()).map_err(|_| Error::memory_error())?;
    let mut ascii = [None; 128];
    for ch in source.chars() {
        let piece = if ch.is_ascii() {
            let slot = &mut ascii[ch as usize];
            match *slot {
                Some(piece) => piece,
                None => *slot.insert(replacement(table, ch)?),
            }
        } else {
            replacement(table, ch)?
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
    let owner = if result.len() <= 2 {
        owned_string(&result)
    } else {
        StrOwner::new(result)
    };
    Ok(StrOwner::into_raw(owner))
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
            .with(|storage| translate(&*source, storage.dictionary()))
    };
    finish(result, std::ptr::null())
}
