//! Dictionary views keep reading their owner while callbacks mutate it.

mod keys;

use std::ops::{Deref, DerefMut};

use super::Id;

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
    /// Copy a dense table with its cached key hashes and insertion positions.
    pub(super) fn clone_dense(&self) -> Option<Self> {
        (self.len() >= self.next.saturating_mul(2) / 3).then(|| self.clone())
    }

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

    pub(super) fn remove(&mut self, index: usize) -> (Id, Id) {
        let position = self.positions.as_ref().map_or(index, |p| p[index]);
        if let Some(keys) = &mut self.index {
            keys.remove(index, position);
        }
        self.positions
            .get_or_insert_with(|| (0..self.next).collect())
            .remove(index);
        self.values.remove(index)
    }

    pub(super) fn insertion_end(&self) -> usize {
        self.next
    }

    /// Return the last live entry whose insertion position is below the cursor.
    pub(super) fn before(&self, end: usize) -> Option<(usize, Id, Id)> {
        let index = match &self.positions {
            Some(positions) => positions.partition_point(|&pos| pos < end),
            None => end.min(self.values.len()),
        }
        .checked_sub(1)?;
        let position = self
            .positions
            .as_ref()
            .map_or(index, |positions| positions[index]);
        let (key, value) = self.values[index];
        Some((position, key, value))
    }

    /// Return the next live entry at or beyond the iterator's insertion position.
    pub(super) fn at(&self, position: usize) -> Option<(usize, Id, Id)> {
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
