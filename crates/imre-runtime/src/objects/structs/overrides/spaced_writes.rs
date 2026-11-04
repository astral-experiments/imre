//! Update short strides whose selected positions shift between bitmap words.
use super::{BLOCK, Binary};
impl Binary {
    /// Carry the first selected bit across words within each uninterrupted span.
    #[inline(never)]
    pub(super) fn set_spaced(
        &mut self,
        mut at: usize,
        stride: usize,
        forward: bool,
        n: usize,
        count: usize,
        set: bool,
    ) -> (usize, usize) {
        let mut pattern = 0;
        for bit in (0..BLOCK).step_by(stride) {
            pattern |= 1u64 << bit;
        }
        let rewind = BLOCK % stride;
        let target = if set { u64::MAX } else { 0 };
        let mut changed = 0;
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
            let visits = (count - done).min(1024).min(available / stride + 1);
            let span = (visits - 1) * stride;
            let (low, high) = if forward {
                (first, first + span)
            } else {
                (first - span, first)
            };
            at = if forward { high } else { low };
            let first_word = low / BLOCK;
            let end = (high / BLOCK + 1).min(self.bits.len());
            let mut phase = (low % BLOCK) % stride;
            if let Some(words) = self.bits.get_mut(first_word..end) {
                for (i, word) in words.iter_mut().enumerate() {
                    let mut mask = pattern << phase;
                    if i == 0 {
                        mask &= u64::MAX << (low % BLOCK);
                    }
                    if first_word + i == high / BLOCK {
                        mask &= u64::MAX >> (BLOCK - 1 - high % BLOCK);
                    }
                    let flips = mask & (*word ^ target);
                    changed += flips.count_ones() as usize;
                    *word ^= flips;
                    phase = if phase >= rewind {
                        phase - rewind
                    } else {
                        phase + stride - rewind
                    };
                }
            }
            done += visits;
        }
        if set {
            self.changed += changed;
        } else {
            self.changed -= changed;
        }
        (done, at)
    }
}
