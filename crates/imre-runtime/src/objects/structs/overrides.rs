//! Store sparse exceptions by occupied block, or one alternate scalar row and a bitmap.
use super::{Error, Result};
use std::collections::HashMap;

const BLOCK: usize = u64::BITS as usize;

#[derive(Default)]
struct Block {
    mask: u64,
    words: Vec<usize>,
}

impl Block {
    fn offset(&self, bit: u64, width: usize) -> usize {
        (self.mask & (bit - 1)).count_ones() as usize * width
    }
}

/// Each set bit owns one block descriptor, packed in bit order.
#[derive(Default)]
struct Page {
    mask: u64,
    blocks: Vec<Block>,
}

impl Page {
    fn rank(&self, bit: u64) -> usize {
        (self.mask & (bit - 1)).count_ones() as usize
    }

    /// Allocate the descriptor before making its bit visible.
    fn insert(&mut self, bit: u64, block: Block) -> Result<()> {
        self.blocks
            .try_reserve(1)
            .map_err(|_| Error::allocation())?;
        self.blocks.insert(self.rank(bit), block);
        self.mask |= bit;
        Ok(())
    }

    /// The least occupied block holds the least occupied row in this page.
    fn first(&self, key: usize) -> usize {
        (key * BLOCK + self.mask.trailing_zeros() as usize) * BLOCK
            + self.blocks[0].mask.trailing_zeros() as usize
    }
}

/// Keep sparse page keys and descriptors in separate arrays. When every page
/// between the endpoints is populated, the key itself addresses the descriptor.
#[derive(Default)]
struct Pages {
    keys: Vec<usize>,
    values: Vec<Page>,
    index: Option<HashMap<usize, usize>>,
}

impl Pages {
    // Bound the cost of moving descriptors on an out-of-order insertion.
    const ORDERED_LIMIT: usize = 1024;

    /// A miss gives the ordered insertion point or the next free indexed slot.
    fn find(&self, key: usize) -> std::result::Result<usize, usize> {
        if let Some(index) = &self.index {
            return index.get(&key).copied().ok_or(self.keys.len());
        }
        if let Some(&first) = self.keys.first() {
            let slot = key.wrapping_sub(first);
            if self.keys.get(slot) == Some(&key) {
                return Ok(slot);
            }
        }
        self.keys.binary_search(&key)
    }

    /// Reserve every allocation before publishing the new descriptor or its index.
    fn insert(&mut self, slot: usize, key: usize, page: Page) -> Result<()> {
        self.keys.try_reserve(1).map_err(|_| Error::allocation())?;
        self.values
            .try_reserve(1)
            .map_err(|_| Error::allocation())?;
        if self.index.is_none() && self.keys.len() == Self::ORDERED_LIMIT {
            let mut index = HashMap::new();
            index
                .try_reserve(self.keys.len() + 1)
                .map_err(|_| Error::allocation())?;
            index.extend(self.keys.iter().enumerate().map(|(i, &key)| (key, i)));
            self.index = Some(index);
        }
        if let Some(index) = &mut self.index {
            index.try_reserve(1).map_err(|_| Error::allocation())?;
            index.insert(key, self.keys.len());
            self.keys.push(key);
            self.values.push(page);
        } else {
            self.keys.insert(slot, key);
            self.values.insert(slot, page);
        }
        Ok(())
    }

    /// Remove an empty descriptor and repair the index if the last block moves.
    fn remove(&mut self, slot: usize) {
        if let Some(index) = &mut self.index {
            index.remove(&self.keys.swap_remove(slot));
            self.values.swap_remove(slot);
            if let Some(key) = self.keys.get(slot) {
                *index.get_mut(key).unwrap() = slot;
            }
        } else {
            self.keys.remove(slot);
            self.values.remove(slot);
        }
    }

    /// All later bits in this page are empty. Ordered pages can also skip the gap to the next page.
    fn gap_end(&self, next: usize, at: usize, len: usize) -> usize {
        if self.index.is_some() {
            return (at | (BLOCK * BLOCK - 1)).saturating_add(1).min(len);
        }
        self.keys
            .get(next)
            .map_or(len, |&key| self.values[next].first(key))
    }
}

#[derive(Default)]
pub(super) struct Sparse {
    pages: Pages,
    occupied: usize,
    pub(super) len: usize,
    // Charge each linear dictionary probe to a new high-water mark. Repeated
    // insert/remove pairs at the same size must remain amortized constant time.
    last_binary_probe: usize,
    // Conservative bounds. Removing a boundary need not search all blocks.
    first: usize,
    last: usize,
}

fn position(at: usize) -> (usize, u64) {
    (at / BLOCK, 1 << (at % BLOCK))
}

/// Count occupied blocks only when the global density does not settle the choice.
fn should_pack(changed: usize, len: usize, occupied: impl FnOnce() -> usize) -> bool {
    changed > len / 4 || (changed > len / BLOCK && occupied() >= len.div_ceil(BLOCK).div_ceil(2))
}

impl Sparse {
    /// Build a bitmap only when it fits and all exceptions have the same payload.
    fn to_binary(&self, at: usize, len: usize, row: &[usize]) -> Option<Binary> {
        let words = len.div_ceil(BLOCK);
        let descriptor_words = self
            .pages
            .values
            .iter()
            .map(|page| page.blocks.capacity())
            .sum::<usize>()
            .saturating_mul(size_of::<Block>() / size_of::<u64>());
        let page_words = self
            .pages
            .values
            .capacity()
            .saturating_mul(size_of::<(usize, Page)>() / size_of::<u64>());
        if words
            > self
                .len
                .saturating_mul(row.len())
                .saturating_add(descriptor_words)
                .saturating_add(page_words)
        {
            return None;
        }
        if !self.iter(row.len()).all(|(_, old)| old == row) {
            return None;
        }
        let mut bits = Vec::new();
        let mut value = Vec::new();
        if bits.try_reserve_exact(words).is_err() || value.try_reserve_exact(row.len()).is_err() {
            return None;
        }
        bits.resize(words, 0);
        value.extend_from_slice(row);
        for (i, _) in self.iter(row.len()) {
            let (block, bit) = position(i);
            bits[block] |= bit;
        }
        let (block, bit) = position(at);
        bits[block] |= bit;
        Some(Binary {
            row: value,
            bits,
            changed: self.len + 1,
        })
    }

    /// Resolve a logical block through a page key and a population count.
    fn block(&self, key: usize) -> Option<&Block> {
        let (key, bit) = position(key);
        let page = &self.pages.values[self.pages.find(key).ok()?];
        (page.mask & bit != 0).then(|| &page.blocks[page.rank(bit)])
    }

    pub(super) fn get(&self, at: usize, width: usize) -> Option<&[usize]> {
        let (key, bit) = position(at);
        let block = self.block(key)?;
        if block.mask & bit == 0 {
            return None;
        }
        let pos = block.offset(bit, width);
        Some(&block.words[pos..pos + width])
    }

    /// Reserve before changing either membership mask or the logical row count.
    pub(super) fn put(&mut self, at: usize, row: &[usize]) -> Result<()> {
        let (key, bit) = position(at);
        let (region, occupied) = position(key);
        let slot = self.pages.find(region);
        let existing = slot.ok().and_then(|i| {
            let page = &mut self.pages.values[i];
            let pos = page.rank(occupied);
            (page.mask & occupied != 0).then(|| &mut page.blocks[pos])
        });
        if let Some(block) = existing {
            let pos = block.offset(bit, row.len());
            if block.mask & bit != 0 {
                block.words[pos..pos + row.len()].copy_from_slice(row);
                return Ok(());
            }
            let end = block.words.len();
            block
                .words
                .try_reserve(row.len())
                .map_err(|_| Error::allocation())?;
            block.words.resize(end + row.len(), 0);
            block.words.copy_within(pos..end, pos + row.len());
            block.words[pos..pos + row.len()].copy_from_slice(row);
            block.mask |= bit;
        } else {
            let mut words = Vec::new();
            words
                .try_reserve_exact(row.len())
                .map_err(|_| Error::allocation())?;
            words.extend_from_slice(row);
            let block = Block { mask: bit, words };
            match slot {
                Ok(i) => self.pages.values[i].insert(occupied, block)?,
                Err(i) => {
                    let mut page = Page::default();
                    page.insert(occupied, block)?;
                    self.pages.insert(i, region, page)?;
                }
            }
            self.occupied += 1;
        }
        self.first = if self.len == 0 {
            at
        } else {
            self.first.min(at)
        };
        self.last = self.last.max(at);
        self.len += 1;
        Ok(())
    }

    pub(super) fn remove(&mut self, at: usize, width: usize) {
        if self.len == 0 {
            return;
        }
        let (key, bit) = position(at);
        let (region, occupied) = position(key);
        let Ok(slot) = self.pages.find(region) else {
            return;
        };
        let page = &mut self.pages.values[slot];
        if page.mask & occupied == 0 {
            return;
        }
        let i = page.rank(occupied);
        let block = &mut page.blocks[i];
        if block.mask & bit == 0 {
            return;
        }
        let pos = block.offset(bit, width);
        block.words.drain(pos..pos + width);
        block.mask ^= bit;
        if block.mask == 0 {
            page.blocks.remove(i);
            page.mask ^= occupied;
            self.occupied -= 1;
            if page.mask == 0 {
                self.pages.remove(slot);
            }
        }
        self.len -= 1;
        if self.len == 0 {
            *self = Self::default();
        }
    }

    pub(super) fn iter(&self, width: usize) -> impl Iterator<Item = (usize, &[usize])> {
        self.pages
            .keys
            .iter()
            .zip(&self.pages.values)
            .flat_map(move |(&region, page)| {
                let mut occupied = page.mask;
                page.blocks.iter().flat_map(move |block| {
                    let key = region * BLOCK + occupied.trailing_zeros() as usize;
                    occupied &= occupied - 1;
                    let mut bits = block.mask;
                    block.words.chunks_exact(width).map(move |row| {
                        let at = key * BLOCK + bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        (at, row)
                    })
                })
            })
    }

    /// Describe a run without testing its bitmap on every visit. Adjacent edits
    /// occupy adjacent scalar rows; every untouched slot shares the default.
    pub(super) fn span(&self, at: usize, len: usize, default: &[usize]) -> super::ReadSpan {
        let uniform = |count| super::ReadSpan {
            start: at,
            len: count,
            index_mask: 0,
            data: default.as_ptr(),
        };
        if self.len == 0 || at > self.last {
            return uniform(len - at);
        }
        if at < self.first {
            return uniform(self.first - at);
        }
        let (key, bit) = position(at);
        let (region, occupied) = position(key);
        let slot = match self.pages.find(region) {
            Ok(slot) => slot,
            Err(next) => return uniform(self.pages.gap_end(next, at, len) - at),
        };
        let page = &self.pages.values[slot];
        let mut next = page.rank(occupied);
        let mut remaining = page.mask & !(occupied - 1);
        if remaining & occupied != 0 {
            let block = &page.blocks[next];
            let bits = block.mask >> (at % BLOCK);
            if bits & 1 != 0 {
                return super::ReadSpan {
                    start: at,
                    len: bits.trailing_ones() as usize,
                    index_mask: usize::MAX,
                    data: block.words[block.offset(bit, default.len())..].as_ptr(),
                };
            }
            if bits != 0 {
                return uniform(bits.trailing_zeros() as usize);
            }
            remaining ^= occupied;
            next += 1;
        }
        let end = if remaining == 0 {
            self.pages.gap_end(slot + 1, at, len)
        } else {
            (region * BLOCK + remaining.trailing_zeros() as usize) * BLOCK
                + page.blocks[next].mask.trailing_zeros() as usize
        };
        uniform(end - at)
    }
}

/// A low-cardinality list needs one scalar payload per distinct row. A set bit
/// selects the alternate payload, regardless of the row's width.
pub(super) struct Binary {
    pub(super) row: Vec<usize>,
    pub(super) bits: Vec<u64>,
    changed: usize,
}

impl Binary {
    fn contains(&self, at: usize) -> bool {
        let (block, bit) = position(at);
        self.bits.get(block).is_some_and(|word| word & bit != 0)
    }

    fn insert(&mut self, at: usize) -> bool {
        let (block, bit) = position(at);
        if block >= self.bits.len() {
            if self.bits.try_reserve(block + 1 - self.bits.len()).is_err() {
                return false;
            }
            self.bits.resize(block + 1, 0);
        }
        let word = &mut self.bits[block];
        self.changed += usize::from(*word & bit == 0);
        *word |= bit;
        true
    }

    fn remove(&mut self, at: usize) {
        let (block, bit) = position(at);
        if let Some(word) = self.bits.get_mut(block) {
            self.changed -= usize::from(*word & bit != 0);
            *word &= !bit;
        }
    }

    fn iter(&self) -> impl Iterator<Item = (usize, &[usize])> {
        self.bits
            .iter()
            .enumerate()
            .flat_map(move |(block, &word)| {
                let mut bits = word;
                std::iter::from_fn(move || {
                    if bits == 0 {
                        return None;
                    }
                    let at = block * BLOCK + bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    Some((at, self.row.as_slice()))
                })
            })
    }

    fn span(&self, at: usize, len: usize, default: &[usize]) -> super::ReadSpan {
        let offset = at % BLOCK;
        let bits = self.bits.get(at / BLOCK).copied().unwrap_or(0) >> offset;
        let (row, count) = if bits & 1 == 0 {
            (default, bits.trailing_zeros())
        } else {
            (self.row.as_slice(), bits.trailing_ones())
        };
        super::ReadSpan {
            start: at,
            len: (count as usize).min(BLOCK - offset).min(len - at),
            index_mask: 0,
            data: row.as_ptr(),
        }
    }
}

pub(super) enum Overrides {
    Sparse(Sparse),
    Binary(Binary),
}

impl Default for Overrides {
    fn default() -> Self {
        Self::Sparse(Sparse::default())
    }
}

impl Overrides {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Sparse(edits) => edits.len,
            Self::Binary(edits) => edits.changed,
        }
    }

    pub(super) fn get(&self, at: usize, width: usize) -> Option<&[usize]> {
        match self {
            Self::Sparse(edits) if edits.len == 0 || at < edits.first || at > edits.last => None,
            Self::Sparse(edits) => edits.get(at, width),
            Self::Binary(edits) => edits.contains(at).then_some(edits.row.as_slice()),
        }
    }

    /// Write either known row directly, or try promoting sparse exceptions as their count doubles.
    pub(super) fn write_known(
        &mut self,
        at: usize,
        len: usize,
        row: &[usize],
        default: &[usize],
    ) -> bool {
        let sparse = match self {
            Self::Binary(binary) => {
                if binary.row == row {
                    return binary.insert(at);
                }
                if row == default {
                    binary.remove(at);
                    return true;
                }
                return false;
            }
            Self::Sparse(sparse) => sparse,
        };
        if row == default {
            sparse.remove(at, row.len());
            return true;
        }
        if !sparse.len.is_power_of_two()
            || sparse.len <= sparse.last_binary_probe
            || sparse.get(at, row.len()).is_some()
        {
            return false;
        }
        sparse.last_binary_probe = sparse.len;
        let Some(binary) = sparse.to_binary(at, len, row) else {
            return false;
        };
        *self = Self::Binary(binary);
        true
    }

    pub(super) fn should_pack(&self, at: usize, len: usize, width: usize) -> bool {
        match self {
            Self::Sparse(edits) => {
                should_pack(edits.len + 1, len, || {
                    edits.occupied + usize::from(edits.block(at / BLOCK).is_none())
                }) && edits.get(at, width).is_none()
            }
            Self::Binary(edits) => {
                // A third distinct row ends the dictionary encoding. Preserve
                // sparse storage when scattering its payloads is still cheaper.
                let block = at / BLOCK;
                should_pack(
                    edits.changed + usize::from(!edits.contains(at)),
                    len,
                    || {
                        edits.bits.iter().filter(|&&word| word != 0).count()
                            + usize::from(edits.bits.get(block).copied().unwrap_or(0) == 0)
                    },
                )
            }
        }
    }

    pub(super) fn put(&mut self, at: usize, row: &[usize]) -> Result<()> {
        match self {
            Self::Sparse(edits) => edits.put(at, row),
            Self::Binary(binary) => {
                // Build the sparse representation before replacing the bitmap.
                // A failed payload allocation must leave the old list intact.
                let mut sparse = Sparse {
                    last_binary_probe: binary.changed,
                    ..Sparse::default()
                };
                for (slot, old) in binary.iter() {
                    sparse.put(slot, old)?;
                }
                sparse.put(at, row)?;
                *self = Self::Sparse(sparse);
                Ok(())
            }
        }
    }

    pub(super) fn remove(&mut self, at: usize, width: usize) {
        match self {
            Self::Sparse(edits) => edits.remove(at, width),
            Self::Binary(binary) => {
                binary.remove(at);
                if binary.changed == 0 {
                    *self = Self::default();
                }
            }
        }
    }

    pub(super) fn iter(&self, width: usize) -> impl Iterator<Item = (usize, &[usize])> {
        let (sparse, binary) = match self {
            Self::Sparse(s) => (Some(s), None),
            Self::Binary(b) => (None, Some(b)),
        };
        sparse
            .into_iter()
            .flat_map(move |s| s.iter(width))
            .chain(binary.into_iter().flat_map(Binary::iter))
    }

    pub(super) fn span(&self, at: usize, len: usize, default: &[usize]) -> super::ReadSpan {
        match self {
            Self::Sparse(s) => s.span(at, len, default),
            Self::Binary(b) => b.span(at, len, default),
        }
    }
}

#[cfg(test)]
mod page_tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Cross-check the entire span partition, including arbitrarily long gaps,
    /// without allocating or visiting one element per logical default row.
    fn check(rows: &Sparse, expected: &BTreeMap<usize, [usize; 2]>, len: usize) {
        let default = [0, 0];
        let mut at = 0;
        while at < len {
            let span = rows.span(at, len, &default);
            assert_eq!(span.start, at);
            assert!((1..=len - at).contains(&span.len));
            let end = at + span.len;
            if span.index_mask == 0 {
                assert_eq!(span.data, default.as_ptr());
                assert!(expected.range(at..end).next().is_none());
            } else {
                assert_eq!(span.index_mask, usize::MAX);
                // The sparse directory is borrowed immutably for this traversal.
                let words = unsafe { std::slice::from_raw_parts(span.data, span.len * 2) };
                for (index, payload) in (at..end).zip(words.chunks_exact(2)) {
                    assert_eq!(payload, expected[&index]);
                }
            }
            at = end;
        }
        assert_eq!(rows.len, expected.len());
        assert!(
            rows.iter(2)
                .all(|(at, row)| expected.get(&at).is_some_and(|v| row == v))
        );
        for (&at, value) in expected {
            assert_eq!(rows.get(at, 2), Some(value.as_slice()));
        }
    }

    #[test]
    fn sparse_spans_cross_block_page_and_integer_boundaries() {
        let mut sparse = Sparse::default();
        let mut expected = BTreeMap::new();
        let far = usize::MAX - 8191;
        for at in [far, 8191, 0, 4097, 63, 65, 4096, 64, 4095, far + 63] {
            let row = [at, !at];
            assert!(sparse.put(at, &row).is_ok());
            expected.insert(at, row);
        }
        check(&sparse, &expected, far + 65);
        for at in [0, 4096, far + 63, 8191, 63, 4095] {
            sparse.remove(at, 2);
            expected.remove(&at);
            check(&sparse, &expected, far + 65);
        }
    }

    #[test]
    fn removing_indexed_pages_repairs_moved_descriptors() {
        let mut sparse = Sparse::default();
        let mut expected = BTreeMap::new();
        let pages = Pages::ORDERED_LIMIT + 1;
        for i in (0..pages).rev() {
            let at = i * BLOCK * BLOCK + i % BLOCK;
            let row = [i, !i];
            assert!(sparse.put(at, &row).is_ok());
            expected.insert(at, row);
        }
        let len = pages * BLOCK * BLOCK;
        check(&sparse, &expected, len);
        for i in (0..pages).step_by(3) {
            let at = i * BLOCK * BLOCK + i % BLOCK;
            sparse.remove(at, 2);
            expected.remove(&at);
        }
        check(&sparse, &expected, len);
    }
}
