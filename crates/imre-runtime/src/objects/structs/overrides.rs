//! Changed slots in an otherwise uniform list. Each occupied block owns only
//! its changed scalar rows, in bit order; no directory is sized by list length.
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
pub(super) struct Overrides {
    blocks: HashMap<usize, Block>,
    pub(super) len: usize,
    // Conservative bounds. Removing a boundary need not search all blocks.
    first: usize,
    last: usize,
}

fn position(at: usize) -> (usize, u64) {
    (at / BLOCK, 1 << (at % BLOCK))
}

impl Overrides {
    /// Called for a potential new exception. Spread matters: a scan can reuse
    /// one row pointer across neighboring edits but must seek between outliers.
    pub(super) fn should_pack_after_insert(&self, at: usize, len: usize) -> bool {
        let changed = self.len + 1;
        if changed > len / 4 {
            return true;
        }
        if changed <= len / BLOCK {
            return false;
        }
        let blocks = self.blocks.len() + usize::from(!self.blocks.contains_key(&(at / BLOCK)));
        blocks >= len.div_ceil(BLOCK).div_ceil(2)
    }

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
