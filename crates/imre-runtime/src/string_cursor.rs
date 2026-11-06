//! Reuse a scalar-to-byte position when native code repeatedly searches one string.

use std::cell::RefCell;

use crate::string_owner::WeakStr;
use crate::{Str, StrOwner};

// Short strings are cheaper to walk directly and need no cursor.
pub(crate) const MIN_BYTES: usize = 256;

thread_local! {
    static RECENT: RefCell<Option<Cursor>> = const { RefCell::new(None) };
}

struct Cursor {
    // A weak observer prevents address reuse and in-place mutation without keeping the text alive.
    owner: WeakStr,
    length: Option<usize>,
    scalar: usize,
    byte: usize,
}

impl Cursor {
    fn length(&mut self, text: &str) -> usize {
        *self.length.get_or_insert_with(|| text.chars().count())
    }

    /// Walk from the last known boundary, or restart if the beginning is closer.
    fn seek(&mut self, text: &str, target: usize) -> Option<usize> {
        let byte = if self.length == Some(text.len()) {
            (target <= text.len()).then_some(target)?
        } else if target == self.scalar {
            self.byte
        } else if target < self.scalar && self.scalar - target < target {
            text[..self.byte]
                .char_indices()
                .rev()
                .nth(self.scalar - target - 1)?
                .0
        } else {
            let (scalar, byte) = if target < self.scalar {
                (0, 0)
            } else {
                (self.scalar, self.byte)
            };
            let tail = &text[byte..];
            let steps = target - scalar;
            let prefix = tail.as_bytes().get(..steps)?;
            if prefix.is_ascii() {
                byte + steps
            } else {
                byte + tail
                    .char_indices()
                    .map(|(at, _)| at)
                    .chain([tail.len()])
                    .nth(steps)?
            }
        };
        self.scalar = target;
        self.byte = byte;
        Some(byte)
    }
}

fn cached<T>(text: &Str, use_cursor: impl FnOnce(&mut Cursor) -> T) -> Option<T> {
    if text.len() < MIN_BYTES {
        return None;
    }
    RECENT.with_borrow_mut(|recent| {
        recent
            .as_mut()
            .filter(|c| std::ptr::eq(c.owner.as_ptr(), text))
            .map(use_cursor)
    })
}

pub(crate) fn length(text: &Str) -> usize {
    cached(text, |c| c.length(text)).unwrap_or_else(|| text.chars().count())
}

pub(crate) fn character(text: &Str, index: isize) -> Option<char> {
    if index < 0 {
        return character_from_end(text, index.unsigned_abs());
    }
    cached(text, |c| {
        text[c.seek(text, index as usize)?..].chars().next()
    })
    .unwrap_or_else(|| text.chars().nth(index as usize))
}

/// Read a negative index using at most four bytes per scalar counted from the end.
pub(crate) fn character_from_end(text: &str, distance: usize) -> Option<char> {
    // A few reverse steps cost less than counting and indexing a suffix.
    if distance <= 8 {
        return text.chars().nth_back(distance.checked_sub(1)?);
    }
    if distance > text.len() {
        return None;
    }
    // The last `distance` scalars fit in this suffix even when they occupy four bytes each.
    let start = text.len().saturating_sub(distance.saturating_mul(4));
    let suffix = &text[text.ceil_char_boundary(start)..];
    let index = suffix.chars().count().checked_sub(distance)?;
    suffix.chars().nth(index)
}

/// A nonzero forward search seeds one cursor. Reads on other strings do not displace it.
pub(crate) fn find(text: &StrOwner, needle: &str, start: usize) -> Option<usize> {
    RECENT.with_borrow_mut(|recent| {
        let cursor = match recent {
            Some(cursor) if cursor.owner.as_ptr() == StrOwner::as_ptr(text) => cursor,
            entry => entry.insert(Cursor {
                owner: StrOwner::downgrade(text),
                length: None,
                scalar: 0,
                byte: 0,
            }),
        };
        let byte = cursor.seek(text, start)?;
        let remainder = &text[byte..];
        let offset = match needle.as_bytes() {
            [ascii] => remainder.find(char::from(*ascii)),
            _ => remainder.find(needle),
        }?;
        cursor.byte += offset;
        cursor.scalar += if cursor.length == Some(text.len()) {
            offset
        } else {
            text[byte..byte + offset].chars().count()
        };
        Some(cursor.scalar)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_indices_cover_every_utf8_boundary() {
        for unit in ["", "abc", "é狐🦊xy", "🦀🦀🦀🦀", "a\0é\u{301}界🦀"] {
            let text = unit.repeat(13);
            for distance in 0..=text.len() + 1 {
                let expected = distance
                    .checked_sub(1)
                    .and_then(|n| text.chars().nth_back(n));
                assert_eq!(character_from_end(&text, distance), expected);
            }
            assert_eq!(character_from_end(&text, usize::MAX), None);
        }
    }

    #[test]
    fn searches_and_indices_can_move_in_either_direction() {
        for unit in ["abc", "aé🦀", "e\u{301}\0"] {
            let text = StrOwner::new(Str::from(unit.repeat(400)));
            let chars: Vec<_> = text.chars().collect();
            for at in [1, 503, 87, 1199, 600, 1200, 1201, 0] {
                for needle in ["", "a", "é", "🦀", "absent"] {
                    let rest: String = chars.iter().skip(at).collect();
                    let expected = (at <= chars.len())
                        .then(|| rest.find(needle))
                        .flatten()
                        .map(|byte| at + rest[..byte].chars().count());
                    assert_eq!(find(&text, needle, at), expected);
                }
                assert_eq!(length(&text), chars.len());
                assert_eq!(character(&text, at as isize), chars.get(at).copied());
                assert_eq!(
                    character(&text, -(at as isize) - 1),
                    chars.iter().rev().nth(at).copied()
                );
            }
            let observer = StrOwner::downgrade(&text);
            drop(text);
            assert!(observer.upgrade().is_none());
        }
    }
}
