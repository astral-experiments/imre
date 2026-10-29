//! Dictionary views keep reading their owner while callbacks mutate it.

mod keys;

use std::ops::{Deref, DerefMut};

use super::{Error, Id, Result};

#[derive(Clone)]
struct Order {
    indices: Vec<usize>,
    revision: usize,
}

#[derive(Clone)]
pub(super) struct Entries {
    values: Vec<(Id, Id)>,
    // Dense storage keeps one position per live key after a deletion.
    positions: Option<Vec<usize>>,
    index: Option<keys::Index>,
    order: Option<Order>,
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
            order: None,
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
    pub(super) fn clone_dense(&self) -> super::Result<Option<Self>> {
        (self.len() >= self.next.saturating_mul(2) / 3)
            .then(|| self.try_clone())
            .transpose()
    }

    pub(super) fn push(&mut self, value: (Id, Id), hash: i64) -> Result<()> {
        if let Some(order) = &mut self.order {
            order
                .indices
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            order.indices.push(self.values.len());
        }
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
        Ok(())
    }

    pub(super) fn remove(&mut self, index: usize) -> (Id, Id) {
        let position = self.positions.as_ref().map_or(index, |p| p[index]);
        if let Some(keys) = &mut self.index {
            keys.remove(index, position);
        }
        self.positions
            .get_or_insert_with(|| (0..self.next).collect())
            .remove(index);
        if let Some(order) = &mut self.order {
            order.indices.retain_mut(|dense| {
                if *dense == index {
                    return false;
                }
                if *dense > index {
                    *dense -= 1;
                }
                true
            });
        }
        self.values.remove(index)
    }

    /// Map the ordered view's rank to an entry in the underlying dict.
    pub(super) fn ordered_dense(&self, rank: usize) -> usize {
        self.order
            .as_ref()
            .map_or(rank, |order| order.indices[rank])
    }

    /// Move an existing key without changing the underlying dict's insertion order.
    fn move_to_end(&mut self, dense: usize, last: bool) -> Result<()> {
        let endpoint = if last { self.len() - 1 } else { 0 };
        if self.ordered_dense(endpoint) == dense {
            return Ok(());
        }
        if self.order.is_none() {
            let mut indices = Vec::new();
            indices
                .try_reserve_exact(self.len())
                .map_err(|_| Error::new("MemoryError", ""))?;
            indices.extend(0..self.len());
            self.order = Some(Order {
                indices,
                revision: 0,
            });
        }
        let order = self.order.as_mut().expect("ordered entries");
        let rank = order
            .indices
            .iter()
            .position(|&i| i == dense)
            .expect("live ordered key");
        if last {
            order.indices[rank..].rotate_left(1);
        } else {
            order.indices[..=rank].rotate_right(1);
        }
        order.revision = order.revision.wrapping_add(1);
        Ok(())
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
