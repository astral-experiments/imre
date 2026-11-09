//! Translate between Python character offsets and positions in UTF-8 strings.

use super::{Heap, Id, NONE, SlotShape, Value};

#[derive(Clone, Copy, Default)]
pub(super) struct TextPosition {
    pub(super) slot: Id,
    character: usize,
    byte: usize,
}

impl Heap {
    /// Resume a seek only for an immutable string whose slot has not been recycled.
    fn text_position(&self, slot: Id, text: &str) -> TextPosition {
        if text.len() < 256 || self.slot_shapes[slot as usize] != SlotShape::ScalarLeaf {
            return TextPosition::default();
        }
        let cursor = self.text_position.get();
        if cursor.slot == slot {
            cursor
        } else {
            TextPosition {
                slot,
                ..TextPosition::default()
            }
        }
    }

    fn remember_text_position(&self, slot: Id, character: usize, byte: usize) {
        if slot != NONE {
            self.text_position.set(TextPosition {
                slot,
                character,
                byte,
            });
        }
    }

    /// Find a character boundary, including the position just past the last character.
    pub(in crate::dynamic) fn string_byte_offset(&self, slot: Id, index: usize) -> Option<usize> {
        let Value::Str(text) = self.get(slot) else {
            return None;
        };
        if index > text.len() {
            return None;
        }
        if index == 0 || self.is_ascii_str(slot) {
            return Some(index);
        }
        let cursor = self.text_position(slot, text);
        let byte = if index == cursor.character {
            cursor.byte
        } else if index > cursor.character {
            let mut chars = text[cursor.byte..].chars();
            chars.nth(index - cursor.character - 1)?;
            text.len() - chars.as_str().len()
        } else if cursor.character - index < index {
            let mut chars = text[..cursor.byte].chars();
            chars.nth_back(cursor.character - index - 1)?;
            chars.as_str().len()
        } else {
            let mut chars = text.chars();
            chars.nth(index - 1)?;
            text.len() - chars.as_str().len()
        };
        self.remember_text_position(cursor.slot, index, byte);
        Some(byte)
    }

    /// Find the Python index of a UTF-8 boundary in the same stored string.
    pub(in crate::dynamic) fn string_character_offset(
        &self,
        slot: Id,
        byte: usize,
    ) -> Option<usize> {
        let Value::Str(text) = self.get(slot) else {
            return None;
        };
        if !text.is_char_boundary(byte) {
            return None;
        }
        if byte == 0 || self.is_ascii_str(slot) {
            return Some(byte);
        }
        let cursor = self.text_position(slot, text);
        let character = if byte >= cursor.byte {
            cursor.character + text[cursor.byte..byte].chars().count()
        } else if cursor.byte - byte < byte {
            cursor.character - text[byte..cursor.byte].chars().count()
        } else {
            text[..byte].chars().count()
        };
        self.remember_text_position(cursor.slot, character, byte);
        Some(character)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeks_across_multibyte_characters_and_recycled_slots() {
        let mut heap = Heap::new();
        let text = "é𐀀a".repeat(130);
        let id = heap.alloc(Value::Str(text.clone().into()));
        let boundaries: Vec<_> = text
            .char_indices()
            .map(|(i, _)| i)
            .chain([text.len()])
            .collect();
        for index in [0, 20, 200, 201, 194, 2, 390, 389, 16] {
            assert_eq!(heap.string_byte_offset(id, index), Some(boundaries[index]));
            assert_eq!(
                heap.string_character_offset(id, boundaries[index]),
                Some(index)
            );
        }
        assert_eq!(heap.string_byte_offset(id, 391), None);
        assert_eq!(heap.string_character_offset(id, 1), None);
        assert_eq!(heap.string_character_offset(id, text.len() + 1), None);
        heap.release_temporaries_since(0);
        heap.collect(std::iter::empty());
        let replacement = heap.alloc(Value::Str("界".repeat(190).into()));
        assert_eq!(replacement, id);
        assert_eq!(heap.string_byte_offset(replacement, 16), Some(48));
        *heap.get_mut(replacement) = Value::Str("ß".repeat(190).into());
        assert_eq!(heap.string_character_offset(replacement, 48), Some(24));
    }
}
