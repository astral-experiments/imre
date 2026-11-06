//! Dictionary views keep reading their owner while callbacks mutate it.

mod keys;

use std::ops::{Deref, DerefMut};

use super::{Id, Result};

/// A contiguous column of live dictionary entries with a reusable consumed prefix.
#[derive(Clone)]
struct EntryColumn<T> {
    data: Vec<T>,
    first: usize,
}

impl<T> From<Vec<T>> for EntryColumn<T> {
    fn from(data: Vec<T>) -> Self {
        Self { data, first: 0 }
    }
}

impl<T> Deref for EntryColumn<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.data[self.first..]
    }
}

impl<T> DerefMut for EntryColumn<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.data[self.first..]
    }
}

impl<T: Copy> EntryColumn<T> {
    /// Move the surviving entries into the prefix in insertion order.
    fn compact(&mut self) {
        let len = self.len();
        self.data.copy_within(self.first.., 0);
        self.data.truncate(len);
        self.first = 0;
    }

    /// Amortize prefix compaction and reuse allocated space if growth fails.
    fn try_reserve(
        &mut self,
        extra: usize,
    ) -> std::result::Result<(), std::collections::TryReserveError> {
        if self.data.capacity() - self.data.len() >= extra {
            return Ok(());
        }
        if self.first > 0 && self.first >= self.len() {
            self.compact();
        }
        if let Err(error) = self.data.try_reserve(extra) {
            if self.data.capacity() - self.len() < extra {
                return Err(error);
            }
            self.compact();
        }
        Ok(())
    }

    /// Append after the caller has reserved all of the table's columns.
    fn push(&mut self, value: T) {
        self.data.push(value);
    }

    /// Shift the shorter side so either end can be removed in constant time.
    fn remove(&mut self, index: usize) -> T {
        let physical = self.first + index;
        if index != 0 && index >= self.len() / 2 {
            return self.data.remove(physical);
        }
        let value = self.data[physical];
        if index != 0 {
            self.data.copy_within(self.first..physical, self.first + 1);
        }
        self.first += 1;
        if self.first == self.data.len() {
            self.clear();
        }
        value
    }

    /// Retain the allocation when the table is emptied.
    fn clear(&mut self) {
        self.data.clear();
        self.first = 0;
    }
}

#[derive(Clone, Default)]
struct Order {
    // Explicit dict descriptors can remove a value while the order node still owns its key.
    keys: Vec<(Id, i64)>,
    // Each live base-dict entry may have an associated order node.
    by_dense: Vec<Option<usize>>,
    revision: usize,
}

#[derive(Clone)]
pub(super) struct Entries {
    values: EntryColumn<(Id, Id)>,
    // Dense storage keeps one position per live key after a deletion.
    positions: Option<EntryColumn<usize>>,
    index: Option<keys::Index>,
    order: Option<allocator_api2::boxed::Box<Order>>,
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
            values: values.into(),
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
    type IntoIter = std::iter::Skip<std::vec::IntoIter<Self::Item>>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.data.into_iter().skip(self.values.first)
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
                .by_dense
                .try_reserve(1)
                .map_err(|_| crate::Error::memory_error())?;
        }
        self.values
            .try_reserve(1)
            .map_err(|_| crate::Error::memory_error())?;
        let index = self.index.as_mut().expect("indexed dictionary");
        index.reserve_hash()?;
        // An existing iterator must keep its insertion position if growth fails.
        if self.next == self.usable {
            let usable = usable_slots(self.values.len() * 3);
            index.compact((usable + 1).next_power_of_two())?;
            self.usable = usable;
            self.next = self.values.len();
            self.positions = None;
        }
        if let Some(positions) = &mut self.positions {
            positions
                .try_reserve(1)
                .map_err(|_| crate::Error::memory_error())?;
            positions.push(self.next);
        }
        index.push(hash, self.next);
        self.next += 1;
        self.values.push(value);
        if let Some(order) = &mut self.order {
            order.by_dense.push(None);
        }
        Ok(())
    }

    /// First insertion position while the live entries form a contiguous suffix.
    fn contiguous_start(&self) -> usize {
        self.next - self.len()
    }

    /// Prepare insertion positions before an externally visible removal.
    pub(super) fn prepare_removal(&mut self) -> Result<()> {
        if self.positions.is_none() {
            let mut positions = Vec::new();
            positions
                .try_reserve_exact(self.len())
                .map_err(|_| crate::Error::memory_error())?;
            positions.extend(self.contiguous_start()..self.next);
            self.positions = Some(positions.into());
        }
        Ok(())
    }

    pub(super) fn remove(&mut self, index: usize) -> Result<(Id, Id)> {
        let position = if index == 0 && self.positions.is_none() {
            self.contiguous_start()
        } else {
            self.prepare_removal()?;
            self.positions
                .as_mut()
                .expect("prepared positions")
                .remove(index)
        };
        if let Some(keys) = &mut self.index {
            keys.remove(index, position);
        }
        if let Some(order) = &mut self.order {
            order.by_dense.remove(index);
        }
        Ok(self.values.remove(index))
    }

    /// Reserved table buffers excluding Python key and value referents.
    pub(super) fn table_bytes(&self) -> usize {
        self.values.data.capacity() * size_of::<(Id, Id)>()
            + self
                .positions
                .as_ref()
                .map_or(0, |p| p.data.capacity() * size_of::<usize>())
            + self.index.as_ref().map_or(0, keys::Index::buffer_bytes)
    }

    pub(super) fn order_bytes(&self) -> usize {
        self.order.as_ref().map_or(0, |order| {
            size_of::<Order>()
                + order.keys.capacity() * size_of::<(Id, i64)>()
                + order.by_dense.capacity() * size_of::<Option<usize>>()
        })
    }

    /// The accelerated OrderedDict's key nodes, independent of base dict entries.
    pub(super) fn ordered_nodes(&self) -> &[(Id, i64)] {
        self.order.as_ref().map_or(&[], |order| &order.keys)
    }

    pub(super) fn ordered_rank(&self, dense: usize) -> Option<usize> {
        self.order.as_ref().and_then(|order| order.by_dense[dense])
    }

    fn reserve_ordered_node(&mut self) -> Result<()> {
        if self.order.is_none() {
            let mut by_dense = Vec::new();
            by_dense
                .try_reserve_exact(self.len())
                .map_err(|_| crate::Error::memory_error())?;
            by_dense.resize(self.len(), None);
            self.order = Some(
                allocator_api2::boxed::Box::try_new(Order {
                    by_dense,
                    ..Order::default()
                })
                .map_err(|_| crate::Error::memory_error())?,
            );
        }
        self.order
            .as_mut()
            .unwrap()
            .keys
            .try_reserve(1)
            .map_err(|_| crate::Error::memory_error().into())
    }

    fn link_ordered_key(&mut self, dense: usize, key: Id, hash: i64) {
        let order = self.order.as_mut().expect("reserved ordered node");
        order.by_dense[dense] = Some(order.keys.len());
        order.keys.push((key, hash));
        order.revision = order.revision.wrapping_add(1);
    }

    fn unlink_ordered_node(&mut self, dense: usize, rank: usize) -> Id {
        let order = self.order.as_mut().expect("live ordered node");
        order.by_dense[dense] = None;
        let (key, _) = order.keys.remove(rank);
        for other in order.by_dense.iter_mut().flatten() {
            if *other > rank {
                *other -= 1;
            }
        }
        order.revision = order.revision.wrapping_add(1);
        key
    }

    fn move_to_end(&mut self, rank: usize, last: bool) {
        let order = self.order.as_mut().expect("live ordered key");
        let end = if last { order.keys.len() - 1 } else { 0 };
        if rank == end {
            return;
        }
        if last {
            order.keys[rank..].rotate_left(1);
        } else {
            order.keys[..=rank].rotate_right(1);
        }
        for other in order.by_dense.iter_mut().flatten() {
            if *other == rank {
                *other = end;
            } else if last && *other > rank {
                *other -= 1;
            } else if !last && *other < rank {
                *other += 1;
            }
        }
        order.revision = order.revision.wrapping_add(1);
    }

    pub(super) fn insertion_end(&self) -> usize {
        self.next
    }

    /// Return the last live entry whose insertion position is below the cursor.
    pub(super) fn before(&self, end: usize) -> Option<(usize, Id, Id)> {
        let index = match &self.positions {
            Some(positions) => positions.partition_point(|&pos| pos < end),
            None => end.saturating_sub(self.contiguous_start()).min(self.len()),
        }
        .checked_sub(1)?;
        let position = self.positions.as_ref().map_or_else(
            || self.contiguous_start() + index,
            |positions| positions[index],
        );
        let (key, value) = self.values[index];
        Some((position, key, value))
    }

    /// Return the next live entry at or beyond the iterator's insertion position.
    pub(super) fn at(&self, position: usize) -> Option<(usize, Id, Id)> {
        let (index, position) = if let Some(positions) = &self.positions {
            let index = positions.partition_point(|&p| p < position);
            (index, *positions.get(index)?)
        } else {
            let first = self.contiguous_start();
            let position = position.max(first);
            (position - first, position)
        };
        self.values
            .get(index)
            .map(|&(key, value)| (position, key, value))
    }
}

#[cfg(test)]
mod tests {
    use super::{Entries, EntryColumn};

    #[test]
    fn compaction_reuses_the_consumed_prefix() {
        let mut column = EntryColumn::from((0..64).collect::<Vec<_>>());
        let allocation = column.data.as_ptr();
        for value in 0..40 {
            assert_eq!(column.remove(0), value);
        }
        column.try_reserve(1).unwrap();
        column.push(64);
        assert_eq!(column.data.as_ptr(), allocation);
        assert_eq!(column.first, 0);
        assert_eq!(&*column, &(40..65).collect::<Vec<_>>());
    }

    #[test]
    fn positions_and_clear_only_expose_surviving_entries() {
        let mut entries = Entries::from(vec![(10, 20), (11, 21), (12, 22), (13, 23)]);
        assert_eq!(entries.remove(0).unwrap(), (10, 20));
        assert!(entries.positions.is_none());
        assert_eq!(entries.at(0), Some((1, 11, 21)));
        assert_eq!(entries.before(1), None);
        assert_eq!(entries.remove(1).unwrap(), (12, 22));
        assert_eq!(entries.at(0), Some((1, 11, 21)));
        assert_eq!(entries.before(4), Some((3, 13, 23)));
        assert_eq!(
            entries.clone().into_iter().collect::<Vec<_>>(),
            [(11, 21), (13, 23)]
        );
        assert_eq!(entries.clear(), [(11, 21), (13, 23)]);
        assert!(entries.is_empty());
    }
}
