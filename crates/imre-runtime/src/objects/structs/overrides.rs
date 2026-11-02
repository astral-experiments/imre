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

#[derive(Default)]
pub(super) struct Sparse {
    blocks: HashMap<usize, Block>,
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
    pub(super) fn get(&self, at: usize, width: usize) -> Option<&[usize]> {
        if self.len == 0 || at < self.first || at > self.last {
            return None;
        }
        let (key, bit) = position(at);
        let block = self.blocks.get(&key)?;
        if block.mask & bit == 0 {
            return None;
        }
        let pos = block.offset(bit, width);
        Some(&block.words[pos..pos + width])
    }

    /// Reserve before changing the mask, so allocation failure leaves all slots intact.
    pub(super) fn put(&mut self, at: usize, row: &[usize]) -> Result<()> {
        let (key, bit) = position(at);
        if let Some(block) = self.blocks.get_mut(&key) {
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
            self.blocks
                .try_reserve(1)
                .map_err(|_| Error::allocation())?;
            self.blocks.insert(key, Block { mask: bit, words });
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
        let Some(block) = self.blocks.get_mut(&key) else {
            return;
        };
        if block.mask & bit == 0 {
            return;
        }
        let pos = block.offset(bit, width);
        block.words.drain(pos..pos + width);
        block.mask ^= bit;
        if block.mask == 0 {
            self.blocks.remove(&key);
        }
        self.len -= 1;
        if self.len == 0 {
            *self = Self::default();
        }
    }

    pub(super) fn iter(&self, width: usize) -> impl Iterator<Item = (usize, &[usize])> {
        self.blocks.iter().flat_map(move |(&key, block)| {
            let mut bits = block.mask;
            block.words.chunks_exact(width).map(move |row| {
                let at = key * BLOCK + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                (at, row)
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
        let offset = at % BLOCK;
        let limit = (BLOCK - offset).min(len - at);
        let Some(block) = self.blocks.get(&(at / BLOCK)) else {
            return uniform(limit);
        };
        let bits = block.mask >> offset;
        if bits & 1 == 0 {
            uniform((bits.trailing_zeros() as usize).min(limit))
        } else {
            let start = block.offset(1 << offset, default.len());
            super::ReadSpan {
                start: at,
                len: (bits.trailing_ones() as usize).min(limit),
                index_mask: usize::MAX,
                data: block.words[start..].as_ptr(),
            }
        }
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
            Self::Sparse(edits) => edits.get(at, width),
            Self::Binary(edits) => edits.contains(at).then_some(edits.row.as_slice()),
        }
    }

    /// Try exact two-row encoding at geometric growth points. The bitmap must
    /// fit within the estimated storage already paid for by sparse exceptions.
    pub(super) fn write_binary(&mut self, at: usize, len: usize, row: &[usize]) -> bool {
        let sparse = match self {
            Self::Binary(binary) => {
                return binary.row == row && binary.insert(at);
            }
            Self::Sparse(sparse) => sparse,
        };
        let words = len.div_ceil(BLOCK);
        if !sparse.len.is_power_of_two()
            || sparse.len <= sparse.last_binary_probe
            || words
                > sparse.len.saturating_mul(row.len()).saturating_add(
                    sparse
                        .blocks
                        .capacity()
                        .saturating_mul(size_of::<(usize, Block)>() / size_of::<u64>()),
                )
            || sparse.get(at, row.len()).is_some()
        {
            return false;
        }
        sparse.last_binary_probe = sparse.len;
        if !sparse.iter(row.len()).all(|(_, old)| old == row) {
            return false;
        }
        let mut bits = Vec::new();
        let mut value = Vec::new();
        if bits.try_reserve_exact(words).is_err() || value.try_reserve_exact(row.len()).is_err() {
            return false;
        }
        bits.resize(words, 0);
        value.extend_from_slice(row);
        for (i, _) in sparse.iter(row.len()) {
            let (block, bit) = position(i);
            bits[block] |= bit;
        }
        let (block, bit) = position(at);
        bits[block] |= bit;
        *self = Self::Binary(Binary {
            row: value,
            bits,
            changed: sparse.len + 1,
        });
        true
    }

    pub(super) fn should_pack(&self, at: usize, len: usize, width: usize) -> bool {
        match self {
            Self::Sparse(edits) => {
                should_pack(edits.len + 1, len, || {
                    edits.blocks.len() + usize::from(!edits.blocks.contains_key(&(at / BLOCK)))
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
