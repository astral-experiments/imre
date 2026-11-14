//! Dictionary views keep reading their owner while callbacks mutate it.

mod keys;

use std::ops::{Deref, DerefMut};

use super::{Id, Result};

// No Vec can contain a node at this index.
const NO_NODE: usize = usize::MAX;

#[derive(Clone, Copy)]
struct OrderNode {
    key: Id,
    hash: i64,
    previous: usize,
    next: usize,
}

#[derive(Clone)]
struct Order {
    // A dict descriptor can delete the base entry while its order node still owns the key.
    nodes: Vec<OrderNode>,
    by_dense: Vec<usize>,
    first: usize,
    last: usize,
    free: usize,
    len: usize,
    revision: usize,
}

impl Default for Order {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            by_dense: Vec::new(),
            first: NO_NODE,
            last: NO_NODE,
            free: NO_NODE,
            len: 0,
            revision: 0,
        }
    }
}

impl Order {
    /// Join two neighbors, treating a missing neighbor as the corresponding end.
    fn join(&mut self, before: usize, after: usize) {
        if before == NO_NODE {
            self.first = after;
        } else {
            self.nodes[before].next = after;
        }
        if after == NO_NODE {
            self.last = before;
        } else {
            self.nodes[after].previous = before;
        }
    }

    /// Attach a detached node to either end without moving its key or changing its index.
    fn attach(&mut self, node: usize, last: bool) {
        let (before, after) = if last {
            (self.last, NO_NODE)
        } else {
            (NO_NODE, self.first)
        };
        self.join(before, node);
        self.join(node, after);
    }

    /// Visit only live order nodes, including keys absent from the base dict.
    fn keys(&self) -> impl Iterator<Item = Id> + '_ {
        let mut cursor = self.first;
        std::iter::from_fn(move || {
            let node = self.nodes.get(cursor)?;
            cursor = node.next;
            Some(node.key)
        })
    }
}

#[derive(Clone)]
pub(super) struct Entries {
    values: Vec<(Id, Id)>,
    // Dense storage keeps one position per live key after a deletion.
    positions: Option<Vec<usize>>,
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
            order.by_dense.push(NO_NODE);
        }
        Ok(())
    }

    pub(super) fn prepare_removal(&mut self) -> Result<&mut Vec<usize>> {
        Ok(match &mut self.positions {
            Some(positions) => positions,
            vacant @ None => {
                let mut positions = Vec::new();
                positions
                    .try_reserve_exact(self.next)
                    .map_err(|_| crate::Error::memory_error())?;
                positions.extend(0..self.next);
                vacant.insert(positions)
            }
        })
    }

    pub(super) fn remove(&mut self, index: usize) -> Result<(Id, Id)> {
        let positions = self.prepare_removal()?;
        let position = positions.remove(index);
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
        self.values.capacity() * size_of::<(Id, Id)>()
            + self
                .positions
                .as_ref()
                .map_or(0, |p| p.capacity() * size_of::<usize>())
            + self.index.as_ref().map_or(0, keys::Index::buffer_bytes)
    }

    pub(super) fn order_bytes(&self) -> usize {
        self.order.as_ref().map_or(0, |order| {
            size_of::<Order>()
                + order.nodes.capacity() * size_of::<OrderNode>()
                + order.by_dense.capacity() * size_of::<usize>()
        })
    }

    pub(super) fn order_len(&self) -> usize {
        self.order.as_ref().map_or(0, |order| order.len)
    }

    /// Return the first node for a forward or reverse traversal of the independent order.
    pub(super) fn order_start(&self, reverse: bool) -> usize {
        self.order.as_ref().map_or(
            NO_NODE,
            |order| if reverse { order.last } else { order.first },
        )
    }

    /// Read a node and its successor without holding a borrow across a Python callback.
    pub(super) fn order_entry(&self, position: usize, reverse: bool) -> Option<(usize, Id, i64)> {
        let node = self.order.as_ref()?.nodes.get(position)?;
        if node.previous == position {
            return None;
        } // Vacant node on the free list.
        Some((
            if reverse { node.previous } else { node.next },
            node.key,
            node.hash,
        ))
    }

    pub(super) fn ordered_keys(&self) -> impl Iterator<Item = Id> + '_ {
        self.order.iter().flat_map(|order| order.keys())
    }

    pub(super) fn order_node(&self, dense: usize) -> Option<usize> {
        let node = self.order.as_ref()?.by_dense[dense];
        (node != NO_NODE).then_some(node)
    }

    fn reserve_ordered_node(&mut self) -> Result<()> {
        if self.order.is_none() {
            let mut by_dense = Vec::new();
            by_dense
                .try_reserve_exact(self.len())
                .map_err(|_| crate::Error::memory_error())?;
            by_dense.resize(self.len(), NO_NODE);
            self.order = Some(
                allocator_api2::boxed::Box::try_new(Order {
                    by_dense,
                    ..Order::default()
                })
                .map_err(|_| crate::Error::memory_error())?,
            );
        }
        let order = self.order.as_mut().expect("allocated order");
        if order.free == NO_NODE {
            order
                .nodes
                .try_reserve(1)
                .map_err(|_| crate::Error::memory_error())?;
        }
        Ok(())
    }

    fn link_ordered_key(&mut self, dense: usize, key: Id, hash: i64) {
        let order = self.order.as_mut().expect("reserved ordered node");
        let node = OrderNode {
            key,
            hash,
            previous: NO_NODE,
            next: NO_NODE,
        };
        let index = if order.free == NO_NODE {
            let index = order.nodes.len();
            order.nodes.push(node);
            index
        } else {
            let index = order.free;
            order.free = order.nodes[index].next;
            order.nodes[index] = node;
            index
        };
        order.by_dense[dense] = index;
        order.attach(index, true);
        order.len += 1;
        order.revision = order.revision.wrapping_add(1);
    }

    fn unlink_ordered_node(&mut self, dense: usize, index: usize) -> Id {
        let order = self.order.as_mut().expect("live ordered node");
        let node = order.nodes[index];
        order.join(node.previous, node.next);
        order.by_dense[dense] = NO_NODE;
        order.nodes[index] = OrderNode {
            key: super::NONE,
            hash: 0,
            previous: index,
            next: order.free,
        };
        order.free = index;
        order.len -= 1;
        order.revision = order.revision.wrapping_add(1);
        node.key
    }

    fn move_to_end(&mut self, index: usize, last: bool) {
        let order = self.order.as_mut().expect("live ordered key");
        if index == if last { order.last } else { order.first } {
            return;
        }
        let node = order.nodes[index];
        order.join(node.previous, node.next);
        order.attach(index, last);
        order.revision = order.revision.wrapping_add(1);
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
