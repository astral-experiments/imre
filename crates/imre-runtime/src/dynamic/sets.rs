use super::{Context, Error, Id, Result, Value};

const EMPTY: usize = usize::MAX;

fn memory_error() -> Error {
    Error::new("MemoryError", "")
}

fn empty_table(size: usize) -> Result<Vec<usize>> {
    let mut slots = Vec::new();
    slots.try_reserve_exact(size).map_err(|_| memory_error())?;
    slots.resize(size, EMPTY);
    Ok(slots)
}

fn copy_slice<T: Copy>(values: &[T]) -> Result<Vec<T>> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(values.len())
        .map_err(|_| memory_error())?;
    result.extend_from_slice(values);
    Ok(result)
}

fn capacity(minimum: usize) -> Result<usize> {
    minimum
        .checked_add(1)
        .and_then(usize::checked_next_power_of_two)
        .map(|n| n.max(8))
        .ok_or_else(memory_error)
}

/// Probe consecutive slots where possible, then perturb the next starting slot.
/// Equality callbacks can observe collisions and their order.
struct Probe {
    base: usize,
    next: usize,
    linear: usize,
    perturb: usize,
    mask: usize,
}

impl Probe {
    fn new(hash: i64, size: usize) -> Self {
        let mask = size - 1;
        let base = (hash as usize) & mask;
        Self {
            base,
            next: base,
            linear: if base + 9 <= mask { 9 } else { 0 },
            perturb: hash as usize,
            mask,
        }
    }
}

impl Iterator for Probe {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        let position = self.next;
        if self.linear != 0 {
            self.next += 1;
            self.linear -= 1;
        } else {
            self.perturb >>= 5;
            self.base = self
                .base
                .wrapping_mul(5)
                .wrapping_add(1)
                .wrapping_add(self.perturb)
                & self.mask;
            self.next = self.base;
            self.linear = if self.base + 9 <= self.mask { 9 } else { 0 };
        }
        Some(position)
    }
}

/// Stored hashes belong to the set, so copying and growing it cannot call Python.
#[derive(Clone, Debug, Default)]
pub(super) struct SetIndex {
    hashes: Vec<i64>,
    slots: Vec<usize>,
}

impl SetIndex {
    fn size(&self) -> usize {
        self.slots.len().max(8)
    }

    fn position(&self, slot: usize) -> Option<usize> {
        self.slots.get(slot).copied().filter(|&pos| pos != EMPTY)
    }

    fn repopulate(&self, table: &mut [usize]) {
        for &pos in self.slots.iter().filter(|&&pos| pos != EMPTY) {
            let slot = Probe::new(self.hashes[pos], table.len())
                .find(|&i| table[i] == EMPTY)
                .expect("spare set slot");
            table[slot] = pos;
        }
    }

    fn bulk_size(count: usize) -> Result<usize> {
        if count >= 5 {
            capacity(count.checked_mul(2).ok_or_else(memory_error)?)
        } else {
            Ok(8)
        }
    }

    fn copied(&self) -> Result<Self> {
        let size = Self::bulk_size(self.hashes.len())?;
        let slots = if size == self.size() {
            copy_slice(&self.slots)?
        } else {
            let mut slots = empty_table(size)?;
            self.repopulate(&mut slots);
            slots
        };
        Ok(Self {
            hashes: copy_slice(&self.hashes)?,
            slots,
        })
    }

    fn insert(&mut self, items: &mut Vec<Id>, slot: usize, key: Id, hash: i64) -> Result<()> {
        let count = items.len() + 1;
        items.try_reserve(1).map_err(|_| memory_error())?;
        self.hashes.try_reserve(1).map_err(|_| memory_error())?;
        let mut grown = if count.saturating_mul(5) >= (self.size() - 1).saturating_mul(3) {
            Some(empty_table(capacity(
                count
                    .checked_mul(if count > 50_000 { 2 } else { 4 })
                    .ok_or_else(memory_error)?,
            )?)?)
        } else {
            None
        };
        if self.slots.is_empty() {
            self.slots = empty_table(8)?;
        }
        self.slots[slot] = items.len();
        items.push(key);
        self.hashes.push(hash);
        if let Some(ref mut slots) = grown {
            self.repopulate(slots);
        }
        if let Some(slots) = grown {
            self.slots = slots;
        }
        Ok(())
    }

    pub(super) fn frozen_hash(&self) -> i64 {
        let mut hash = 0_u64;
        for &lane in &self.hashes {
            let lane = lane as u64;
            hash ^= ((lane ^ 89_869_747) ^ (lane << 16)).wrapping_mul(3_644_798_167);
        }
        hash ^= (self.hashes.len() as u64 + 1).wrapping_mul(1_927_868_237);
        hash ^= (hash >> 11) ^ (hash >> 25);
        hash = hash.wrapping_mul(69_069).wrapping_add(907_133_923);
        if hash == u64::MAX {
            590_923_713
        } else {
            hash as i64
        }
    }
}

impl Context {
    pub(super) fn empty_set(&mut self, frozen: bool) -> Id {
        self.alloc(Value::Set {
            items: Vec::new(),
            index: SetIndex::default(),
            frozen,
        })
    }

    fn set_slot(&mut self, set: Id, key: Id, hash: i64) -> Result<(usize, bool)> {
        'restart: loop {
            let Value::Set { index, .. } = self.heap.get(set) else {
                return Err(Error::new("TypeError", "expected set"));
            };
            let size = index.size();
            for slot in Probe::new(hash, size) {
                let Value::Set { index, items, .. } = self.heap.get(set) else {
                    unreachable!()
                };
                let Some(pos) = index.position(slot) else {
                    return Ok((slot, false));
                };
                if index.hashes[pos] != hash {
                    continue;
                }
                let entry = items[pos];
                if entry == key {
                    return Ok((slot, true));
                }
                let mark = self.heap.temporary_mark();
                self.heap.keep(entry);
                let equal = self.keys_equal(entry, key)?;
                self.heap.release_temporaries(mark);
                let Value::Set { index, items, .. } = self.heap.get(set) else {
                    unreachable!()
                };
                if index.size() != size || index.position(slot) != Some(pos) || items[pos] != entry
                {
                    continue 'restart;
                }
                if equal {
                    return Ok((slot, true));
                }
            }
        }
    }

    pub(super) fn contains_set_key(
        &mut self,
        set: Id,
        key: Id,
        allow_mutable_key: bool,
    ) -> Result<bool> {
        self.heap.keep(set);
        self.heap.keep(key);
        let hash = if allow_mutable_key
            && let Value::Set {
                frozen: false,
                index,
                ..
            } = self.heap.get(key)
        {
            index.frozen_hash()
        } else {
            self.hash_key(key)
                .map_err(|e| self.key_hash_error(key, e, "set element"))?
        };
        Ok(self.set_slot(set, key, hash)?.1)
    }

    pub(super) fn insert_set_key(&mut self, set: Id, key: Id, known: Option<i64>) -> Result<()> {
        self.heap.keep(set);
        self.heap.keep(key);
        let hash = match known {
            Some(hash) => hash,
            None => self
                .hash_key(key)
                .map_err(|e| self.key_hash_error(key, e, "set element"))?,
        };
        let (slot, present) = self.set_slot(set, key, hash)?;
        if !present && let Value::Set { items, index, .. } = self.heap.get_mut(set) {
            index.insert(items, slot, key, hash)?;
        }
        Ok(())
    }

    /// Read the next live entry in table order, including its insertion hash.
    fn next_set_key(&self, set: Id, position: usize) -> Option<(usize, Id, i64)> {
        let Value::Set { index, items, .. } = self.heap.get(set) else {
            unreachable!()
        };
        (position..index.slots.len()).find_map(|slot| {
            let item = index.position(slot)?;
            Some((slot + 1, items[item], index.hashes[item]))
        })
    }

    /// Compare sets using the hashes already stored by the source.
    pub(super) fn set_is_subset(
        &mut self,
        source: Id,
        target: Id,
        equal_length: bool,
    ) -> Result<Option<bool>> {
        let (Value::Set { items: a, .. }, Value::Set { items: b, .. }) =
            (self.heap.get(source), self.heap.get(target))
        else {
            return Ok(None);
        };
        if a.len() > b.len() || (equal_length && a.len() != b.len()) {
            return Ok(Some(false));
        }
        self.heap.keep(source);
        self.heap.keep(target);
        let mut position = 0;
        while let Some((next, key, hash)) = self.next_set_key(source, position) {
            self.heap.keep(key);
            if !self.set_slot(target, key, hash)?.1 {
                return Ok(Some(false));
            }
            position = next;
        }
        Ok(Some(true))
    }

    fn reserve_set_union(&mut self, set: Id, additional: usize) -> Result<()> {
        let Value::Set { index, .. } = self.heap.get_mut(set) else {
            unreachable!()
        };
        let total = index
            .hashes
            .len()
            .checked_add(additional)
            .ok_or_else(memory_error)?;
        if total.saturating_mul(5) >= (index.size() - 1).saturating_mul(3) {
            let size = capacity(total.checked_mul(2).ok_or_else(memory_error)?)?;
            let mut slots = empty_table(size)?;
            index.repopulate(&mut slots);
            index.slots = slots;
        }
        Ok(())
    }

    /// Exact sets and dictionaries supply stored hashes. Other iterables yield one item at a time.
    pub(super) fn update_set(&mut self, set: Id, source: Id) -> Result<()> {
        if set == source {
            return Ok(());
        }
        self.heap.keep(set);
        self.heap.keep(source);
        if let Value::Set { items, index, .. } = self.heap.get(source) {
            if items.is_empty() {
                return Ok(());
            }
            if matches!(self.heap.get(set), Value::Set { items, .. } if items.is_empty()) {
                let copied = (copy_slice(items)?, index.copied()?);
                let Value::Set { items, index, .. } = self.heap.get_mut(set) else {
                    unreachable!()
                };
                (*items, *index) = copied;
                return Ok(());
            }
            self.reserve_set_union(set, items.len())?;
            let mut position = 0;
            while let Some((next, key, hash)) = self.next_set_key(source, position) {
                self.insert_set_key(set, key, Some(hash))?;
                position = next;
            }
        } else if let Value::Dict(entries) = self.heap.get(source) {
            self.reserve_set_union(set, entries.len())?;
            let mut position = 0;
            while let Some((next, key, _, hash)) = self.dictionary_pair(source, position) {
                self.insert_set_key(set, key, hash)?;
                position = next + 1;
            }
        } else {
            let iterator = self.iterator(source)?;
            while let Some(item) = self.next_item(iterator)? {
                self.insert_set_key(set, item, None)?;
            }
        }
        Ok(())
    }
}
