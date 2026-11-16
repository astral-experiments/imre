//! Copy compact ASCII integer maps through four register-sized lookup tables.

use super::Int;
use std::arch::x86_64::*;
use std::mem::MaybeUninit;

/// Try the full sixteen-character blocks when all present keys fit one window
/// of 64 ASCII codes and their tagged values fit in a byte.
pub(super) fn copy_prefix(
    values: &[usize; 128],
    input: &[u8],
    output: &mut [MaybeUninit<Int>],
) -> Option<(usize, usize)> {
    let blocks = input.len().min(output.len()) / 16;
    if blocks < 32 || !std::is_x86_feature_detected!("avx2") {
        return None;
    }
    let first = values.iter().position(|&word| word != 0)?;
    let base = (first & !15).min(64);
    if values[base + 64..].iter().any(|&word| word != 0) {
        return None;
    }
    let mut table = [[0; 16]; 4];
    for (byte, &word) in table.as_flattened_mut().iter_mut().zip(&values[base..]) {
        *byte = u8::try_from(word).ok()?;
    }
    // SAFETY: AVX2 was detected above. Both slices contain exactly this many
    // full blocks and the table holds only missing or immediate integer words.
    let complete = unsafe {
        copy_blocks(
            &input.as_chunks::<16>().0[..blocks],
            &mut output.as_chunks_mut::<16>().0[..blocks],
            base as u8,
            &table,
        )
    };
    Some((blocks * 16, usize::from(complete)))
}

/// Look up the low nibble in parallel and select its table with bits 4 and 5.
#[target_feature(enable = "avx2")]
unsafe fn copy_blocks(
    input: &[[u8; 16]],
    output: &mut [[MaybeUninit<Int>; 16]],
    base: u8,
    table: &[[u8; 16]; 4],
) -> bool {
    // SAFETY: each table row is one full unaligned vector.
    let [t0, t1, t2, t3] = table
        .each_ref()
        .map(|row| unsafe { _mm_loadu_si128(row.as_ptr().cast()) });
    let zero = _mm_setzero_si128();
    let mut missing = zero;
    for (text, slots) in input.iter().zip(output) {
        // SAFETY: the input array spans exactly sixteen bytes.
        let chars = unsafe { _mm_loadu_si128(text.as_ptr().cast()) };
        let keys = _mm_sub_epi8(chars, _mm_set1_epi8(base as i8));
        let bit4 = _mm_slli_epi16::<3>(keys);
        let low = _mm_blendv_epi8(_mm_shuffle_epi8(t0, keys), _mm_shuffle_epi8(t1, keys), bit4);
        let high = _mm_blendv_epi8(_mm_shuffle_epi8(t2, keys), _mm_shuffle_epi8(t3, keys), bit4);
        let selected = _mm_blendv_epi8(low, high, _mm_slli_epi16::<2>(keys));
        let in_window = _mm_cmpeq_epi8(_mm_min_epu8(keys, _mm_set1_epi8(63)), keys);
        let words = _mm_and_si128(selected, in_window);
        missing = _mm_or_si128(missing, _mm_cmpeq_epi8(words, zero));
        // A missing word becomes immediate integer zero until the caller
        // applies the mapping's missing-key policy.
        let words = _mm_or_si128(words, _mm_set1_epi8(1));
        let groups = [
            words,
            _mm_srli_si128::<4>(words),
            _mm_srli_si128::<8>(words),
            _mm_srli_si128::<12>(words),
        ];
        for (dst, group) in slots.as_chunks_mut::<4>().0.iter_mut().zip(groups) {
            // SAFETY: on x86-64 each of these four transparent Int slots is
            // eight bytes. Widening initializes their canonical tagged words.
            unsafe { _mm256_storeu_si256(dst.as_mut_ptr().cast(), _mm256_cvtepu8_epi64(group)) };
        }
    }
    _mm_movemask_epi8(missing) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_windows_match_every_ascii_key_and_missing_word() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        let mut values = [0; 128];
        let mut output = [const { MaybeUninit::uninit() }; 528];
        for base in [0, 16, 32, 48, 64] {
            values.fill(0);
            for (i, word) in values.iter_mut().enumerate().skip(base).take(64) {
                if !i.is_multiple_of(7) {
                    *word = 2 * i + 1;
                }
            }
            for complete in [true, false] {
                let input: [u8; 529] = std::array::from_fn(|i| {
                    if complete {
                        (base + 1 + usize::from((base + 1).is_multiple_of(7))) as u8
                    } else {
                        (i % 128) as u8
                    }
                });
                let (n, present) = copy_prefix(&values, &input, &mut output).unwrap();
                assert_eq!((n, present), (528, usize::from(complete)));
                for (slot, &key) in output.iter().zip(&input) {
                    // SAFETY: the reported prefix initialized all 528 slots.
                    let word = unsafe { slot.assume_init_ref() }.0;
                    assert_eq!(word, values[usize::from(key)] | 1);
                }
            }
        }
        values[0] = 1;
        assert!(copy_prefix(&values, &[0; 528], &mut output).is_none());
        values.fill(0);
        values[2] = 257;
        assert!(copy_prefix(&values, &[2; 528], &mut output).is_none());
    }
}
