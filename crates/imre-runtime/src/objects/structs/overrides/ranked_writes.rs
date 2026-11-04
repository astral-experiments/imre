use super::{BLOCK, Binary, position};
use num_integer::Integer;

impl Binary {
    /// Write a partial modular orbit in address order by computing each word's mask.
    #[inline(never)]
    pub(super) fn fill_ranked_prefix(
        &mut self,
        at: usize,
        step: usize,
        n: usize,
        count: usize,
        spacing: usize,
    ) -> Option<(usize, usize)> {
        let words = n / BLOCK;
        if !n.is_multiple_of(BLOCK)
            || !BLOCK.is_multiple_of(spacing)
            || count < 1024
            || count / 8 < words
        {
            return None;
        }
        let period = n / spacing;
        let visits = count + 1; // Include the seed at ordinal zero.
        let inverse = ((step / spacing) as i128)
            .extended_gcd(&(period as i128))
            .x
            .rem_euclid(period as i128) as usize;
        let rank = |index: usize| (index as u128 * inverse as u128 % period as u128) as usize;
        let add_rank = |a: usize, b: usize| {
            if a >= period - b {
                a - (period - b)
            } else {
                a + b
            }
        };
        let slots = BLOCK / spacing;
        let advance = rank(slots);
        let mut first = (period - rank(at / spacing)) % period;

        // For each bit, its ordinal is (first + offset) % period. It belongs
        // to the prefix between two transitions in first. Sorting those
        // transitions gives the entire word mask for any starting ordinal.
        let mut events = [(0usize, 0u64); 2 * BLOCK];
        let events = &mut events[..2 * slots];
        let mut mask = 0;
        let (pairs, _) = events.as_chunks_mut::<2>();
        for (slot, [begin, end]) in pairs.iter_mut().enumerate() {
            let offset = rank(slot);
            let bit = 1u64 << (at % spacing + slot * spacing);
            let start = (period - offset) % period;
            *begin = (start, bit);
            *end = (add_rank(start, visits), bit);
            // The prefix mask immediately before the first transition at zero.
            if offset.checked_sub(1).unwrap_or(period - 1) < visits {
                mask |= bit;
            }
        }
        events.sort_unstable_by_key(|event| event.0);
        for (_, value) in &mut *events {
            mask ^= *value;
            *value = mask;
        }

        let mut written = 0;
        while written < words && !crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Relaxed)
        {
            let end = (written + 256).min(words);
            for word in &mut self.bits[written..end] {
                // Slot zero always has a transition at ordinal zero.
                let after = events.partition_point(|event| event.0 <= first);
                *word = events[after - 1].1;
                first = add_rank(first, advance);
            }
            written = end;
        }
        if written < words || crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Relaxed) {
            self.bits[..written].fill(0);
            let (block, seed) = position(at);
            self.bits[block] = seed;
            return Some((0, at));
        }
        self.changed = visits;
        let last = (at as u128 + count as u128 * step as u128) % n as u128;
        Some((count, last as usize))
    }
}
