//! Update regularly spaced bits a word at a time, in source index order.

use super::{BLOCK, Binary};

impl Binary {
    /// Group regularly spaced stores by bitmap word, preserving visit order across wraps.
    #[inline(never)]
    pub(super) fn set_strided(
        &mut self,
        mut at: usize,
        stride: usize,
        forward: bool,
        n: usize,
        count: usize,
        set: bool,
    ) -> (usize, usize) {
        let shift = stride.trailing_zeros();
        let pattern = u64::MAX / ((1u64 << stride) - 1);
        let mut done = 0;
        while done < count && !crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Relaxed) {
            let first = if forward {
                let next = at + stride;
                if next >= n { next - n } else { next }
            } else if at < stride {
                n - stride + at
            } else {
                at - stride
            };
            let available = if forward { n - 1 - first } else { first };
            let visits = (count - done).min(1024).min((available >> shift) + 1);
            let span = (visits - 1) << shift;
            let range = if forward {
                first..first + span + 1
            } else {
                first - span..first + 1
            };
            at = if forward { range.end - 1 } else { range.start };
            self.fill_range(range, pattern << (first & (stride - 1)), set);
            done += visits;
        }
        (done, at)
    }

    /// Apply a repeating word mask within the range and count the bits that change.
    fn fill_range(&mut self, range: std::ops::Range<usize>, pattern: u64, set: bool) {
        let end = range.end.min(self.bits.len().saturating_mul(BLOCK));
        let mut at = range.start;
        let mut changed = 0;
        while at < end {
            let offset = at % BLOCK;
            if offset == 0 && end - at >= BLOCK {
                let last = end / BLOCK;
                let target = if set { u64::MAX } else { 0 };
                for word in &mut self.bits[at / BLOCK..last] {
                    let flips = pattern & (*word ^ target);
                    changed += flips.count_ones() as usize;
                    *word ^= flips;
                }
                at = last * BLOCK;
                continue;
            }
            let width = (end - at).min(BLOCK - offset);
            let mask = (u64::MAX >> (BLOCK - width)) << offset;
            let word = &mut self.bits[at / BLOCK];
            let flips = mask & pattern & if set { !*word } else { *word };
            changed += flips.count_ones() as usize;
            *word ^= flips;
            at += width;
        }
        if set {
            self.changed += changed;
        } else {
            self.changed -= changed;
        }
    }
}
