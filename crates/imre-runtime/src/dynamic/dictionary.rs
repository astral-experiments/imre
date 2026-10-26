//! Dictionary views keep reading their owner while callbacks mutate it.

mod keys;

use std::ops::{Deref, DerefMut};

use super::value::DictViewKind;
use super::{Context, Error, Id, Result, Value};

#[derive(Clone)]
pub(super) struct Entries {
    values: Vec<(Id, Id)>,
    // Dense storage keeps one position per live key after a deletion.
    positions: Option<Vec<usize>>,
    index: Option<keys::Index>,
    next: usize,
    usable: usize,
}

fn usable_slots(minimum_size: usize) -> usize {
    let slots = minimum_size.max(8).next_power_of_two();
    slots * 2 / 3
}

impl From<Vec<(Id, Id)>> for Entries {
    fn from(values: Vec<(Id, Id)>) -> Self {
        let next = values.len();
        Self {
            values,
            positions: None,
            index: None,
            next,
            usable: usable_slots(next + next.div_ceil(2)),
        }
    }
}

impl Default for Entries {
    fn default() -> Self {
        Vec::new().into()
    }
}

impl Deref for Entries {
    type Target = [(Id, Id)];

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl DerefMut for Entries {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values
    }
}

impl IntoIterator for Entries {
    type Item = (Id, Id);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

impl<'a> IntoIterator for &'a Entries {
    type Item = &'a (Id, Id);
    type IntoIter = std::slice::Iter<'a, (Id, Id)>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

impl Entries {
    pub(super) fn push(&mut self, value: (Id, Id), hash: i64) {
        // CPython compacts its entries when the index table runs out of usable slots.
        // Existing iterators keep their numerical cursor in the compacted table.
        if self.next == self.usable {
            self.usable = usable_slots(self.values.len() * 3);
            self.next = self.values.len();
            self.positions = None;
            if let Some(index) = &mut self.index {
                index.compact((self.usable + 1).next_power_of_two());
            }
        }
        if let Some(positions) = &mut self.positions {
            positions.push(self.next);
        }
        self.index
            .as_mut()
            .expect("indexed dictionary")
            .push(hash, self.next);
        self.next += 1;
        self.values.push(value);
    }

    pub(super) fn remove(&mut self, index: usize) {
        let position = self.positions.as_ref().map_or(index, |p| p[index]);
        if let Some(keys) = &mut self.index {
            keys.remove(index, position);
        }
        self.positions
            .get_or_insert_with(|| (0..self.next).collect())
            .remove(index);
        self.values.remove(index);
    }

    fn at(&self, position: usize) -> Option<(usize, Id, Id)> {
        let (index, position) = if let Some(positions) = &self.positions {
            let index = positions.partition_point(|&p| p < position);
            (index, *positions.get(index)?)
        } else {
            (position, position)
        };
        self.values
            .get(index)
            .map(|&(key, value)| (position, key, value))
    }
}

#[derive(Clone, Copy)]
pub(super) struct Cursor {
    pub(super) owner: Option<Id>,
    kind: DictViewKind,
    size: Option<usize>,
    position: usize,
    remaining: usize,
}

impl Context {
    pub(super) fn dictionary_iterator(&mut self, owner: Id, kind: DictViewKind) -> Id {
        let size = self.dict_entries(owner).expect("dictionary owner").len();
        self.alloc(Value::DictionaryIterator(Cursor {
            owner: Some(owner),
            kind,
            size: Some(size),
            position: 0,
            remaining: size,
        }))
    }

    pub(super) fn dictionary_entry(&mut self, id: Id) -> Result<Option<(usize, Id, Id)>> {
        let Value::DictionaryIterator(cursor) = *self.heap.get(id) else {
            unreachable!("dictionary iterator")
        };
        let Some(owner) = cursor.owner else {
            return Ok(None);
        };
        let storage = self.dict_storage(owner).expect("dictionary owner");
        let (Value::Dict(entries) | Value::DefaultDict { entries, .. }) = self.heap.get(storage)
        else {
            unreachable!("dictionary storage")
        };
        if cursor.size != Some(entries.len()) {
            let Value::DictionaryIterator(cursor) = self.heap.get_mut(id) else {
                unreachable!()
            };
            cursor.size = None;
            return Err(Error::new(
                "RuntimeError",
                "dictionary changed size during iteration",
            ));
        }
        let entry = entries.at(cursor.position);
        if entry.is_some() && cursor.remaining != 0 {
            return Ok(entry);
        }
        let Value::DictionaryIterator(cursor) = self.heap.get_mut(id) else {
            unreachable!()
        };
        cursor.owner = None;
        if entry.is_some() {
            return Err(Error::new(
                "RuntimeError",
                "dictionary keys changed during iteration",
            ));
        }
        Ok(None)
    }

    pub(super) fn next_dictionary_item(&mut self, id: Id) -> Result<Option<Id>> {
        let Some((position, key, value)) = self.dictionary_entry(id)? else {
            return Ok(None);
        };
        let Value::DictionaryIterator(cursor) = self.heap.get_mut(id) else {
            unreachable!()
        };
        cursor.position = position + 1;
        cursor.remaining -= 1;
        Ok(Some(match cursor.kind {
            DictViewKind::Keys => key,
            DictViewKind::Values => value,
            DictViewKind::Items => self.alloc(Value::Tuple(vec![key, value])),
        }))
    }

    pub(super) fn dictionary_hint(&self, cursor: &Cursor) -> usize {
        if let Some(owner) = cursor.owner
            && cursor.size == self.dict_entries(owner).map(<[_]>::len)
        {
            cursor.remaining
        } else {
            0
        }
    }
}
